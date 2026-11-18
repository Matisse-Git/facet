//! Worktrees, submodules and Git LFS for a repository tab (M4). Like the other `app_*`
//! files, everything here only starts background work and applies its results.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use facet_git::parse::{LfsAttributes, LfsLock, Worktree};
use facet_git::{checked_out_elsewhere, same_dir, GitError, LfsUsage, WorktreeTarget};

use crate::actions::Action;
use crate::app::{describe_error, FacetApp};
use crate::app_repo::{NetworkJob, Outcome, RepoEvent};
use crate::app_work::{ready_info, WorkEvent};
use crate::ui::confirm::Confirm;
use crate::ui::prompt::{Prompt, PromptField, PromptKind};

/// Git LFS state of one tab.
#[derive(Default)]
pub struct LfsState {
    pub usage: LfsUsage,
    /// LFS attributes of the files in the change lists, for those LFS handles.
    pub attributes: HashMap<String, LfsAttributes>,
    /// The path set (a hash) of the latest attribute request; older answers are dropped.
    attributes_for: Option<u64>,
    pub locks: Vec<LfsLock>,
    pub locks_loaded: bool,
    pub locks_running: bool,
    /// Report the outcome of the lock request in flight (the person asked for it).
    locks_notify: bool,
    /// Why the lock list couldn't be read (shown in the sidebar).
    pub locks_error: Option<String>,
}

impl LfsState {
    pub fn lock(&self, path: &str) -> Option<&LfsLock> {
        self.locks.iter().find(|lock| lock.path == path)
    }

    pub fn attributes(&self, path: &str) -> LfsAttributes {
        self.attributes.get(path).copied().unwrap_or_default()
    }
}

/// Results of LFS queries for one tab.
pub enum TreeEvent {
    Attributes {
        key: u64,
        result: Result<Vec<(String, LfsAttributes)>, GitError>,
    },
    Locks(Result<Vec<LfsLock>, GitError>),
}

/// A path git printed (forward slashes) in Windows form.
pub fn native_path(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', "\\"))
}

