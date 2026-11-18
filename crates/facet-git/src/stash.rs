//! Stashes: push (everything, selected files, or only what is staged), apply, pop, drop
//! (with undo).

use std::path::Path;

use crate::parse::StashEntry;
use crate::{Git, GitError};

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
