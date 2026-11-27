//! Removing repositories from Facet: off the lists at once (Undo puts them back, tab and all),
//! and, only when asked in the Remove Repository dialog, the folder to the Recycle Bin after a
//! check of what exists only there. Facet never deletes a folder permanently.
//!
//! Also keeps an eye on the recent list: entries whose folder is gone or no longer a
//! repository are marked, so stale ones are easy to spot and remove.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use facet_git::{GitError, RemovalCheck};

use crate::actions::Action;
use crate::app::{display_name, Event, FacetApp};
use crate::platform::{self, RecycleError};
use crate::settings::same_path;
use crate::ui::toast::ToastKind;

/// Branches listed by name in the dialog's warning; the rest are summed up.
const BRANCHES_LISTED: usize = 4;

/// What a recent entry's folder looks like now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoHealth {
    Fine,
    /// The folder is gone (deleted, renamed, or its drive is not connected).
    Missing,
    /// The folder exists but holds no repository any more.
    NotARepository,
}

impl RepoHealth {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Fine => None,
            Self::Missing => Some("Folder missing"),
            Self::NotARepository => Some("Not a repository"),
        }
    }

    /// Look at `path` on disk: a working tree has `.git` (a folder, or a file in linked
    /// worktrees and submodules); a bare repository has `HEAD` and `objects` itself.
    fn of(path: &Path) -> Self {
        if !path.is_dir() {
            Self::Missing
        } else if path.join(".git").exists()
            || (path.join("HEAD").is_file() && path.join("objects").is_dir())
        {
            Self::Fine
        } else {
            Self::NotARepository
        }
    }
}

/// The recent list's health, read off the UI thread.
#[derive(Default)]
pub struct RecentHealth {
    by_path: HashMap<PathBuf, RepoHealth>,
    /// The list the last check covered; a different list is checked again.
    checked: Vec<PathBuf>,
    in_flight: bool,
}

impl RecentHealth {
    /// `Fine` until checked: a mark appears only once the disk says so.
    pub fn of(&self, path: &Path) -> RepoHealth {
        self.by_path
            .iter()
            .find(|(known, _)| same_path(known, path))
            .map_or(RepoHealth::Fine, |(_, health)| *health)
    }
}

/// What the dialog knows about the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderCheck {
    Checking,
    Ready(RemovalCheck),
    /// The folder is gone: there is nothing to move to the Recycle Bin.
    Missing,
    /// The folder exists, but git can't read it as a repository root.
    Unreadable(String),
}

/// The Remove Repository dialog.
pub struct RemoveRepoView {
    pub path: PathBuf,
    pub name: String,
    /// "Also move the folder to the Recycle Bin".
    pub recycle: bool,
    pub check: FolderCheck,
    /// The folder is being moved; the dialog waits for the result.
    pub recycling: bool,
}

impl RemoveRepoView {
    /// Why the folder can't go to the Recycle Bin, if it can't.
    pub fn recycle_blocked(&self) -> Option<String> {
        match &self.check {
            FolderCheck::Missing => Some("The folder no longer exists.".to_owned()),
            FolderCheck::Ready(check) if !check.linked_worktrees.is_empty() => {
                let names: Vec<String> = check
                    .linked_worktrees
                    .iter()
                    .map(|path| display_name(path))
                    .collect();
                Some(format!(
                    "Its linked worktrees ({}) keep their history in this folder. Remove its \
                     worktrees first (Worktrees in the sidebar).",
                    names.join(", ")
                ))
            }
            _ => None,
        }
    }

    /// What exists only in this folder, one line each.
    pub fn warnings(&self) -> Vec<String> {
        let check = match &self.check {
            FolderCheck::Ready(check) => check,
            FolderCheck::Unreadable(_) => {
                return vec![
                    "Facet can't read it as a Git repository, so it can't tell what's in it."
                        .to_owned(),
                ]
            }
            FolderCheck::Checking | FolderCheck::Missing => return Vec::new(),
        };
        let mut lines = Vec::new();
        if check.changed_files > 0 {
            lines.push(count(check.changed_files, "file", "files") + " with uncommitted changes");
        }
        if check.untracked_files > 0 {
            lines.push(count(
                check.untracked_files,
                "untracked file",
                "untracked files",
            ));
        }
        let shown = check.unpushed.iter().take(BRANCHES_LISTED);
        for (index, (branch, commits)) in shown.enumerate() {
            let at_least = if check.unpushed_capped && index == 0 {
                "at least "
            } else {
                ""
            };
            lines.push(format!(
                "{at_least}{} on {branch} not pushed to any remote",
                count(*commits, "commit", "commits")
            ));
        }
        if check.unpushed.len() > BRANCHES_LISTED {
            let rest = check.unpushed.len() - BRANCHES_LISTED;
            lines.push(format!(
                "unpushed commits on {rest} more {}",
                if rest == 1 { "branch" } else { "branches" }
            ));
        }
        if check.stashes > 0 {
            lines.push(count(check.stashes, "stash", "stashes"));
        }
        if !check.has_remote {
            lines.push("no remote: this folder is the only copy of its history".to_owned());
        }
        lines
    }