/// The folder a new worktree goes to by default: next to the repository, named after it and
/// the branch (`D:\game` + `feature/x` → `D:\game-feature-x`).
pub fn suggested_worktree_path(root: &Path, branch: &str) -> PathBuf {
    let name = root.file_name().map_or_else(
        || "repository".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let suffix: String = branch
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let suffix = suffix.trim_matches('-');
    let folder = if suffix.is_empty() {
        format!("{name}-worktree")
    } else {
        format!("{name}-{suffix}")
    };
    root.parent().unwrap_or(root).join(folder)
}

/// What a worktree has checked out, to add it again after a removal.
fn restore_target(worktree: &Worktree) -> Option<WorktreeTarget> {
    match (worktree.branch_name(), &worktree.head) {
        (Some(branch), _) => Some(WorktreeTarget::Branch(branch.to_owned())),
        (None, Some(head)) => Some(WorktreeTarget::Detached(head.clone())),
        (None, None) => None,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl FacetApp {
    // ----- LFS ---------------------------------------------------------------------------------

    /// A new refs snapshot told how the repository uses LFS: start what that needs.
    pub(crate) fn on_lfs_usage(&mut self, index: usize, usage: LfsUsage) {
        let lfs = &mut self.tabs[index].lfs;
        let was = std::mem::replace(&mut lfs.usage, usage);
        if !usage.lfs {
            lfs.attributes.clear();
            lfs.attributes_for = None;
        } else if !was.lfs {
            self.request_lfs_attributes(index);
        }
        let lfs = &self.tabs[index].lfs;
        if usage.locking && !lfs.locks_loaded && !lfs.locks_running {
            self.request_locks(index, false);
        }
    }

    /// Ask `check-attr` which of the listed files LFS handles (only when the set changed).
    pub(crate) fn request_lfs_attributes(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if !tab.lfs.usage.lfs {
            return;
        }
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let paths: Vec<String> = tab.work.entries.iter().map(|e| e.path.clone()).collect();
        let mut hasher = DefaultHasher::new();
        paths.hash(&mut hasher);
        let key = hasher.finish();
        if tab.lfs.attributes_for == Some(key) {
            return;
        }
        tab.lfs.attributes_for = Some(key);
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let result = facet_git::lfs_attributes(&git, &root, &paths).await;
            WorkEvent::Repo(RepoEvent::Trees(TreeEvent::Attributes { key, result }))
        });
    }

    /// Read the lock list from the LFS server (a network request, on the credential git).
    pub(crate) fn request_locks(&mut self, index: usize, notify: bool) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        tab.lfs.locks_notify |= notify;
        if tab.lfs.locks_running {
            return;
        }
        tab.lfs.locks_running = true;
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let result = facet_git::lfs_locks(&git, &root).await;
            WorkEvent::Repo(RepoEvent::Trees(TreeEvent::Locks(result)))
        });
    }

    pub(crate) fn handle_tree_event(&mut self, index: usize, event: TreeEvent) {
        let lfs = &mut self.tabs[index].lfs;
        match event {
            TreeEvent::Attributes { key, result } => {
                if lfs.attributes_for != Some(key) {
                    return;
                }
                match result {
                    Ok(list) => {
                        lfs.attributes = list
                            .into_iter()
                            .filter(|(_, attributes)| attributes.lfs || attributes.lockable)
                            .collect();
                    }
                    Err(GitError::Cancelled) => lfs.attributes_for = None,
                    Err(error) => {
                        lfs.attributes_for = None;
                        let detail = describe_error(&error).1;
                        self.error_toast("Couldn't read the LFS attributes", detail);
                    }
                }
            }
            TreeEvent::Locks(result) => {
                lfs.locks_running = false;
                let notify = std::mem::take(&mut lfs.locks_notify);
                match result {
                    Ok(locks) => {
                        lfs.locks_loaded = true;
                        lfs.locks_error = None;
                        let count = locks.len();
                        lfs.locks = locks;
                        if notify {
                            self.info_toast(match count {
                                0 => "No files are locked".to_owned(),
                                1 => "1 file is locked".to_owned(),
                                n => format!("{n} files are locked"),
                            });
                        }
                    }
                    Err(GitError::Cancelled) => {}
                    Err(error) => {
                        lfs.locks_loaded = true;
                        let detail = describe_error(&error).1;
                        lfs.locks_error = Some(
                            detail
                                .as_deref()
                                .and_then(|text| text.lines().last())
                                .unwrap_or("The LFS server didn't answer")
                                .to_owned(),
                        );
                        if notify {
                            self.error_toast("Couldn't read the LFS locks", detail);
                        }
                    }
                }
            }
        }
    }

    /// A lock or unlock finished: show the new lock list.
    pub(crate) fn on_locks_outcome(
        &mut self,
        index: usize,
        title: String,
        undo: Option<Action>,
        locks: Option<Vec<LfsLock>>,
    ) {
        if let Some(locks) = locks {
            let lfs = &mut self.tabs[index].lfs;
            lfs.locks = locks;
            lfs.locks_loaded = true;
            lfs.locks_error = None;
        }
        match undo {
            Some(action) => self.action_toast(title, "Undo", action),
            None => self.info_toast(title),
        }
    }

    fn lock_file(&mut self, path: String) {
        let name = file_name(&path).to_owned();
        self.repo_op(format!("lock {name}"), move |git, root| async move {
            let lock = facet_git::lfs_lock(&git, &root, &path).await?;
            let locks = facet_git::lfs_locks(&git, &root).await.ok();
            Ok(Outcome::Locks {
                title: format!("Locked {name}"),
                undo: Some(Action::UnlockFile { id: lock.id, path }),
                locks,
            })
        });
    }

    fn unlock_file(&mut self, id: String, path: String, force: bool) {
        let name = file_name(&path).to_owned();
        self.repo_op(format!("unlock {name}"), move |git, root| async move {
            facet_git::lfs_unlock(&git, &root, &id, force).await?;
            let locks = facet_git::lfs_locks(&git, &root).await.ok();
            Ok(if force {
                // Someone else's lock can't be given back.
                Outcome::Locks {
                    title: format!("Broke the lock on {name}"),
                    undo: None,
                    locks,
                }
            } else {
                Outcome::Locks {
                    title: format!("Unlocked {name}"),
                    undo: Some(Action::LockFile(path)),
                    locks,
                }
            })
        });
    }

    fn ask_track_with_lfs(&mut self) {
        let pattern = self
            .current_file()
            .and_then(|(path, _)| {
                let name = file_name(&path).to_owned();
                name.rsplit_once('.').map(|(_, ext)| format!("*.{ext}"))
            })
            .unwrap_or_default();
        self.prompt = Some(
            Prompt::new(PromptKind::LfsTrack, "Store Files with Git LFS", "Track")
                .body(
                    "Files matching the pattern are stored with Git LFS from now on. Facet adds \
                     the pattern to .gitattributes; commit that file to share it. Files already \
                     committed stay as they are.",
                )
                .field(PromptField::new("Pattern", "*.psd", true).with_value(&pattern))
                .check("Lockable: read-only until someone locks the file", false),
        );
    }

    // ----- worktrees ---------------------------------------------------------------------------

    /// Refuse to switch to a branch another worktree has checked out, offering to open that
    /// worktree instead. Returns whether it refused.
    pub(crate) fn refuse_branch_in_other_worktree(&mut self, branch: &str) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let Some(info) = ready_info(tab) else {
            return false;
        };
        let Some(worktree) =
            checked_out_elsewhere(&tab.refs.snapshot.worktrees, info.root(), branch)
        else {
            return false;
        };
        let path = native_path(&worktree.path);
        self.confirm = Some(Confirm {
            title: format!("{branch} is checked out in another worktree"),
            body: format!(
                "Git lets a branch be checked out in only one worktree at a time, and {branch} \
                 is checked out in {}. Open that worktree to work on the branch there.",
                path.display()
            ),
            confirm_label: "Open That Worktree".to_owned(),
            action: Action::OpenPath(path),
        });
        true
    }

    fn ask_add_worktree(&mut self, branch: Option<String>) {
        let Some(root) = self
            .active_tab()
            .and_then(ready_info)
            .map(|i| i.root().to_owned())
        else {
            return;
        };
        let branch = branch.unwrap_or_default();
        let folder = if branch.is_empty() {
            String::new()
        } else {
            suggested_worktree_path(&root, &branch)
                .display()
                .to_string()
        };
        let hint = suggested_worktree_path(&root, "branch")
            .display()
            .to_string();
        self.prompt = Some(
            Prompt::new(PromptKind::AddWorktree, "Add Worktree", "Add Worktree")
                .body(
                    "A worktree is a second folder of this repository with its own checked-out \
                     branch. An existing branch is checked out there; a new name creates the \
                     branch at HEAD. Leave the folder empty to put it next to this repository.",
                )
                .field(PromptField::new("Branch", "feature/my-change", true).with_value(&branch))
                .field(PromptField::new("Folder", &hint, false).with_value(&folder)),
        );
    }

    fn create_worktree(&mut self, path: PathBuf, branch: String) {
        if self.refuse_branch_in_other_worktree(&branch) {
            return;
        }
        let Some(tab) = self.active_tab() else {
            return;
        };
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let path = if path.as_os_str().is_empty() {
            suggested_worktree_path(&root, &branch)
        } else if path.is_relative() {
            root.parent().unwrap_or(&root).join(path)
        } else {
            path
        };
        let snapshot = &tab.refs.snapshot;
        let target = if snapshot.local().any(|r| r.name == branch) {
            WorktreeTarget::Branch(branch.clone())
        } else if let Some(remote) = snapshot.refs.iter().find(|r| {
            matches!(r.kind, facet_git::parse::RefKind::Remote { .. }) && r.name == branch
        }) {
            // `origin/feature`: a local `feature` tracking it.
            WorktreeTarget::NewBranch {
                name: remote.short_name().to_owned(),
                start: Some(remote.name.clone()),
            }
        } else {
            WorktreeTarget::NewBranch {
                name: branch.clone(),
                start: None,
            }
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.repo_op("add the worktree", move |git, root| async move {
            facet_git::add_worktree(&git, &root, &path, &target).await?;
            Ok(Outcome::Offer {
                title: format!("Added worktree {name}"),
                button: "Open",
                action: Action::OpenPath(path),
            })
        });
    }

    fn ask_remove_worktree(&mut self, path: PathBuf) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let Some(worktree) = tab
            .refs
            .snapshot
            .worktrees
            .iter()
            .find(|w| same_dir(Path::new(&w.path), &path))
        else {
            return;
        };
        if worktree.main {
            self.info_toast("The main worktree can't be removed");
            return;
        }
        if same_dir(&path, &root) {
            self.info_toast("Open another worktree of this repository to remove this one");
            return;
        }
        let Some(restore) = restore_target(worktree) else {
            self.info_toast("This worktree has no commit checked out yet");
            return;
        };
        let what = match &restore {
            WorktreeTarget::Branch(branch) => format!("The branch {branch} stays"),
            _ => "Its commit stays in the repository".to_owned(),
        };
        self.confirm = Some(Confirm {
            title: format!("Remove the worktree {}?", worktree.name()),
            body: format!(
                "Facet deletes the folder {}. Git refuses if it has uncommitted changes or \
                 untracked files, so no work is lost. {what}, and Undo adds the worktree again.",
                path.display()
            ),
            confirm_label: "Remove".to_owned(),
            action: Action::ConfirmedRemoveWorktree { path, restore },
        });
    }

    fn remove_worktree(&mut self, path: PathBuf, restore: WorktreeTarget) {
        // A tab showing the worktree holds a watch on its folder, which keeps Windows from
        // deleting it: close it first.
        while let Some(open) = self.tabs.iter().position(|t| same_dir(&t.path, &path)) {
            self.remove_tab(open);
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.repo_op("remove the worktree", move |git, root| async move {
            let text = path.to_string_lossy().into_owned();
            facet_git::remove_worktree(&git, &root, &text).await?;
            Ok(Outcome::Undo {
                title: format!("Removed worktree {name}"),
                action: Action::RestoreWorktree {
                    path,
                    target: restore,
                },
            })
        });
    }

    // ----- actions -----------------------------------------------------------------------------

    /// Handle worktree, submodule and LFS actions. Returns the action back if it is not one.
    pub(crate) fn run_tree_action(&mut self, action: Action) -> Option<Action> {
        let Some(index) = self.active.filter(|_| self.active_work().is_some()) else {
            return match action {
                Action::AddWorktree { .. }
                | Action::CreateWorktree { .. }
                | Action::RemoveWorktree(_)
                | Action::ConfirmedRemoveWorktree { .. }
                | Action::RestoreWorktree { .. }
                | Action::PruneWorktrees
                | Action::UpdateSubmodules(_)
                | Action::SyncSubmodules(_)
                | Action::OpenSubmodule(_)
                | Action::RefreshLocks
                | Action::LockFile(_)
                | Action::UnlockFile { .. }
                | Action::ForceUnlock { .. }
                | Action::ConfirmedForceUnlock { .. }
                | Action::LfsPull
                | Action::TrackWithLfs
                | Action::CreateLfsTrack { .. } => None,
                other => Some(other),
            };
        };
        match action {
            Action::AddWorktree { branch } => self.ask_add_worktree(branch),
            Action::CreateWorktree { path, branch } => self.create_worktree(path, branch),
            Action::RemoveWorktree(path) => self.ask_remove_worktree(path),
            Action::ConfirmedRemoveWorktree { path, restore } => {
                self.remove_worktree(path, restore);
            }
            Action::RestoreWorktree { path, target } => {
                self.repo_op("add the worktree again", move |git, root| async move {
                    facet_git::add_worktree(&git, &root, &path, &target).await?;
                    Ok(Outcome::Done(Some("Worktree restored".to_owned())))
                });
            }
            Action::PruneWorktrees => {
                self.repo_op("forget missing worktrees", |git, root| async move {
                    facet_git::prune_worktrees(&git, &root).await?;
                    Ok(Outcome::Done(Some(
                        "Forgot worktrees whose folders are gone".to_owned(),
                    )))
                });
            }

            Action::UpdateSubmodules(path) => {
                self.start_network(NetworkJob::UpdateSubmodules(path));
            }
            Action::SyncSubmodules(path) => {
                self.repo_op("sync submodule URLs", move |git, root| async move {
                    facet_git::sync_submodules(&git, &root, path.as_deref()).await?;
                    Ok(Outcome::Done(Some("Submodule URLs synced".to_owned())))
                });
            }
            Action::OpenSubmodule(path) => {
                if let Some(root) = ready_info(&self.tabs[index]).map(|i| i.root().to_owned()) {
                    let folder = path.split('/').fold(root, |dir, part| dir.join(part));
                    self.open_path(folder, true);
                }
            }

            Action::RefreshLocks => self.request_locks(index, true),
            Action::LockFile(path) => self.lock_file(path),
            Action::UnlockFile { id, path } => self.unlock_file(id, path, false),
            Action::ForceUnlock { id, path, owner } => {
                let owner = if owner.is_empty() {
                    "someone else".to_owned()
                } else {
                    owner
                };
                self.confirm = Some(Confirm {
                    title: format!("Break {owner}'s lock on {}?", file_name(&path)),
                    body: format!(
                        "{owner} may still be editing {path}; their changes could conflict with \
                         yours. A broken lock can't be given back — {owner} has to lock the \
                         file again."
                    ),
                    confirm_label: "Force Unlock".to_owned(),
                    action: Action::ConfirmedForceUnlock { id, path },
                });
            }
            Action::ConfirmedForceUnlock { id, path } => self.unlock_file(id, path, true),
            Action::LfsPull => self.start_network(NetworkJob::LfsPull),
            Action::TrackWithLfs => self.ask_track_with_lfs(),
            Action::CreateLfsTrack { pattern, lockable } => {
                self.repo_op(format!("track {pattern}"), move |git, root| async move {
                    facet_git::lfs_track(&git, &root, &pattern, lockable).await?;
                    Ok(Outcome::Done(Some(format!(
                        "{pattern} is stored with LFS from now on — commit .gitattributes"
                    ))))
                });
            }
            other => return Some(other),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_folders_sit_next_to_the_repository() {
        let root = Path::new(r"D:\Projects\game");
        assert_eq!(
            suggested_worktree_path(root, "feature/new hud"),
            PathBuf::from(r"D:\Projects\game-feature-new-hud")
        );
        assert_eq!(
            suggested_worktree_path(root, ""),
            PathBuf::from(r"D:\Projects\game-worktree")
        );
        assert_eq!(native_path("D:/a/b c"), PathBuf::from(r"D:\a\b c"));
    }

    #[test]
    fn removed_worktrees_come_back_on_their_branch_or_commit() {
        let mut worktree = Worktree {
            path: "D:/game-x".into(),
            head: Some("a".repeat(40)),
            branch: Some("refs/heads/x".into()),
            ..Worktree::default()
        };
        assert_eq!(
            restore_target(&worktree),
            Some(WorktreeTarget::Branch("x".into()))
        );
        worktree.branch = None;
        worktree.detached = true;
        assert_eq!(
            restore_target(&worktree),
            Some(WorktreeTarget::Detached("a".repeat(40)))
        );
        worktree.head = None;
        assert_eq!(restore_target(&worktree), None);
    }
}
