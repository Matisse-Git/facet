use std::path::{Path, PathBuf};

use crate::parse::{
    self, parse_commit_summary, parse_version, CommitSummary, GitVersion, COMMIT_SUMMARY_FORMAT,
};
use crate::{Git, GitError};

/// Where a repository lives on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Absolute path of the `.git` directory (or the repository itself when bare).
    pub git_dir: PathBuf,
    /// Root of the working tree; `None` for a bare repository.
    pub workdir: Option<PathBuf>,
}

impl RepoInfo {
    pub fn is_bare(&self) -> bool {
        self.workdir.is_none()
    }

    /// The directory git commands for this repository should run in.
    pub fn root(&self) -> &Path {
        self.workdir.as_deref().unwrap_or(&self.git_dir)
    }

    /// Short display name: the folder name of the working tree (or of the bare repo).
    pub fn name(&self) -> String {
        self.root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root().display().to_string())
    }
}

/// What `HEAD` currently points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// On a branch. `commit` is `None` while the branch is unborn (fresh `git init`).
    Branch {
        name: String,
        commit: Option<CommitSummary>,
    },
    /// Detached at a commit.
    Detached { commit: CommitSummary },
}

impl Head {
    pub fn commit(&self) -> Option<&CommitSummary> {
        match self {
            Self::Branch { commit, .. } => commit.as_ref(),
            Self::Detached { commit } => Some(commit),
        }
    }
}

/// Ask the git executable for its version.
pub async fn version(git: &Git) -> Result<GitVersion, GitError> {
    let output = git.command(["version"]).run().await?;
    parse_version(&output.stdout)
}

/// Find the repository containing `path`.
///
/// `path` may be the working tree root, any directory inside it, the `.git` directory itself, or
/// a bare repository.
pub async fn discover(git: &Git, path: &Path) -> Result<RepoInfo, GitError> {
    const COMMAND: &str = "rev-parse";

    // Spawning git in a missing directory fails with an OS error that reads as if git itself
    // were broken; report the real problem instead.
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.is_dir() => {}
        _ => {
            return Err(GitError::PathNotFound {
                path: path.to_owned(),
            })
        }
    }

    let output = git
        .command([
            "rev-parse",
            "--is-bare-repository",
            "--is-inside-work-tree",
            "--absolute-git-dir",
        ])
        .current_dir(path)
        .background()
        .output()
        .await?;
    if !output.success() {
        return Err(GitError::NotARepository {
            path: path.to_owned(),
            detail: output.stderr_text(),
        });
    }

    let text = parse::utf8(COMMAND, &output.stdout)?;
    let mut lines = text.lines();
    let mut next = |what: &str| {
        lines
            .next()
            .ok_or_else(|| GitError::parse(COMMAND, format!("missing `{what}` line")))
    };
    let is_bare = parse_bool(COMMAND, next("is-bare-repository")?)?;
    let inside_work_tree = parse_bool(COMMAND, next("is-inside-work-tree")?)?;
    let git_dir = native_path(next("absolute-git-dir")?);

    if is_bare {
        return Ok(RepoInfo {
            git_dir,
            workdir: None,
        });
    }

    // Inside the working tree git can tell us its root. Inside `.git` it cannot, so resolve from
    // the directory that contains `.git` instead.
    let ask_from = if inside_work_tree {
        path.to_owned()
    } else {
        match git_dir.parent() {
            Some(parent) => parent.to_owned(),
            None => {
                return Err(GitError::NotARepository {
                    path: path.to_owned(),
                    detail: "no working tree found for this git directory".to_owned(),
                })
            }
        }
    };
    let output = git
        .command(["rev-parse", "--show-toplevel"])
        .current_dir(&ask_from)
        .background()
        .output()
        .await?;
    if !output.success() {
        return Err(GitError::NotARepository {
            path: path.to_owned(),
            detail: output.stderr_text(),
        });
    }
    let toplevel = parse::utf8(COMMAND, &output.stdout)?
        .lines()
        .next()
        .ok_or_else(|| GitError::parse(COMMAND, "missing `show-toplevel` line"))?;

    Ok(RepoInfo {
        git_dir,
        workdir: Some(native_path(toplevel)),
    })
}

/// Read what `HEAD` points at, without touching the index or working tree (cheap on huge repos).
pub async fn head(git: &Git, repo_root: &Path) -> Result<Head, GitError> {
    // `symbolic-ref -q` exits 1 (silently) when HEAD is detached.
    let branch = git
        .command(["symbolic-ref", "-q", "HEAD"])
        .current_dir(repo_root)
        .background()
        .output();
    // `rev-parse -q --verify` exits 1 (silently) when the branch is unborn.
    let resolved = git
        .command(["rev-parse", "-q", "--verify", "HEAD^{commit}"])
        .current_dir(repo_root)
        .background()
        .output();
    // Fails on an unborn branch; that case is decided by `resolved` above, not by this exit code.
    let summary = git
        .command([
            "log",
            "-1",
            "-z",
            // A user's `log.showSignature=true` would prepend GPG output to the record.
            "--no-show-signature",
            COMMIT_SUMMARY_FORMAT,
            "HEAD",
        ])
        .current_dir(repo_root)
        .background()
        .output();

    // All three run concurrently: process start-up dominates on Windows (tens of ms each), so
    // one round of three is about three times faster than three rounds of one.
    let (branch, resolved, summary) = tokio::join!(branch, resolved, summary);
    let (branch, resolved, summary) = (branch?, resolved?, summary?);

    let branch_name = if branch.success() {
        let full = parse::utf8("symbolic-ref", &branch.stdout)?.trim_end_matches(['\r', '\n']);
        Some(full.strip_prefix("refs/heads/").unwrap_or(full).to_owned())
    } else {
        None
    };

    let commit = if resolved.success() {
        if !summary.success() {
            return Err(GitError::Failed {
                command: "git log -1".to_owned(),
                code: summary.status,
                stderr: summary.stderr_text(),
            });
        }
        Some(parse_commit_summary(&summary.stdout)?)
    } else {
        None
    };

    match (branch_name, commit) {
        (Some(name), commit) => Ok(Head::Branch { name, commit }),
        (None, Some(commit)) => Ok(Head::Detached { commit }),
        (None, None) => Err(GitError::parse(
            "rev-parse",
            "HEAD is neither a branch nor a commit",
        )),
    }
}

fn parse_bool(command: &'static str, value: &str) -> Result<bool, GitError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(GitError::parse(
            command,
            format!("expected true/false, got `{other}`"),
        )),
    }
}

/// Git prints paths with forward slashes on Windows; convert to the platform's separators.
fn native_path(path: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_is_the_folder_name() {
        let repo = RepoInfo {
            git_dir: PathBuf::from("/work/game/.git"),
            workdir: Some(PathBuf::from("/work/game")),
        };
        assert_eq!(repo.name(), "game");
        assert!(!repo.is_bare());
        assert_eq!(repo.root(), Path::new("/work/game"));

        let bare = RepoInfo {
            git_dir: PathBuf::from("/srv/game.git"),
            workdir: None,
        };
        assert_eq!(bare.name(), "game.git");
        assert!(bare.is_bare());
    }

    #[test]
    fn bools_are_strict() {
        assert!(parse_bool("x", "true").unwrap());
        assert!(!parse_bool("x", "false").unwrap());
        assert!(parse_bool("x", "yes").is_err());
    }
}
