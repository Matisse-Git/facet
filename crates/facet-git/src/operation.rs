//! Operations that rewrite history or stop half-way: the in-progress state git keeps in the
//! git directory (rebase, merge, cherry-pick, revert, am, bisect) with continue / skip /
//! abort; cherry-pick, revert and reset; auto-stash; and reflog-backed undo.

use std::path::Path;

use crate::parse::{parse_reflog, ReflogEntry, REFLOG_FORMAT};
use crate::rebase::Editors;
use crate::{Git, GitCommand, GitError, RepoInfo};

/// What is in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationKind {
    Rebase,
    Merge,
    CherryPick,
    Revert,
    /// `git am` (applying a mailbox).
    Apply,
    Bisect,
}

impl OperationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rebase => "Rebase",
            Self::Merge => "Merge",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Apply => "Apply patches",
            Self::Bisect => "Bisect",
        }
    }

    /// Whether git can skip the current step.
    pub fn can_skip(self) -> bool {
        matches!(
            self,
            Self::Rebase | Self::CherryPick | Self::Revert | Self::Apply
        )
    }

    /// Whether git can continue (bisect is driven differently).
    pub fn can_continue(self) -> bool {
        self != Self::Bisect
    }
}

/// An operation that stopped half-way, as git recorded it in the git directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub kind: OperationKind,
    /// Step being applied and the total, when git counts them (rebase, am).
    pub step: Option<(u32, u32)>,
    /// The branch being rebased (`main`), when one is.
    pub branch: Option<String>,
    /// The commit being applied (`CHERRY_PICK_HEAD`, `MERGE_HEAD`, the rebase's stopped
    /// commit), when known.
    pub commit: Option<String>,
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn number(path: &Path) -> Option<u32> {
    read(path)?.parse().ok()
}

/// The operation in progress in `git_dir` (the per-worktree git directory), if any. Reads a
/// few small files; call it off the UI thread.
pub fn operation(git_dir: &Path) -> Option<Operation> {
    let branch = |file: &Path| {
        read(file).map(|name| {
            name.strip_prefix("refs/heads/")
                .map_or(name.clone(), str::to_owned)
        })
    };
    let merge = git_dir.join("rebase-merge");
    if merge.is_dir() {
        let step = number(&merge.join("msgnum")).zip(number(&merge.join("end")));
        return Some(Operation {
            kind: OperationKind::Rebase,
            step,
            branch: branch(&merge.join("head-name")).filter(|name| name != "detached HEAD"),
            commit: read(&merge.join("stopped-sha")),
        });
    }
    let apply = git_dir.join("rebase-apply");
    if apply.is_dir() {
        let step = number(&apply.join("next")).zip(number(&apply.join("last")));
        let am = apply.join("applying").exists();
        return Some(Operation {
            kind: if am {
                OperationKind::Apply
            } else {
                OperationKind::Rebase
            },
            step,
            branch: branch(&apply.join("head-name")).filter(|name| name != "detached HEAD"),
            commit: read(&apply.join("original-commit")),
        });
    }
    let simple = |kind, commit: Option<String>| Operation {
        kind,
        step: None,
        branch: None,
        commit,
    };
    if let Some(commit) = read(&git_dir.join("MERGE_HEAD")) {
        let first = commit.lines().next().map(str::to_owned);
        return Some(simple(OperationKind::Merge, first));
    }
    if let Some(commit) = read(&git_dir.join("CHERRY_PICK_HEAD")) {
        return Some(simple(OperationKind::CherryPick, Some(commit)));
    }
    if let Some(commit) = read(&git_dir.join("REVERT_HEAD")) {
        return Some(simple(OperationKind::Revert, Some(commit)));
    }
    // A multi-commit pick whose current step was committed by hand: only the sequencer's
    // todo is left, and `--continue` goes on with the rest.
    if let Some(todo) = read(&git_dir.join("sequencer").join("todo")) {
        let kind = if todo.starts_with("revert") {
            OperationKind::Revert
        } else {
            OperationKind::CherryPick
        };
        return Some(simple(kind, None));
    }
    if git_dir.join("BISECT_LOG").exists() {
        return Some(simple(OperationKind::Bisect, None));
    }
    None
}

