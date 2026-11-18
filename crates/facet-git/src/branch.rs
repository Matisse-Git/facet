//! Branches and tags: create, rename, delete (with undo), switch, upstream tracking.
//!
//! Names are always passed after the options they belong to and validated by git itself
//! (`check-ref-format` rules); nothing here builds refs by string concatenation that git
//! wouldn't accept.

use std::path::Path;

use crate::{Git, GitError};

/// Is `name` acceptable as a new branch name? Asks git, so the rules are git's own.
pub async fn valid_branch_name(git: &Git, root: &Path, name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && git
            .command(["check-ref-format", "--branch", name])
            .current_dir(root)
            .background()
            .output()
            .await
            .is_ok_and(|output| output.success())
}

/// Create branch `name` at `start` (a commit, branch or tag); with `switch`, check it out.
/// Starting from a remote-tracking branch sets it as upstream (git's `branch.autoSetupMerge`).
pub async fn create_branch(
    git: &Git,
    root: &Path,
    name: &str,
    start: &str,
    switch: bool,
) -> Result<(), GitError> {
    let command = if switch {
        git.command(["switch", "-c", name, start])
    } else {
        git.command(["branch", "--", name, start])
    };
    command.current_dir(root).run().await.map(drop)
}

pub async fn rename_branch(git: &Git, root: &Path, old: &str, new: &str) -> Result<(), GitError> {
    git.command(["branch", "-m", "--", old, new])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// What [`delete_branch`] removed, enough to put it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedBranch {
    pub name: String,
    pub oid: String,
    pub upstream: Option<String>,
}

/// Delete a local branch, merged or not (the caller has asked for confirmation and keeps
/// the returned record for undo). Its commit is read first so undo can recreate it.
pub async fn delete_branch(
    git: &Git,
    root: &Path,
    name: &str,
    upstream: Option<String>,
) -> Result<DeletedBranch, GitError> {
    let full = format!("refs/heads/{name}");
    let output = git
        .command(["rev-parse", "--verify", "-q"])
        .arg(format!("{full}^{{commit}}"))
        .current_dir(root)
        .run()
        .await?;
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    git.command(["branch", "-D", "--", name])
        .current_dir(root)
        .run()
        .await?;
    Ok(DeletedBranch {
        name: name.to_owned(),
        oid,
        upstream,
    })
}

/// Undo [`delete_branch`]: the branch comes back at the same commit, with its upstream.
pub async fn restore_branch(
    git: &Git,
    root: &Path,
    deleted: &DeletedBranch,
) -> Result<(), GitError> {
    git.command(["branch", "--", &deleted.name, &deleted.oid])
        .current_dir(root)
        .run()
        .await?;
    if let Some(upstream) = &deleted.upstream {
        // The upstream may be gone by now; the branch itself is what matters.
        let _ = set_upstream(git, root, &deleted.name, Some(upstream)).await;
    }
    Ok(())
}

/// Set (or with `None`, remove) the upstream of a local branch.
pub async fn set_upstream(
    git: &Git,
    root: &Path,
    branch: &str,
    upstream: Option<&str>,
) -> Result<(), GitError> {
    let command = match upstream {
        Some(upstream) => git
            .command(["branch"])
            .arg(format!("--set-upstream-to={upstream}"))
            .args(["--", branch]),
        None => git.command(["branch", "--unset-upstream", "--", branch]),
    };
    command.current_dir(root).run().await.map(drop)
}

/// What to check out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchTarget {
    /// A local branch.
    Branch(String),
    /// A remote-tracking branch (`origin/feature`): creates the local branch `local`
    /// tracking it.
    Track {
        remote_branch: String,
        local: String,
    },
    /// Any commit, detached.
    Detached(String),
}

/// Switch the working tree. Git refuses (and changes nothing) when local changes would be
/// overwritten; uncommitted changes that don't conflict are carried over, as in git.
pub async fn switch(git: &Git, root: &Path, target: &SwitchTarget) -> Result<(), GitError> {
    let command = match target {
        SwitchTarget::Branch(name) => git.command(["switch", "--no-guess", name]),
        SwitchTarget::Track {
            remote_branch,
            local,
        } => git.command(["switch", "-c", local, "--track", remote_branch]),
        SwitchTarget::Detached(oid) => git.command(["switch", "--detach", oid]),
    };
    command.current_dir(root).run().await.map(drop)
}

// ----- tags -----------------------------------------------------------------------------------

/// Is `name` acceptable as a tag name?
pub async fn valid_tag_name(git: &Git, root: &Path, name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && git
            .command(["check-ref-format"])
            .arg(format!("refs/tags/{name}"))
            .current_dir(root)
            .background()
            .output()
            .await
            .is_ok_and(|output| output.success())
}

/// Create a tag at `target`: annotated when there is a message (sent over stdin), else
/// lightweight.
pub async fn create_tag(
    git: &Git,
    root: &Path,
    name: &str,
    target: &str,
    message: Option<&str>,
) -> Result<(), GitError> {
    let command = match message.filter(|m| !m.trim().is_empty()) {
        Some(message) => git
            .command(["tag", "-a", "--file=-", "--", name, target])
            .stdin(message),
        None => git.command(["tag", "--", name, target]),
    };
    command.current_dir(root).run().await.map(drop)
}

/// What [`delete_tag`] removed: the object the ref pointed at (the tag object of an
/// annotated tag), which still exists after the ref is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedTag {
    pub name: String,
    pub object: String,
}

pub async fn delete_tag(git: &Git, root: &Path, name: &str) -> Result<DeletedTag, GitError> {
    let full = format!("refs/tags/{name}");
    let output = git
        .command(["rev-parse", "--verify", "-q", &full])
        .current_dir(root)
        .run()
        .await?;
    let object = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    git.command(["tag", "-d", "--", name])
        .current_dir(root)
        .run()
        .await?;
    Ok(DeletedTag {
        name: name.to_owned(),
        object,
    })
}

/// Undo [`delete_tag`]: recreate the ref exactly (annotated tags stay annotated). Fails if a
/// tag of that name was created in the meantime.
pub async fn restore_tag(git: &Git, root: &Path, deleted: &DeletedTag) -> Result<(), GitError> {
    git.command(["update-ref"])
        .arg(format!("refs/tags/{}", deleted.name))
        .arg(&deleted.object)
        // An empty old value: the ref must not exist yet.
        .arg("")
        .current_dir(root)
        .run()
        .await
        .map(drop)
}