    /// The confirm button can be pressed.
    pub fn can_confirm(&self) -> bool {
        !self.recycling
            && (!self.recycle
                || (self.check != FolderCheck::Checking && self.recycle_blocked().is_none()))
    }

    pub fn confirm_label(&self) -> &'static str {
        if self.recycling {
            "Moving…"
        } else if !self.recycle {
            "Remove"
        } else if self.warnings().is_empty() {
            "Move to Recycle Bin"
        } else {
            "Move to Recycle Bin anyway"
        }
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Results of the dialog's background work.
pub enum RemoveEvent {
    Checked {
        path: PathBuf,
        result: Result<RemovalCheck, GitError>,
    },
    Recycled {
        path: PathBuf,
        result: Result<(), RecycleError>,
        /// A linked worktree: the main repository, where git's record of it is pruned.
        main_worktree: Option<PathBuf>,
        /// Puts the entry back on the lists (offered when the move failed).
        restore: Action,
    },
    Pruned(Result<(), GitError>),
    Health(Vec<(PathBuf, RepoHealth)>),
}

impl FacetApp {
    /// Take `path` off the recent list and close its tab. The toast offers Undo and, while the
    /// folder exists, the Recycle Bin.
    fn remove_repository(&mut self, path: PathBuf) {
        let Some(restore) = self.forget_repository(&path) else {
            return;
        };
        let mut actions = vec![("Undo".to_owned(), restore)];
        if self.recent_health.of(&path) != RepoHealth::Missing {
            actions.push((
                "Move to Recycle Bin…".to_owned(),
                Action::ShowRemoveRepository {
                    path: path.clone(),
                    recycle: true,
                },
            ));
        }
        let title = format!("Removed {} from Facet", display_name(&path));
        self.offer_toast(ToastKind::Info, title, None, actions);
    }

    /// Close the tabs of `path` and drop it from the recent list. Returns the action that puts
    /// both back, or `None` when Facet did not list it.
    fn forget_repository(&mut self, path: &Path) -> Option<Action> {
        let tab = self.tabs.iter().position(|t| same_path(&t.path, path));
        let active = tab.is_some() && tab == self.active;
        // One repository has one tab, but a path opened twice before loading may have two.
        while let Some(index) = self.tabs.iter().position(|t| same_path(&t.path, path)) {
            self.remove_tab(index);
        }
        let recent = self.settings.forget_recent(path);
        (tab.is_some() || recent.is_some()).then(|| Action::RestoreRepository {
            path: path.to_owned(),
            recent,
            tab,
            active,
        })
    }

    /// Undo of a removal: the recent entry at its old place, the tab at its old position.
    fn restore_repository(
        &mut self,
        path: PathBuf,
        recent: Option<usize>,
        tab: Option<usize>,
        active: bool,
    ) {
        if let Some(index) = recent {
            self.settings.restore_recent(&path, index);
        }
        let Some(position) = tab else {
            return;
        };
        if self.tabs.iter().any(|t| same_path(&t.path, &path)) {
            return; // opened again meanwhile
        }
        let active_id = self.active_tab().map(|t| t.id);
        // Reopened like a restored session tab: it does not move to the top of the recents.
        self.open_path(path, false);
        let Some(tab) = self.tabs.pop() else {
            return;
        };
        let id = tab.id;
        let position = position.min(self.tabs.len());
        self.tabs.insert(position, tab);
        let keep = if active {
            Some(id)
        } else {
            active_id.or(Some(id))
        };
        self.active = keep.and_then(|id| self.tabs.iter().position(|t| t.id == id));
    }

    fn show_remove_repository(&mut self, path: PathBuf, recycle: bool) {
        let name = display_name(&path);
        self.remove_view = Some(RemoveRepoView {
            path: path.clone(),
            name,
            recycle,
            check: FolderCheck::Checking,
            recycling: false,
        });
        let git = self.git.clone();
        self.spawn(async move {
            let result = facet_git::removal_check(&git, &path).await;
            Event::Remove(RemoveEvent::Checked { path, result })
        });
    }