/// How a history-rewriting command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Finished,
    /// Stopped half-way — conflicts, or an `edit` step — and `operation` is in progress.
    /// `message` is the first lines of what git said.
    Stopped {
        operation: Operation,
        message: String,
    },
}

/// Run a command that may stop half-way, and tell finishing, stopping and failing apart.
pub(crate) async fn sequenced(command: GitCommand, git_dir: &Path) -> Result<Progress, GitError> {
    let line = command.command_line();
    let output = command.output().await?;
    match operation(git_dir) {
        Some(operation) if operation.kind != OperationKind::Bisect => Ok(Progress::Stopped {
            operation,
            message: summary(&output.stderr, &output.stdout),
        }),
        _ if output.success() => Ok(Progress::Finished),
        _ => Err(GitError::Failed {
            command: line,
            code: output.status,
            stderr: output.stderr_text(),
        }),
    }
}

/// The lines worth showing from git's report of a stop: conflicts and errors, not hints.
fn summary(stderr: &[u8], stdout: &[u8]) -> String {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("CONFLICT")
                || line.starts_with("error:")
                || line.starts_with("Stopped at")
                || line.starts_with("fatal:")
        })
        .take(4)
        .collect();
    lines.join("\n")
}

/// `git <op> --continue`. The commit message git prepared is used as it is (the editor is
/// Facet's helper, which leaves it alone unless a reword asked for a new one).
pub async fn continue_operation(
    git: &Git,
    info: &RepoInfo,
    operation: &Operation,
    editors: &Editors,
) -> Result<Progress, GitError> {
    let verb = verb(operation.kind)?;
    let git = editors.apply(git, &info.git_dir);
    let progress = sequenced(
        git.command([verb, "--continue"]).current_dir(info.root()),
        &info.git_dir,
    )
    .await?;
    finish_rebase(info, &progress);
    Ok(progress)
}

/// Skip the step that stopped (rebase, cherry-pick, revert, am).
pub async fn skip_operation(
    git: &Git,
    info: &RepoInfo,
    operation: &Operation,
    editors: &Editors,
) -> Result<Progress, GitError> {
    if !operation.kind.can_skip() {
        return Err(GitError::Unsupported(format!(
            "A {} can't skip a step.",
            operation.kind.label().to_lowercase()
        )));
    }
    let git = editors.apply(git, &info.git_dir);
    let progress = sequenced(
        git.command([verb(operation.kind)?, "--skip"])
            .current_dir(info.root()),
        &info.git_dir,
    )
    .await?;
    finish_rebase(info, &progress);
    Ok(progress)
}

/// Abandon the operation and go back to where it started.
pub async fn abort_operation(
    git: &Git,
    info: &RepoInfo,
    operation: &Operation,
) -> Result<(), GitError> {
    let command = if operation.kind == OperationKind::Bisect {
        git.command(["bisect", "reset"])
    } else {
        git.command([verb(operation.kind)?, "--abort"])
    };
    command.current_dir(info.root()).run().await?;
    finish_rebase(info, &Progress::Finished);
    Ok(())
}

fn verb(kind: OperationKind) -> Result<&'static str, GitError> {
    Ok(match kind {
        OperationKind::Rebase => "rebase",
        OperationKind::Merge => "merge",
        OperationKind::CherryPick => "cherry-pick",
        OperationKind::Revert => "revert",
        OperationKind::Apply => "am",
        OperationKind::Bisect => {
            return Err(GitError::Unsupported(
                "A bisect is continued by marking commits good or bad.".to_owned(),
            ))
        }
    })
}

/// Once nothing is in progress any more, drop the messages kept for an interactive rebase.
fn finish_rebase(info: &RepoInfo, progress: &Progress) {
    if matches!(progress, Progress::Finished) {
        crate::rebase::clear_state(&info.git_dir);
    }
}

/// Where HEAD was before an operation, so it can be put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoPoint {
    /// HEAD before the operation.
    pub head: String,
    /// How to move back: `soft` keeps what a soft reset left staged, `keep` refuses to touch
    /// files with local changes.
    pub mode: ResetMode,
    /// Local changes auto-stashed before the operation; re-applied on undo.
    pub stash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResetMode {
    /// Move the branch only; the index and files keep the changes (staged).
    Soft,
    /// Move the branch and the index; files keep the changes (unstaged).
    Mixed,
    /// Move the branch, index and files; local changes are discarded.
    Hard,
    /// Move the branch, index and files, but refuse if local changes would be lost.
    Keep,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
            Self::Keep => "--keep",
        }
    }
}

