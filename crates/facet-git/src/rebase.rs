//! Rebasing: onto another commit, and interactively — reorder, reword, edit, squash, fixup
//! and drop commits from a todo list Facet builds.
//!
//! Git asks an editor for the todo list (`GIT_SEQUENCE_EDITOR`) and for commit messages
//! (`GIT_EDITOR`). Facet's own executable stands in for both, like the askpass helper
//! (PLAN.md, G7): started with [`SEQUENCE_FLAG`] it copies the todo Facet prepared over git's;
//! started with [`MESSAGE_FLAG`] during a `reword` it writes the new message Facet kept for
//! that commit, and otherwise leaves git's message as it is (squash, continue after a
//! conflict). Both files live in `<git dir>/facet-rebase/`, so a rebase stopped at an `edit`
//! step still rewords correctly when it is continued later.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::operation::{operation, Progress};
use crate::parse::{parse_log, LogCommit, LOG_FORMAT};
use crate::{Git, GitError, RepoInfo};

/// First argument of the helper when git runs it as the sequence editor.
pub const SEQUENCE_FLAG: &str = "--facet-sequence-editor";
/// First argument of the helper when git runs it as the commit message editor.
pub const MESSAGE_FLAG: &str = "--facet-message-editor";
/// The git directory whose `facet-rebase/` the helper reads.
const GIT_DIR_ENV: &str = "FACET_GIT_DIR";
const STATE_DIR: &str = "facet-rebase";
const TODO_FILE: &str = "todo";

/// The editors git runs during a rebase or a `--continue`, as shell commands (git appends
/// the file name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editors {
    pub sequence: String,
    pub message: String,
}

impl Editors {
    /// Facet's executable as both editors.
    pub fn facet(exe: &Path) -> Self {
        let exe = shell_quote(&exe.to_string_lossy().replace('\\', "/"));
        Self {
            sequence: format!("{exe} {SEQUENCE_FLAG}"),
            message: format!("{exe} {MESSAGE_FLAG}"),
        }
    }

    /// Git's own no-op editor: messages and todo lists are taken as git prepared them.
    pub fn accept() -> Self {
        Self {
            sequence: ":".to_owned(),
            message: ":".to_owned(),
        }
    }

    /// `git` with these editors and the helper's git directory in its environment.
    pub fn apply(&self, git: &Git, git_dir: &Path) -> Git {
        git.with_env("GIT_SEQUENCE_EDITOR", &self.sequence)
            .with_env("GIT_EDITOR", &self.message)
            .with_env(GIT_DIR_ENV, git_dir.as_os_str())
    }
}

/// Single-quote for `sh` (git runs editors through the shell).
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// What to do with one commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TodoAction {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

impl TodoAction {
    pub const ALL: [Self; 6] = [
        Self::Pick,
        Self::Reword,
        Self::Edit,
        Self::Squash,
        Self::Fixup,
        Self::Drop,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Reword => "reword",
            Self::Edit => "edit",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pick => "Pick",
            Self::Reword => "Reword",
            Self::Edit => "Edit",
            Self::Squash => "Squash",
            Self::Fixup => "Fixup",
            Self::Drop => "Drop",
        }
    }

    /// What the action does, for tooltips.
    pub fn help(self) -> &'static str {
        match self {
            Self::Pick => "Keep the commit as it is",
            Self::Reword => "Keep the commit with a new message",
            Self::Edit => "Stop after this commit to amend it",
            Self::Squash => "Meld into the commit above, combining both messages",
            Self::Fixup => "Meld into the commit above, keeping only its message",
            Self::Drop => "Remove the commit",
        }
    }

    /// Melds into the previous kept commit.
    pub fn melds(self) -> bool {
        matches!(self, Self::Squash | Self::Fixup)
    }
}

/// One line of the todo list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub action: TodoAction,
    pub oid: String,
    pub subject: String,
    /// The new message for [`TodoAction::Reword`].
    pub message: Option<String>,
}