    /// The dialog's confirm button (or Enter).
    fn confirm_remove_repository(&mut self) {
        let Some(view) = &self.remove_view else {
            return;
        };
        if !view.can_confirm() {
            return;
        }
        let path = view.path.clone();
        if !view.recycle {
            self.remove_view = None;
            self.remove_repository(path);
            return;
        }
        if path.parent().is_none() {
            return; // a drive root: never
        }
        let main_worktree = match &view.check {
            FolderCheck::Ready(check) => check.main_worktree.clone(),
            _ => None,
        };
        // Its tab's file watcher would keep Windows from moving the folder (G32).
        let restore = self
            .forget_repository(&path)
            .unwrap_or(Action::RestoreRepository {
                path: path.clone(),
                recent: None,
                tab: None,
                active: false,
            });
        if let Some(view) = &mut self.remove_view {
            view.recycling = true;
        }
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("facet-recycle".into())
            .spawn(move || {
                let result = platform::move_to_recycle_bin(&path);
                let event = Event::Remove(RemoveEvent::Recycled {
                    path,
                    result,
                    main_worktree,
                    restore,
                });
                if tx.send(event).is_ok() {
                    ctx.request_repaint();
                }
            });
        if let Err(error) = spawned {
            if let Some(view) = &mut self.remove_view {
                view.recycling = false;
            }
            self.error_toast("Couldn't start moving the folder", Some(error.to_string()));
        }
    }

    pub(crate) fn handle_remove_event(&mut self, event: RemoveEvent) {
        match event {
            RemoveEvent::Checked { path, result } => {
                let Some(view) = self
                    .remove_view
                    .as_mut()
                    .filter(|view| same_path(&view.path, &path))
                else {
                    return;
                };
                view.check = match result {
                    Ok(check) => FolderCheck::Ready(check),
                    Err(GitError::PathNotFound { .. }) => FolderCheck::Missing,
                    Err(error) => FolderCheck::Unreadable(error.to_string()),
                };
                if view.check == FolderCheck::Missing {
                    view.recycle = false;
                }
            }
            RemoveEvent::Recycled {
                path,
                result,
                main_worktree,
                restore,
            } => {
                self.remove_view = None;
                let name = display_name(&path);
                match result {
                    Ok(()) => {
                        self.push_toast(
                            ToastKind::Info,
                            format!("Moved {name} to the Recycle Bin"),
                            Some(
                                "Facet can't undo this: restore the folder from the Recycle \
                                 Bin if you need it back."
                                    .to_owned(),
                            ),
                        );
                        if let Some(main) = main_worktree {
                            let git = self.git.clone();
                            self.spawn(async move {
                                let result = facet_git::prune_worktrees(&git, &main).await;
                                Event::Remove(RemoveEvent::Pruned(result))
                            });
                        }
                    }
                    Err(error) => self.offer_toast(
                        ToastKind::Error,
                        format!("Couldn't move {name} to the Recycle Bin"),
                        Some(format!(
                            "{error}\nThe folder is still there; Facet no longer lists it."
                        )),
                        vec![("Undo".to_owned(), restore)],
                    ),
                }
                self.check_recent_health(true);
            }
            RemoveEvent::Pruned(Ok(())) => {}
            RemoveEvent::Pruned(Err(error)) => {
                let (_, detail) = crate::app::describe_error(&error);
                self.error_toast(
                    "Couldn't clean up git's record of the removed worktree",
                    detail,
                );
            }
            RemoveEvent::Health(results) => {
                self.recent_health.in_flight = false;
                self.recent_health.by_path = results.into_iter().collect();
            }
        }
    }

