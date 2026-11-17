//! Parsers for git plumbing output.
//!
//! Every parser here is a pure function over bytes and has golden tests (fixtures captured from
//! real git live in `tests/golden/`). Parsers never see human-readable porcelain.

mod commit;
mod version;

pub use commit::{parse_commit_summary, CommitSummary, COMMIT_SUMMARY_FORMAT};
pub use version::{parse_version, GitVersion};

use crate::GitError;

/// Decode git output as UTF-8, naming the command in the error.
pub(crate) fn utf8<'a>(command: &'static str, bytes: &'a [u8]) -> Result<&'a str, GitError> {
    std::str::from_utf8(bytes).map_err(|e| GitError::parse(command, format!("invalid UTF-8: {e}")))
}
