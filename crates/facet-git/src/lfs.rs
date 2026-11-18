//! Git LFS: is it installed, which paths it handles, file locks, explicit transfers.
//!
//! Everything runs the user's own `git lfs` through [`Git`] (job objects, command log).
//! Lock commands talk to the LFS server, so the app runs them on the credential-enabled
//! `Git` (askpass / GCM, G7).

use std::path::Path;

use crate::parse::{
    parse_check_attr, parse_locks, parse_new_lock, parse_verified_locks, LfsAttributes, LfsLock,
};
use crate::remote::Network;
use crate::{Git, GitError};

/// `git lfs version`, verbatim (shown in settings, never parsed); `None` when git-lfs is
/// not installed.
pub async fn lfs_version(git: &Git) -> Option<String> {
    let output = git
        .command(["lfs", "version"])
        .background()
        .output()
        .await
        .ok()?;
    output
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// How a repository uses LFS, from files alone (no process, G5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LfsUsage {
    /// An LFS object store in the git directory, or an `lfs` filter in `.gitattributes`.
    pub lfs: bool,
    /// The top-level `.gitattributes` marks files `lockable`: the team uses file locking,
    /// so the lock list is worth asking the server for.
    pub locking: bool,
}

/// Whether the repository uses LFS and file locking. Blocking file reads: call it off the
/// UI thread.
pub fn lfs_usage(root: &Path, git_dir: &Path) -> LfsUsage {
    let attributes = std::fs::read_to_string(root.join(".gitattributes")).unwrap_or_default();
    let locking = attributes.contains("lockable");
    LfsUsage {
        lfs: locking || attributes.contains("filter=lfs") || git_dir.join("lfs").is_dir(),
        locking,
    }
}

/// LFS attributes of `paths` (repository-relative), in one `check-attr` process.
pub async fn lfs_attributes(
    git: &Git,
    root: &Path,
    paths: &[String],
) -> Result<Vec<(String, LfsAttributes)>, GitError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    let output = git
        .command(["check-attr", "-z", "--stdin", "filter", "lockable"])
        .current_dir(root)
        .background()
        .stdin(stdin)
        .run()
        .await?;
    parse_check_attr(&output.stdout)
}

/// All locks on the server. Ownership comes from `--verify` when the server supports it;
/// otherwise the plain list is returned with ownership unknown.
pub async fn lfs_locks(git: &Git, root: &Path) -> Result<Vec<LfsLock>, GitError> {
    let verified = git
        .command(["lfs", "locks", "--verify", "--json"])
        .current_dir(root)
        .output()
        .await?;
    if verified.success() {
        return parse_verified_locks(&verified.stdout);
    }
    let plain = git
        .command(["lfs", "locks", "--json"])
        .current_dir(root)
        .run()
        .await?;
    let mut locks = parse_locks(&plain.stdout)?;
    locks.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(locks)
}

/// Lock one file (repository-relative path).
pub async fn lfs_lock(git: &Git, root: &Path, path: &str) -> Result<LfsLock, GitError> {
    let output = git
        .command(["lfs", "lock", "--json", "--", path])
        .current_dir(root)
        .run()
        .await?;
    parse_new_lock(&output.stdout)
}

/// Release a lock by id. `force` breaks someone else's lock (the caller confirms first).
pub async fn lfs_unlock(git: &Git, root: &Path, id: &str, force: bool) -> Result<(), GitError> {
    let mut command = git.command(["lfs", "unlock", "--json", "--id", id]);
    if force {
        command = command.arg("--force");
    }
    command.current_dir(root).run().await.map(drop)
}

/// `git lfs pull`: download the LFS objects of the current checkout and replace pointer
/// files with their content (after a clone with `GIT_LFS_SKIP_SMUDGE`, or a failed smudge).
pub async fn lfs_pull(net: &Network<'_>) -> Result<(), GitError> {
    net.run(&["lfs", "pull"]).await
}

/// `git lfs track -- <pattern>`: store matching files in LFS from now on (edits
/// `.gitattributes`, which the user then commits).
pub async fn lfs_track(
    git: &Git,
    root: &Path,
    pattern: &str,
    lockable: bool,
) -> Result<(), GitError> {
    let mut command = git.command(["lfs", "track"]);
    if lockable {
        command = command.arg("--lockable");
    }
    command
        .arg("--")
        .arg(pattern)
        .current_dir(root)
        .run()
        .await
        .map(drop)
}