async fn rev_parse(git: &Git, root: &Path, rev: &str) -> Result<Option<String>, GitError> {
    let output = git
        .command(["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")])
        .current_dir(root)
        .output()
        .await?;
    Ok(output
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

async fn current_head(git: &Git, root: &Path) -> Result<String, GitError> {
    rev_parse(git, root, "HEAD")
        .await?
        .ok_or_else(|| GitError::Unsupported("This branch has no commits yet.".to_owned()))
}

/// Stash tracked changes (staged and unstaged; untracked files stay where they are) under
/// `message`. Returns the stash commit, or `None` when there was nothing to stash.
pub async fn autostash(git: &Git, root: &Path, message: &str) -> Result<Option<String>, GitError> {
    let before = rev_parse(git, root, "refs/stash").await?;
    git.command(["stash", "push", "--message", message])
        .current_dir(root)
        .run()
        .await?;
    let after = rev_parse(git, root, "refs/stash").await?;
    Ok(after.filter(|after| before.as_ref() != Some(after)))
}

/// Re-apply an auto-stash (restaging what was staged) and drop it from the list. If it does
/// not apply cleanly it stays in the list and the error says so.
pub async fn restore_autostash(git: &Git, root: &Path, stash: &str) -> Result<(), GitError> {
    let applied = git
        .command(["stash", "apply", "--index", stash])
        .current_dir(root)
        .output()
        .await?;
    if !applied.success() {
        return Err(GitError::Unsupported(format!(
            "Your auto-stashed changes conflict with the result and were kept in the stash list \
             ({}). {}",
            &stash[..stash.len().min(7)],
            applied.stderr_text().lines().next().unwrap_or_default()
        )));
    }
    let list = git
        .command(["stash", "list", "--format=%H"])
        .current_dir(root)
        .run()
        .await?;
    let position = String::from_utf8_lossy(&list.stdout)
        .lines()
        .position(|oid| oid.trim() == stash);
    if let Some(index) = position {
        git.command(["stash", "drop", &format!("stash@{{{index}}}")])
            .current_dir(root)
            .run()
            .await?;
    }
    Ok(())
}

/// Copy commits onto HEAD, oldest first. A merge commit is picked relative to its first
/// parent. Local changes must be out of the way (see [`autostash`]).
pub async fn cherry_pick(
    git: &Git,
    info: &RepoInfo,
    commits: &[(String, bool)],
) -> Result<(Progress, UndoPoint), GitError> {
    pick_or_revert(git, info, "cherry-pick", commits).await
}

/// Make commits that undo the given ones, newest first.
pub async fn revert(
    git: &Git,
    info: &RepoInfo,
    commits: &[(String, bool)],
) -> Result<(Progress, UndoPoint), GitError> {
    pick_or_revert(git, info, "revert", commits).await
}

async fn pick_or_revert(
    git: &Git,
    info: &RepoInfo,
    verb: &str,
    commits: &[(String, bool)],
) -> Result<(Progress, UndoPoint), GitError> {
    if commits.is_empty() {
        return Err(GitError::Unsupported("No commits selected.".to_owned()));
    }
    let root = info.root();
    let head = current_head(git, root).await?;
    let mut command = git.command([verb]);
    if verb == "revert" {
        command = command.arg("--no-edit");
    }
    // `-m 1` applies to every commit of the run; git rejects it for ordinary commits, so a
    // merge goes alone.
    if commits.iter().any(|(_, merge)| *merge) {
        if commits.len() > 1 {
            return Err(GitError::Unsupported(
                "Pick a merge commit on its own.".to_owned(),
            ));
        }
        command = command.args(["-m", "1"]);
    }
    command = command
        .args(commits.iter().map(|(oid, _)| oid.as_str()))
        .current_dir(root);
    let progress = sequenced(command, &info.git_dir).await?;
    Ok((
        progress,
        UndoPoint {
            head,
            mode: ResetMode::Keep,
            stash: None,
        },
    ))
}

/// Merge `rev` (a branch or commit) into HEAD with git's own message. Fast-forwards when the
/// repository's `merge.ff` allows it; hooks run as usual (G8). Local changes must be out of
/// the way (see [`autostash`]). Conflicts stop it like any other operation.
pub async fn merge(
    git: &Git,
    info: &RepoInfo,
    rev: &str,
) -> Result<(Progress, UndoPoint), GitError> {
    let root = info.root();
    let head = current_head(git, root).await?;
    let command = git.command(["merge", "--no-edit", rev]).current_dir(root);
    let progress = sequenced(command, &info.git_dir).await?;
    Ok((
        progress,
        UndoPoint {
            head,
            mode: ResetMode::Keep,
            stash: None,
        },
    ))
}

/// Move the current branch (or detached HEAD) to `target`. A hard reset first auto-stashes
/// tracked changes, so even that can be undone.
pub async fn reset(
    git: &Git,
    root: &Path,
    target: &str,
    mode: ResetMode,
) -> Result<UndoPoint, GitError> {
    let head = current_head(git, root).await?;
    let stash = if mode == ResetMode::Hard {
        let short = &target[..target.len().min(7)];
        autostash(git, root, &format!("Facet: before reset --hard to {short}")).await?
    } else {
        None
    };
    git.command(["reset", mode.flag(), target, "--"])
        .current_dir(root)
        .run()
        .await?;
    let mode = match mode {
        ResetMode::Soft => ResetMode::Soft,
        ResetMode::Mixed => ResetMode::Mixed,
        ResetMode::Hard | ResetMode::Keep => ResetMode::Keep,
    };
    Ok(UndoPoint { head, mode, stash })
}

/// Put HEAD back where an [`UndoPoint`] recorded it, re-applying an auto-stash.
pub async fn undo_to(git: &Git, root: &Path, point: &UndoPoint) -> Result<(), GitError> {
    git.command(["reset", point.mode.flag(), &point.head, "--"])
        .current_dir(root)
        .run()
        .await?;
    if let Some(stash) = &point.stash {
        restore_autostash(git, root, stash).await?;
    }
    Ok(())
}

/// The newest `limit` entries of `refname`'s reflog (`HEAD` for the checkout history). A ref
/// without a reflog has an empty list.
pub async fn reflog(
    git: &Git,
    root: &Path,
    refname: &str,
    limit: usize,
) -> Result<Vec<ReflogEntry>, GitError> {
    let output = git
        .command([
            "log",
            "-g",
            "-z",
            "--date=unix",
            "--no-show-signature",
            REFLOG_FORMAT,
        ])
        .arg(format!("-n{limit}"))
        .args([refname, "--"])
        .current_dir(root)
        .background()
        .output()
        .await?;
    if !output.success() {
        // No reflog (fresh repository, or reflogs disabled): nothing to show.
        let stderr = output.stderr_text();
        if stderr.contains("does not have any commits")
            || stderr.contains("unknown revision")
            || stderr.contains("bad default revision")
            || stderr.contains("bad revision")
        {
            return Ok(Vec::new());
        }
        return Err(GitError::Failed {
            command: format!("git log -g {refname}"),
            code: output.status,
            stderr,
        });
    }
    parse_reflog(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_keeps_conflicts_and_errors() {
        let stderr = b"Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\n\
                       error: could not apply 1234567... Edit\nhint: Resolve all conflicts\n";
        assert_eq!(
            summary(stderr, b""),
            "CONFLICT (content): Merge conflict in a.txt\nerror: could not apply 1234567... Edit"
        );
    }

    #[test]
    fn state_files_are_recognised() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(operation(dir.path()), None);
        std::fs::write(dir.path().join("CHERRY_PICK_HEAD"), "abc\n").unwrap();
        let op = operation(dir.path()).unwrap();
        assert_eq!(op.kind, OperationKind::CherryPick);
        assert_eq!(op.commit.as_deref(), Some("abc"));

        let merge = dir.path().join("rebase-merge");
        std::fs::create_dir(&merge).unwrap();
        std::fs::write(merge.join("head-name"), "refs/heads/topic\n").unwrap();
        std::fs::write(merge.join("msgnum"), "2\n").unwrap();
        std::fs::write(merge.join("end"), "5\n").unwrap();
        let op = operation(dir.path()).unwrap();
        assert_eq!(op.kind, OperationKind::Rebase);
        assert_eq!(op.step, Some((2, 5)));
        assert_eq!(op.branch.as_deref(), Some("topic"));
    }
}
