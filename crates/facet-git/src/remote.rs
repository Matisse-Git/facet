//! Remotes and the network: add/remove remotes, fetch, pull, push.
//!
//! Network commands report progress (`--progress`, shown verbatim, never parsed), are
//! cancellable (the whole process tree, see `job.rs`) and get credentials from Git
//! Credential Manager or Facet's askpass bridge (`askpass.rs`). Push never force-pushes
//! blindly: only `--force-with-lease` against the commit the person saw.

use std::path::Path;

use crate::parse::Remote;
use crate::{CancellationToken, Git, GitError, ProgressFn};

pub async fn add_remote(git: &Git, root: &Path, name: &str, url: &str) -> Result<(), GitError> {
    git.command(["remote", "add", "--", name, url])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// Remove a remote (and its remote-tracking branches). Undo with [`restore_remote`], given
/// the [`Remote`] as it was listed.
pub async fn remove_remote(git: &Git, root: &Path, name: &str) -> Result<(), GitError> {
    git.command(["remote", "remove", "--", name])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

/// Put a removed remote back (its URLs; remote-tracking branches return with the next fetch).
pub async fn restore_remote(git: &Git, root: &Path, remote: &Remote) -> Result<(), GitError> {
    let url = remote
        .url
        .as_deref()
        .ok_or_else(|| GitError::Unsupported(format!("remote {} had no URL", remote.name)))?;
    add_remote(git, root, &remote.name, url).await?;
    if let Some(push_url) = &remote.push_url {
        git.command(["remote", "set-url", "--push", "--", &remote.name, push_url])
            .current_dir(root)
            .run()
            .await?;
    }
    Ok(())
}

/// Context shared by network commands.
#[derive(Clone)]
pub struct Network<'a> {
    pub git: &'a Git,
    pub root: &'a Path,
    pub cancel: &'a CancellationToken,
    pub progress: ProgressFn,
}

impl Network<'_> {
    /// Run a network command with live progress. git-lfs only prints its transfer progress
    /// to a terminal unless `GIT_LFS_FORCE_PROGRESS` is set; with it, LFS uploads and
    /// downloads show in the same progress line as git's own.
    pub(crate) async fn run(&self, args: &[&str]) -> Result<(), GitError> {
        self.git
            .with_env("GIT_LFS_FORCE_PROGRESS", "1")
            .command(args)
            .current_dir(self.root)
            .cancel_with(self.cancel)
            .progress(self.progress.clone())
            .run()
            .await
            .map(drop)
    }
}

/// Fetch one remote, or all of them with `None`. Pruning follows the repository's own
/// `fetch.prune` setting.
pub async fn fetch(net: &Network<'_>, remote: Option<&str>) -> Result<(), GitError> {
    match remote {
        Some(remote) => net.run(&["fetch", "--progress", "--", remote]).await,
        None => net.run(&["fetch", "--progress", "--all"]).await,
    }
}

/// Fetch every remote without asking anyone anything (the background fetch): no credential
/// window, no askpass prompt; a remote that needs a sign-in just fails. Only refs change,
/// never the working tree.
pub async fn fetch_in_background(git: &Git, root: &Path) -> Result<(), GitError> {
    git.with_env("GCM_INTERACTIVE", "never")
        .with_env("GIT_TERMINAL_PROMPT", "0")
        .with_env("GIT_ASKPASS", "echo")
        .with_env("SSH_ASKPASS", "echo")
        .with_env("SSH_ASKPASS_REQUIRE", "never")
        .command([
            "-c",
            "credential.interactive=false",
            "fetch",
            "--all",
            "--quiet",
        ])
        .current_dir(root)
        .background()
        .run()
        .await
        .map(drop)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// Merge the upstream in (a merge commit when the histories have diverged).
    Merge,
    /// Only fast-forward; never creates a merge commit.
    FastForwardOnly,
    /// Rebase local commits onto the upstream.
    Rebase,
}

/// Pull the current branch from its upstream.
pub async fn pull(net: &Network<'_>, mode: PullMode) -> Result<(), GitError> {
    let flag = match mode {
        PullMode::Merge => "--no-rebase",
        PullMode::FastForwardOnly => "--ff-only",
        PullMode::Rebase => "--rebase",
    };
    net.run(&["pull", "--progress", flag]).await
}

/// What to push where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    pub remote: String,
    /// Local branch name.
    pub branch: String,
    /// Branch name on the remote (the upstream's, or the same name when publishing).
    pub remote_branch: String,
    /// Make the pushed branch the upstream (publishing a new branch).
    pub set_upstream: bool,
    /// Overwrite the remote branch, but only if it still is at this commit (the one the
    /// person saw): `--force-with-lease=<ref>:<oid>`. `None`: a normal push.
    pub force_with_lease: Option<String>,
}

pub async fn push(net: &Network<'_>, request: &PushRequest) -> Result<(), GitError> {
    let mut args: Vec<String> = vec!["push".into(), "--progress".into()];
    if request.set_upstream {
        args.push("--set-upstream".into());
    }
    if let Some(expected) = &request.force_with_lease {
        args.push(format!(
            "--force-with-lease=refs/heads/{}:{expected}",
            request.remote_branch
        ));
    }
    args.push("--".into());
    args.push(request.remote.clone());
    args.push(format!(
        "refs/heads/{}:refs/heads/{}",
        request.branch, request.remote_branch
    ));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    net.run(&args).await
}

/// Push one tag.
pub async fn push_tag(net: &Network<'_>, remote: &str, tag: &str) -> Result<(), GitError> {
    let refspec = format!("refs/tags/{tag}:refs/tags/{tag}");
    net.run(&["push", "--progress", "--", remote, &refspec])
        .await
}
