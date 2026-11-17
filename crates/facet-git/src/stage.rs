//! Changing the index: staging and unstaging whole files, and applying partial patches.
//!
//! Path lists go to git on stdin (`--pathspec-from-file=- --pathspec-file-nul`), so thousands
//! of selected files never hit the Windows command-line length limit, and always with
//! `--literal-pathspecs` so file names are never read as glob patterns.

use std::path::Path;

use crate::{Git, GitError};

/// NUL-separated path list for `--pathspec-file-nul`.
pub(crate) fn nul_list<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<u8> {
    let mut list = Vec::new();
    for path in paths {
        list.extend_from_slice(path.as_bytes());
        list.push(0);
    }
    list
}

const FROM_STDIN: &[&str] = &["--pathspec-from-file=-", "--pathspec-file-nul"];

/// Stage the current working tree state of `paths`: modifications, new files and deletions.
pub async fn stage_paths(git: &Git, root: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    git.command(["--literal-pathspecs", "add", "-A"])
        .args(FROM_STDIN)
        .current_dir(root)
        .stdin(nul_list(paths.iter().map(String::as_str)))
        .run()
        .await?;
    Ok(())
}

/// Stage everything in the working tree.
pub async fn stage_all(git: &Git, root: &Path) -> Result<(), GitError> {
    git.command(["add", "-A"]).current_dir(root).run().await?;
    Ok(())
}

/// Remove the staged changes of `paths` from the index (the working tree is not touched).
/// For a staged rename pass both the old and the new path. Works on an unborn branch too.
pub async fn unstage_paths(git: &Git, root: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    git.command(["--literal-pathspecs", "reset", "-q"])
        .args(FROM_STDIN)
        .current_dir(root)
        .stdin(nul_list(paths.iter().map(String::as_str)))
        .run()
        .await?;
    Ok(())
}

/// Unstage everything (the working tree is not touched).
pub async fn unstage_all(git: &Git, root: &Path) -> Result<(), GitError> {
    git.command(["reset", "-q"]).current_dir(root).run().await?;
    Ok(())
}

/// Where a patch built by [`crate::build_patch`] is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTo {
    /// Stage: apply to the index.
    Index,
    /// Unstage: apply in reverse to the index.
    IndexReverse,
    /// Apply to the working tree file (undoing a discard).
    Worktree,
    /// Discard: apply in reverse to the working tree file.
    WorktreeReverse,
}

/// Apply a patch with `git apply`. Git verifies the whole patch before changing anything.
pub async fn apply_patch(
    git: &Git,
    root: &Path,
    patch: &[u8],
    target: ApplyTo,
) -> Result<(), GitError> {
    let flags: &[&str] = match target {
        ApplyTo::Index => &["--cached"],
        ApplyTo::IndexReverse => &["--cached", "--reverse"],
        ApplyTo::Worktree => &[],
        ApplyTo::WorktreeReverse => &["--reverse"],
    };
    git.command(["apply"])
        .args(flags)
        // Lines are staged exactly as they are; whitespace policy is not Facet's to enforce.
        .args(["--whitespace=nowarn", "-"])
        .current_dir(root)
        .stdin(patch)
        .run()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_lists_are_nul_separated() {
        assert_eq!(nul_list(["a b.txt", "c/d"]), b"a b.txt\0c/d\0");
        assert!(nul_list([]).is_empty());
    }
}
