//! Async git CLI plumbing layer for Facet.
//!
//! Every git operation in Facet goes through [`Git`], which spawns the user's real `git`
//! executable. That is what guarantees parity with hooks, LFS, the credential manager, signing,
//! autocrlf and long paths. Rules for everything in this crate:
//!
//! * Only machine-readable plumbing output is parsed (`-z`, explicit `--format`), never
//!   human-readable porcelain.
//! * Nothing here blocks: all calls are `async` and cancellable.
//! * Nothing here touches the global git config, and hooks are never bypassed.
//! * Every spawned command is recorded in the [`CommandLog`] exactly as executed.

mod cmdlog;
mod commit;
mod diff;
mod discard;
mod error;
pub mod parse;
mod patch;
mod repo;
mod runner;
mod stage;
mod status;

pub use cmdlog::{CommandLog, CommandOutcome, CommandRecord};
pub use commit::{commit, last_commit_message, CommitOutcome};
pub use diff::{diff_file, object_sizes, read_blob, DiffRequest, DiffSource, LfsPointer};
pub use discard::{
    discard_files, discard_patch, list_undo, prune_undo, undo, DiscardFile, UndoEntry,
};
pub use error::GitError;
pub use patch::{build_patch, PatchDirection, PatchError};
pub use repo::{commit_summary, discover, head, version, Head, RepoInfo};
pub use runner::{Git, GitCommand, GitOutput};
pub use stage::{apply_patch, stage_all, stage_paths, unstage_all, unstage_paths, ApplyTo};
pub use status::{perf_config, set_perf_config, status, PerfConfig};
pub use tokio_util::sync::CancellationToken;