/// The commits an interactive rebase from `base` (exclusive; `None` for the root) would
/// replay, oldest first. Merge commits are refused: replaying them is a different operation
/// (`--rebase-merges`) whose todo list Facet does not edit.
pub async fn rebase_plan(
    git: &Git,
    root: &Path,
    base: Option<&str>,
) -> Result<Vec<LogCommit>, GitError> {
    let range = match base {
        Some(base) => format!("{base}..HEAD"),
        None => "HEAD".to_owned(),
    };
    let output = git
        .command([
            "log",
            "--reverse",
            "--topo-order",
            "-z",
            "--no-show-signature",
            LOG_FORMAT,
        ])
        .args([range.as_str(), "--"])
        .current_dir(root)
        .background()
        .run()
        .await?;
    let commits = parse_log(&output.stdout)?;
    if commits.is_empty() {
        return Err(GitError::Unsupported(
            "There are no commits after that one on the current branch.".to_owned(),
        ));
    }
    if commits.iter().any(|commit| commit.parents.len() > 1) {
        return Err(GitError::Unsupported(
            "Those commits include a merge. Interactive rebase works on a straight line of \
             commits; start after the merge."
                .to_owned(),
        ));
    }
    Ok(commits)
}

/// The todo file for `items`, checking it makes sense first.
pub fn todo_text(items: &[TodoItem]) -> Result<String, GitError> {
    let mut text = String::new();
    let mut kept = false;
    for item in items {
        if item.action.melds() && !kept {
            return Err(GitError::Unsupported(format!(
                "\"{}\" can't {} into a commit above it: there is none. Pick or reword the \
                 first commit.",
                item.subject,
                item.action.word()
            )));
        }
        if item.action == TodoAction::Reword
            && item.message.as_deref().is_none_or(|m| m.trim().is_empty())
        {
            return Err(GitError::Unsupported(format!(
                "Write the new message for \"{}\".",
                item.subject
            )));
        }
        kept |= item.action != TodoAction::Drop;
        let subject = item.subject.replace(['\r', '\n'], " ");
        text.push_str(&format!(
            "{} {} {}\n",
            item.action.word(),
            item.oid,
            subject
        ));
    }
    if !kept {
        return Err(GitError::Unsupported(
            "Every commit would be dropped; keep at least one, or reset instead.".to_owned(),
        ));
    }
    Ok(text)
}

fn state_dir(git_dir: &Path) -> PathBuf {
    git_dir.join(STATE_DIR)
}

/// Remove what Facet kept for a finished or abandoned rebase.
pub(crate) fn clear_state(git_dir: &Path) {
    if operation(git_dir).is_none() {
        let _ = std::fs::remove_dir_all(state_dir(git_dir));
    }
}

/// Rewrite the commits after `base` (`None`: from the root) as `items` say. Local changes
/// are auto-stashed for the duration (`--autostash`).
pub async fn interactive_rebase(
    git: &Git,
    info: &RepoInfo,
    base: Option<&str>,
    items: &[TodoItem],
    editors: &Editors,
) -> Result<Progress, GitError> {
    let todo = todo_text(items)?;
    if operation(&info.git_dir).is_some() {
        return Err(GitError::Unsupported(
            "Another operation is in progress; finish or abort it first.".to_owned(),
        ));
    }
    let dir = state_dir(&info.git_dir);
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join(TODO_FILE), todo).await?;
    for item in items {
        if let (TodoAction::Reword, Some(message)) = (item.action, &item.message) {
            tokio::fs::write(dir.join(format!("msg-{}", item.oid)), message.trim()).await?;
        }
    }
    let git = editors.apply(git, &info.git_dir);
    let mut command = git.command(["rebase", "--interactive", "--autostash"]);
    command = match base {
        Some(base) => command.arg(base),
        None => command.arg("--root"),
    };
    let result = run(command.current_dir(info.root()), &info.git_dir).await;
    if !matches!(result, Ok(Progress::Stopped { .. })) {
        clear_state(&info.git_dir);
    }
    result
}

/// Rebase the current branch onto `onto` (non-interactive), auto-stashing local changes.
pub async fn rebase_onto(
    git: &Git,
    info: &RepoInfo,
    onto: &str,
    editors: &Editors,
) -> Result<Progress, GitError> {
    if operation(&info.git_dir).is_some() {
        return Err(GitError::Unsupported(
            "Another operation is in progress; finish or abort it first.".to_owned(),
        ));
    }
    let git = editors.apply(git, &info.git_dir);
    run(
        git.command(["rebase", "--autostash", onto])
            .current_dir(info.root()),
        &info.git_dir,
    )
    .await
}

async fn run(command: crate::GitCommand, git_dir: &Path) -> Result<Progress, GitError> {
    crate::operation::sequenced(command, git_dir).await
}

