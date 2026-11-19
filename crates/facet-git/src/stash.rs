//! Stashes: push (everything, selected files, or only what is staged), apply, pop, drop
//! (with undo), and the files a stash holds.

use std::path::Path;

use crate::parse::{parse_changed_files, ChangedFile, StashEntry};
use crate::{CancellationToken, Git, GitError};

/// What [`stash_push`] puts away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashOptions {
    /// Shown in the stash list; git writes "WIP on <branch>" when empty.
    pub message: String,
    /// Stash untracked files too.
    pub include_untracked: bool,
    /// Only what is staged (`--staged`); the working tree keeps everything else.
    pub staged_only: bool,
    /// Only these paths (a partial stash); empty means everything.
    pub paths: Vec<String>,
}

pub async fn stash_push(git: &Git, root: &Path, options: &StashOptions) -> Result<(), GitError> {
    // Paths are literal (`[a]*.txt` is a file, not a pattern) through per-path `:(literal)`
    // magic. The global `--literal-pathspecs` flag must not be used: `stash push` runs git
    // internally with pathspec magic of its own, and with the flag it leaves untracked files
    // behind after stashing them.
    let mut command = git.command(["stash", "push"]);
    if !options.message.trim().is_empty() {
        command = command.arg("--message").arg(options.message.trim());
    }
    if options.include_untracked {
        command = command.arg("--include-untracked");
    }
    if options.staged_only {
        command = command.arg("--staged");
    }
    if !options.paths.is_empty() {
        command = command
            .arg("--")
            .args(options.paths.iter().map(|path| format!(":(literal){path}")));
    }
    command.current_dir(root).run().await.map(drop)
}

/// Make sure `entry` is still at its position in the list (another tool may have pushed or
/// dropped a stash since it was listed) before acting on its selector.
async fn verify(git: &Git, root: &Path, entry: &StashEntry) -> Result<(), GitError> {
    let output = git
        .command(["rev-parse", "-q", "--verify", &entry.selector])
        .current_dir(root)
        .output()
        .await?;
    let current = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.success() && current == entry.oid {
        Ok(())
    } else {
        Err(GitError::Unsupported(format!(
            "The stash list changed since it was shown ({} moved). Refresh and try again.",
            entry.selector
        )))
    }
}

/// Apply a stash, keeping it (`pop == false`) or removing it once applied cleanly.
pub async fn stash_apply(
    git: &Git,
    root: &Path,
    entry: &StashEntry,
    pop: bool,
) -> Result<(), GitError> {
    if pop {
        verify(git, root, entry).await?;
        git.command(["stash", "pop", &entry.selector])
    } else {
        // Applying by commit id works whatever happened to the list.
        git.command(["stash", "apply", &entry.oid])
    }
    .current_dir(root)
    .run()
    .await
    .map(drop)
}

/// Drop a stash. The commit stays in the object database for a while, so
/// [`restore_stash`] can put it back.
pub async fn stash_drop(git: &Git, root: &Path, entry: &StashEntry) -> Result<(), GitError> {
    verify(git, root, entry).await?;
    git.command(["stash", "drop", &entry.selector])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// Undo [`stash_drop`]: store the commit again, at the top of the list.
pub async fn restore_stash(git: &Git, root: &Path, entry: &StashEntry) -> Result<(), GitError> {
    git.command(["stash", "store", "--message", &entry.message, &entry.oid])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// One file a stash holds, and the two commits its diff is read between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashFile {
    pub file: ChangedFile,
    /// Stashed while untracked (it comes from the stash's third parent).
    pub untracked: bool,
    /// The older side of the diff; `None` is the empty tree (untracked files).
    pub old: Option<String>,
    pub new: String,
}

/// The files a stash holds: its tracked changes against the commit it was made on (staged
/// and unstaged together, as they come back when it is applied), then its untracked files.
pub async fn stash_files(
    git: &Git,
    root: &Path,
    entry: &StashEntry,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<StashFile>, GitError> {
    let diff_tree = || {
        let command = git
            .command([
                "diff-tree",
                "-r",
                "-z",
                "-M",
                "--raw",
                "--numstat",
                "--no-commit-id",
            ])
            .current_dir(root)
            .background();
        match cancel {
            Some(cancel) => command.cancel_with(cancel),
            None => command,
        }
    };
    let tracked = match &entry.base {
        Some(base) => diff_tree().args([base.as_str(), entry.oid.as_str()]),
        None => diff_tree().args(["--root", entry.oid.as_str()]),
    };
    let untracked = async {
        if !entry.untracked {
            return Ok(None);
        }
        let mut parent = git
            .command(["rev-parse", "--verify", "-q"])
            .arg(format!("{}^3^{{commit}}", entry.oid))
            .current_dir(root)
            .background();
        if let Some(cancel) = cancel {
            parent = parent.cancel_with(cancel);
        }
        let oid = String::from_utf8_lossy(&parent.run().await?.stdout)
            .trim()
            .to_owned();
        let output = diff_tree().args(["--root", oid.as_str()]).run().await?;
        Ok::<_, GitError>(Some((oid, parse_changed_files(&output.stdout)?)))
    };
    let (tracked, untracked) = tokio::join!(tracked.run(), untracked);
    let mut files: Vec<StashFile> = parse_changed_files(&tracked?.stdout)?
        .into_iter()
        .map(|file| StashFile {
            file,
            untracked: false,
            old: entry.base.clone(),
            new: entry.oid.clone(),
        })
        .collect();
    if let Some((oid, untracked)) = untracked? {
        files.extend(untracked.into_iter().map(|file| StashFile {
            file,
            untracked: true,
            old: None,
            new: oid.clone(),
        }));
    }
    Ok(files)
}
