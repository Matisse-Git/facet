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
mod error;
pub mod parse;
mod repo;
mod runner;

pub use cmdlog::{CommandLog, CommandOutcome, CommandRecord};
pub use error::GitError;
pub use repo::{discover, head, version, Head, RepoInfo};
pub use runner::{Git, GitCommand, GitOutput};
pub use tokio_util::sync::CancellationToken;