/// Entry point for the editor helper: if git started this process as an editor (first
/// argument [`SEQUENCE_FLAG`] or [`MESSAGE_FLAG`], file name last), do the job and return
/// the exit code. `None` means "not the helper".
pub fn editor_main(args: &[OsString]) -> Option<i32> {
    let flag = args.first()?.to_str()?;
    if flag != SEQUENCE_FLAG && flag != MESSAGE_FLAG {
        return None;
    }
    let file = PathBuf::from(args.last()?);
    let git_dir = std::env::var_os(GIT_DIR_ENV).map(PathBuf::from)?;
    let result = if flag == SEQUENCE_FLAG {
        write_todo(&git_dir, &file)
    } else {
        write_message(&git_dir, &file)
    };
    Some(i32::from(result.is_err()))
}

/// Replace git's todo list with Facet's.
pub fn write_todo(git_dir: &Path, file: &Path) -> std::io::Result<()> {
    let todo = std::fs::read(state_dir(git_dir).join(TODO_FILE))?;
    std::fs::write(file, todo)
}

/// If the step being applied is a reword Facet has a message for, write that message;
/// otherwise keep the one git prepared.
pub fn write_message(git_dir: &Path, file: &Path) -> std::io::Result<()> {
    let Ok(done) = std::fs::read_to_string(git_dir.join("rebase-merge").join("done")) else {
        return Ok(());
    };
    let Some(last) = done
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with('#'))
    else {
        return Ok(());
    };
    let mut words = last.split_whitespace();
    let (Some(action), Some(oid)) = (words.next(), words.next()) else {
        return Ok(());
    };
    if action != "reword" && action != "r" {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(state_dir(git_dir)) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(kept) = name.strip_prefix("msg-") else {
            continue;
        };
        if kept.starts_with(oid) || oid.starts_with(kept) {
            let message = std::fs::read(entry.path())?;
            return std::fs::write(file, message);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(action: TodoAction, oid: &str) -> TodoItem {
        TodoItem {
            action,
            oid: oid.to_owned(),
            subject: format!("subject {oid}"),
            message: (action == TodoAction::Reword).then(|| "New message".to_owned()),
        }
    }

    #[test]
    fn todo_lists_are_validated() {
        let ok = todo_text(&[
            item(TodoAction::Drop, "a"),
            item(TodoAction::Reword, "b"),
            item(TodoAction::Fixup, "c"),
        ])
        .unwrap();
        assert_eq!(
            ok,
            "drop a subject a\nreword b subject b\nfixup c subject c\n"
        );
        assert!(todo_text(&[item(TodoAction::Drop, "a"), item(TodoAction::Squash, "b")]).is_err());
        assert!(todo_text(&[item(TodoAction::Drop, "a")]).is_err());
        let mut empty = item(TodoAction::Reword, "a");
        empty.message = Some("  ".to_owned());
        assert!(todo_text(&[empty]).is_err());
    }

    #[test]
    fn editors_quote_the_executable_for_sh() {
        let editors = Editors::facet(Path::new(r"C:\Program Files\it's\facet.exe"));
        assert_eq!(
            editors.sequence,
            format!("'C:/Program Files/it'\\''s/facet.exe' {SEQUENCE_FLAG}")
        );
    }

    #[test]
    fn message_helper_writes_only_for_rewords() {
        let git_dir = tempfile::tempdir().unwrap();
        let dir = git_dir.path();
        std::fs::create_dir_all(dir.join("rebase-merge")).unwrap();
        std::fs::create_dir_all(state_dir(dir)).unwrap();
        std::fs::write(state_dir(dir).join("msg-abcdef0123"), "Reworded").unwrap();
        let file = dir.join("COMMIT_EDITMSG");

        std::fs::write(&file, "original").unwrap();
        std::fs::write(
            dir.join("rebase-merge/done"),
            "pick 111 a\nreword abcdef0 b\n",
        )
        .unwrap();
        write_message(dir, &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Reworded");

        std::fs::write(&file, "combined").unwrap();
        std::fs::write(
            dir.join("rebase-merge/done"),
            "reword abcdef0 b\nsquash 222 c\n",
        )
        .unwrap();
        write_message(dir, &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "combined");
    }

    #[test]
    fn helper_ignores_other_arguments() {
        assert_eq!(editor_main(&[OsString::from("C:/repo")]), None);
        assert_eq!(editor_main(&[]), None);
    }
}
