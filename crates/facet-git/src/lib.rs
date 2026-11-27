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

mod accounts;
pub mod askpass;
mod branch;
mod clone;
mod cmdlog;
mod commit;
pub mod conflict;
mod diff;
mod discard;
mod error;
pub mod forge;
pub mod graph;
mod history;
mod job;
mod lfs;
mod oid;
mod operation;
pub mod parse;
mod patch;
pub mod rebase;
mod refs;
mod remote;
mod removal;
mod repo;
mod runner;
mod stage;
mod stash;
mod status;
mod submodule;
pub mod tools;
mod worktree;

pub use accounts::{
    global_identity, identity, line_endings, set_line_endings, set_local_identity, sign_in,
    sign_out, stored_accounts, unset_local_identity, Identity, LineEndings,
};
pub use branch::{
    create_branch, create_tag, delete_branch, delete_tag, rename_branch, restore_branch,
    restore_tag, set_upstream, switch, valid_branch_name, valid_tag_name, DeletedBranch,
    DeletedTag, SwitchTarget,
};
pub use clone::{clone, clone_folder_name, init};
pub use cmdlog::{CommandLog, CommandOutcome, CommandRecord};
pub use commit::{commit, last_commit_author, last_commit_message, CommitOutcome};
pub use diff::{
    diff_file, empty_tree, object_sizes, read_blob, DiffRequest, DiffSource, LfsPointer,
};
pub use discard::{
    discard_files, discard_patch, list_undo, prune_undo, undo, DiscardFile, UndoEntry,
};
pub use error::GitError;
pub use history::{
    blame, commit_detail, file_history, log_args, search_args, search_args_in, stream_log,
    stream_log_in, stream_search, stream_search_in, Scope, SearchKind, LOG_ARGS,
};
pub use lfs::{
    lfs_attributes, lfs_lock, lfs_locks, lfs_pull, lfs_track, lfs_unlock, lfs_usage, lfs_version,
    LfsUsage,
};
pub use oid::{Hex, Oid, Parents};
pub use operation::{
    abort_operation, autostash, cherry_pick, continue_operation, merge, operation, reflog, reset,
    restore_autostash, revert, skip_operation, undo_to, Operation, OperationKind, Progress,
    ResetMode, UndoPoint,
};
pub use patch::{build_patch, PatchDirection, PatchError};
pub use refs::{refs, RefsSnapshot};
pub use remote::{
    add_remote, fetch, fetch_in_background, pull, push, push_tag, remove_remote, restore_remote,
    Network, PullMode, PushRequest,
};
pub use removal::{removal_check, RemovalCheck};
pub use repo::{commit_summary, discover, head, version, Head, RepoInfo};
pub use runner::{Git, GitCommand, GitOutput, ProgressFn};
pub use stage::{apply_patch, stage_all, stage_paths, unstage_all, unstage_paths, ApplyTo};
pub use stash::{
    restore_stash, stash_apply, stash_drop, stash_files, stash_push, StashFile, StashOptions,
};
pub use status::{line_counts, perf_config, set_perf_config, status, PerfConfig};
pub use submodule::{submodule_range, submodules, sync_submodules, update_submodules};
pub use tokio_util::sync::CancellationToken;
pub use worktree::{
    add_worktree, checked_out_elsewhere, prune_worktrees, remove_worktree, same_dir, worktrees,
    WorktreeTarget,
};