    /// Look at the recent folders again when the list changed since the last look, or always
    /// with `force` (the window got focus back: folders may have been deleted meanwhile).
    pub(crate) fn check_recent_health(&mut self, force: bool) {
        let health = &mut self.recent_health;
        if health.in_flight || (!force && health.checked == self.settings.recent_repos) {
            return;
        }
        health.in_flight = true;
        health.checked = self.settings.recent_repos.clone();
        let paths = health.checked.clone();
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn_blocking(move || {
            let results = paths
                .into_iter()
                .map(|path| {
                    let health = RepoHealth::of(&path);
                    (path, health)
                })
                .collect();
            if tx.send(Event::Remove(RemoveEvent::Health(results))).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Actions of repository removal; anything else is handed back.
    pub(crate) fn run_remove_action(&mut self, action: Action) -> Option<Action> {
        match action {
            Action::RemoveRepository(path) => self.remove_repository(path),
            Action::RestoreRepository {
                path,
                recent,
                tab,
                active,
            } => self.restore_repository(path, recent, tab, active),
            Action::ShowRemoveActiveRepository => {
                if let Some(path) = self.active_tab().map(|tab| tab.path.clone()) {
                    self.show_remove_repository(path, false);
                }
            }
            Action::ShowRemoveRepository { path, recycle } => {
                self.show_remove_repository(path, recycle);
            }
            Action::ToggleRemoveRecycle => {
                if let Some(view) = self
                    .remove_view
                    .as_mut()
                    .filter(|view| !view.recycling && view.recycle_blocked().is_none())
                {
                    view.recycle = !view.recycle;
                }
            }
            Action::ConfirmRemoveRepository => self.confirm_remove_repository(),
            Action::CloseRemoveRepository => {
                if self
                    .remove_view
                    .as_ref()
                    .is_some_and(|view| !view.recycling)
                {
                    self.remove_view = None;
                }
            }
            other => return Some(other),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(check: FolderCheck, recycle: bool) -> RemoveRepoView {
        RemoveRepoView {
            path: PathBuf::from(r"D:\Repos\Old"),
            name: "Old".into(),
            recycle,
            check,
            recycling: false,
        }
    }

    fn safe() -> RemovalCheck {
        RemovalCheck {
            has_remote: true,
            ..RemovalCheck::default()
        }
    }

    #[test]
    fn removing_from_the_list_never_waits_for_the_check() {
        let view = view(FolderCheck::Checking, false);
        assert!(view.can_confirm());
        assert_eq!(view.confirm_label(), "Remove");
    }

    #[test]
    fn the_recycle_bin_waits_for_the_check_and_names_what_would_be_lost() {
        assert!(!view(FolderCheck::Checking, true).can_confirm());

        let clean = view(FolderCheck::Ready(safe()), true);
        assert!(clean.can_confirm() && clean.warnings().is_empty());
        assert_eq!(clean.confirm_label(), "Move to Recycle Bin");

        let check = RemovalCheck {
            changed_files: 1,
            untracked_files: 3,
            unpushed: (1..=6).map(|i| (format!("b{i}"), 7 - i)).collect(),
            unpushed_capped: true,
            stashes: 2,
            has_remote: false,
            ..RemovalCheck::default()
        };
        let risky = view(FolderCheck::Ready(check), true);
        assert!(risky.can_confirm(), "warned, never blocked");
        assert_eq!(risky.confirm_label(), "Move to Recycle Bin anyway");
        assert_eq!(
            risky.warnings(),
            [
                "1 file with uncommitted changes",
                "3 untracked files",
                "at least 6 commits on b1 not pushed to any remote",
                "5 commits on b2 not pushed to any remote",
                "4 commits on b3 not pushed to any remote",
                "3 commits on b4 not pushed to any remote",
                "unpushed commits on 2 more branches",
                "2 stashes",
                "no remote: this folder is the only copy of its history",
            ]
        );

        let unreadable = view(FolderCheck::Unreadable("not a repo".into()), true);
        assert!(unreadable.can_confirm());
        assert_eq!(unreadable.warnings().len(), 1);
    }

    #[test]
    fn missing_folders_and_main_worktrees_cannot_be_recycled() {
        let missing = view(FolderCheck::Missing, true);
        assert!(missing.recycle_blocked().is_some() && !missing.can_confirm());

        let with_worktrees = view(
            FolderCheck::Ready(RemovalCheck {
                linked_worktrees: vec![PathBuf::from(r"D:\Repos\Old-feature")],
                ..safe()
            }),
            true,
        );
        let reason = with_worktrees.recycle_blocked().unwrap();
        assert!(reason.contains("Old-feature"), "{reason}");
        assert!(!with_worktrees.can_confirm());

        // A linked worktree itself may go; git's record of it is pruned afterwards.
        let linked = view(
            FolderCheck::Ready(RemovalCheck {
                main_worktree: Some(PathBuf::from(r"D:\Repos\Main")),
                ..safe()
            }),
            true,
        );
        assert!(linked.recycle_blocked().is_none() && linked.can_confirm());
    }

    #[test]
    fn health_tells_missing_from_not_a_repository() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            RepoHealth::of(&temp.path().join("gone")),
            RepoHealth::Missing
        );
        assert_eq!(RepoHealth::of(temp.path()), RepoHealth::NotARepository);
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        assert_eq!(RepoHealth::of(temp.path()), RepoHealth::Fine);

        let bare = temp.path().join("bare.git");
        std::fs::create_dir_all(bare.join("objects")).unwrap();
        std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(RepoHealth::of(&bare), RepoHealth::Fine);
    }
}
