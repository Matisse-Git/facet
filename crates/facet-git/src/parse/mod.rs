//! Parsers for git plumbing output.
//!
//! Every parser here is a pure function over bytes and has golden tests (fixtures captured from
//! real git live in `tests/golden/`). Parsers never see human-readable porcelain.

mod blame;
mod commit;
mod detail;
mod diff;
mod filelog;
mod lfs;
mod log;
mod reflog;
mod refs;
mod stash;
mod status;
mod submodule;
mod version;
mod worktree;

pub use blame::{parse_blame, BlameCommit, BlameParser, BlameRange};
pub use commit::{parse_commit_summary, CommitSummary, COMMIT_SUMMARY_FORMAT};
pub use detail::{
    parse_changed_files, parse_commit_detail, ChangedFile, CommitDetail, DETAIL_FORMAT,
};
pub use diff::{parse_file_diff, DiffLine, FileDiff, Hunk, LineKind};
pub use filelog::{parse_file_log, FileLogEntry};
pub use lfs::{
    parse_check_attr, parse_locks, parse_new_lock, parse_verified_locks, LfsAttributes, LfsLock,
};
pub use log::{parse_log, Authors, LogCommit, LogParser, LOG_FORMAT};
pub use reflog::{parse_reflog, ReflogEntry, REFLOG_FORMAT};
pub use refs::{parse_refs, parse_remotes, GitRef, RefKind, Remote, Track, REFS_FORMAT};
pub use stash::{parse_stashes, StashEntry, STASH_FORMAT};
pub use status::{parse_status, BranchInfo, Change, EntryKind, Status, StatusEntry};
pub use submodule::{parse_gitlinks, parse_gitmodules, Submodule};
pub use version::{parse_version, GitVersion};
pub use worktree::{parse_worktrees, Worktree};

use crate::GitError;

/// Decode git output as UTF-8, naming the command in the error.
pub(crate) fn utf8<'a>(command: &'static str, bytes: &'a [u8]) -> Result<&'a str, GitError> {
    std::str::from_utf8(bytes).map_err(|e| GitError::parse(command, format!("invalid UTF-8: {e}")))
}
