use std::path::PathBuf;

/// Everything that can go wrong while talking to git.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The git executable could not be started at all (not installed, bad path, …).
    #[error("could not start git ({exe}): {source}")]
    Spawn {
        exe: String,
        #[source]
        source: std::io::Error,
    },

    /// Git ran and exited with a non-zero status. `stderr` is git's own message, verbatim.
    #[error("{command} failed (exit {code}): {stderr}")]
    Failed {
        command: String,
        code: i32,
        stderr: String,
    },

    /// The path does not exist or is not a directory, so git was never asked about it.
    #[error("folder not found: {}", path.display())]
    PathNotFound { path: PathBuf },

    /// The path is not inside a git repository (or git refused to treat it as one).
    /// `detail` is git's own message, verbatim, for display.
    #[error("not a git repository: {}", path.display())]
    NotARepository { path: PathBuf, detail: String },

    /// The operation was cancelled through its [`CancellationToken`](crate::CancellationToken).
    #[error("cancelled")]
    Cancelled,

    #[error("i/o error while talking to git: {0}")]
    Io(#[from] std::io::Error),

    /// Git produced output that does not match the documented plumbing format.
    #[error("unexpected output from `git {command}`: {detail}")]
    Parse {
        command: &'static str,
        detail: String,
    },
}

impl GitError {
    pub(crate) fn parse(command: &'static str, detail: impl Into<String>) -> Self {
        Self::Parse {
            command,
            detail: detail.into(),
        }
    }
}
