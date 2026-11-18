//! Linked worktrees: list, add, remove, prune.
//!
//! Removing never forces: a worktree with local changes or untracked files is refused by git
//! and the message says so, so nothing unsaved can be lost from Facet.

use std::path::Path;

use crate::parse::{parse_worktrees, Worktree};
use crate::{Git, GitError};

/// Every worktree of the repository, the main one first.
pub async fn worktrees(git: &Git, root: &Path) -> Result<Vec<Worktree>, GitError> {
    let output = git
        .command(["worktree", "list", "--porcelain", "-z"])
        .current_dir(root)
        .background()
        .run()
        .await?;
    parse_worktrees(&output.stdout)
}

/// What a new worktree checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeTarget {
    /// An existing local branch (git refuses one checked out elsewhere).
    Branch(String),
    /// A new branch created at `start` (a commit-ish; `None` = HEAD).
    NewBranch { name: String, start: Option<String> },
    /// A detached HEAD at a commit-ish.
    Detached(String),
}

/// `git worktree add`. The post-checkout hook runs as usual (G8).
pub async fn add_worktree(
    git: &Git,
    root: &Path,
    path: &Path,
    target: &WorktreeTarget,
) -> Result<(), GitError> {
    let mut command = git.command(["worktree", "add"]);
    command = match target {
        WorktreeTarget::Branch(branch) => command.arg("--").arg(path).arg(branch),
        WorktreeTarget::NewBranch { name, start } => {
            let command = command.arg("-b").arg(name).arg("--").arg(path);
            match start {
                Some(start) => command.arg(start),
                None => command,
            }
        }
        WorktreeTarget::Detached(commit) => command.arg("--detach").arg("--").arg(path).arg(commit),
    };
    command.current_dir(root).run().await.map(drop)
}

/// `git worktree remove` without `--force`: refused while the worktree has changes.
pub async fn remove_worktree(git: &Git, root: &Path, path: &str) -> Result<(), GitError> {
    let output = git
        .command(["worktree", "remove", "--", path])
        .current_dir(root)
        .output()
        .await?;
    if output.success() {
        return Ok(());
    }
    let stderr = output.stderr_text();
    if stderr.contains("modified or untracked files") || stderr.contains("--force") {
        return Err(GitError::Unsupported(format!(
            "The worktree at {path} has uncommitted changes or untracked files. Commit, stash \
             or delete them there first; Facet never removes a worktree with unsaved work."
        )));
    }
    Err(GitError::Failed {
        command: "git worktree remove".to_owned(),
        code: output.status,
        stderr,
    })
}

/// `git worktree prune`: forget worktrees whose directories are gone.
pub async fn prune_worktrees(git: &Git, root: &Path) -> Result<(), GitError> {
    git.command(["worktree", "prune"])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// The worktree (other than `current`) that has `branch` (short name) checked out.
pub fn checked_out_elsewhere<'a>(
    worktrees: &'a [Worktree],
    current: &Path,
    branch: &str,
) -> Option<&'a Worktree> {
    worktrees.iter().find(|worktree| {
        worktree.branch_name() == Some(branch) && !same_dir(Path::new(&worktree.path), current)
    })
}

/// Paths from git (forward slashes) and from Windows (backslashes, any case) compare equal
/// when they name the same directory.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    norm(a) == norm(b)
}
