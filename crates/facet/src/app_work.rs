//! Working copy behavior of [`FacetApp`]: status refreshes, diff loading, staging, commit,
//! discard and undo. Every git call runs on the tokio runtime and comes back as a
//! [`WorkEvent`]; this file only starts work and applies results.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use facet_git::parse::{CommitSummary, EntryKind, Status, StatusEntry};
use facet_git::{
    ApplyTo, CancellationToken, CommitOutcome, DiscardFile, Git, GitError, Head, PatchDirection,
    PerfConfig, RepoInfo, UndoEntry,
};

use crate::actions::Action;
use crate::app::{describe_error, Event, FacetApp, RepoTab, TabState};
use crate::app_repo::RepoEvent;
use crate::highlight::{decorate, display_text, Decorations};
use crate::loader::{self, DiffPayload, LoadContext};
use crate::settings::same_path;
use crate::theme::Palette;
use crate::ui::confirm::Confirm;
use crate::watcher::{self, FsChange, IndexStamp};
use crate::workspace::{Area, DiffBody, DiffKey, DiffView, TextDiff, Workspace};

/// The warm status budget from SPEC §2.4; refreshes slower than this count as slow.
const STATUS_BUDGET_MS: u32 = 150;
/// Undo entries are kept this long / this many (see `facet_git::prune_undo`).
const UNDO_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const UNDO_KEEP: usize = 50;

pub struct StatusSnapshot {
    pub status: Status,
    /// The commit at `HEAD`, when it is not the one the tab already knows.
    pub summary: Option<CommitSummary>,
    /// Lines added / removed per path (empty when there are too many files to count).
    pub counts: std::collections::HashMap<String, (u32, u32)>,
}

/// Line counts are shown for up to this many changed files.
const MAX_COUNTED_FILES: usize = 400;

/// Results of background work for one repository tab.
pub enum WorkEvent {
    Status {
        epoch: u64,
        elapsed_ms: u32,
        result: Result<StatusSnapshot, GitError>,
    },
    Diff {
        slot: DiffSlot,
        generation: u64,
        result: Result<DiffPayload, GitError>,
    },
    Decorated {
        slot: DiffSlot,
        generation: u64,
        decorations: Decorations,
    },
    /// A stage / unstage / apply operation finished.
    Changed {
        what: &'static str,
        result: Result<(), GitError>,
    },
    Committed {
        amend: bool,
        result: Result<CommitOutcome, GitError>,
    },
    Discarded(Result<UndoEntry, GitError>),
    Undone {
        consumed: String,
        result: Result<UndoEntry, GitError>,
    },
    AmendMessage(Result<Option<String>, GitError>),
    Perf(Result<PerfConfig, GitError>),
    PerfSet {
        enabled: bool,
        result: Result<(), GitError>,
    },
    UndoList(Vec<UndoEntry>),
    Fs(FsChange),
    /// History, branches, remotes, stashes (see `app_repo.rs`).
    Repo(RepoEvent),
}

/// Which diff pane a load belongs to: the working copy's, the history's commit detail, or
/// the stash view's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSlot {
    Work,
    Commit,
    Stash,
}

pub(crate) fn diff_view(tab: &RepoTab, slot: DiffSlot) -> &DiffView {
    match slot {
        DiffSlot::Work => &tab.work.diff,
        DiffSlot::Commit => &tab.history.commit.diff,
        DiffSlot::Stash => &tab.stash.diff,
    }
}

pub(crate) fn diff_view_mut(tab: &mut RepoTab, slot: DiffSlot) -> &mut DiffView {
    match slot {
        DiffSlot::Work => &mut tab.work.diff,
        DiffSlot::Commit => &mut tab.history.commit.diff,
        DiffSlot::Stash => &mut tab.stash.diff,
    }
}

/// Append a line to the file named by `FACET_PERF_LOG` (profiling aid; off by default).
pub(crate) fn perf_log(line: &str) {
    static TARGET: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    let target = TARGET.get_or_init(|| std::env::var_os("FACET_PERF_LOG").map(Into::into));
    if let Some(path) = target {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
    }
}

pub(crate) fn ready_info(tab: &RepoTab) -> Option<&RepoInfo> {
    match &tab.state {
        TabState::Ready { info, .. } if !info.is_bare() => Some(info),
        _ => None,
    }
}

impl FacetApp {
    pub(crate) fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// The active tab's workspace, if it has a working tree and finished loading.
    pub fn active_work(&self) -> Option<&Workspace> {
        let tab = self.active_tab()?;
        ready_info(tab).map(|_| &tab.work)
    }

    pub fn active_work_mut(&mut self) -> Option<&mut Workspace> {
        let tab = self.tabs.get_mut(self.active?)?;
        ready_info(tab)?;
        Some(&mut tab.work)
    }

    /// Run `work` for a tab on the runtime; its result (plus the index stamp afterwards, for
    /// work that may write the index) comes back as an event.
    pub(crate) fn spawn_work(
        &self,
        tab: &RepoTab,
        stamp_after: bool,
        work: impl Future<Output = WorkEvent> + Send + 'static,
    ) {
        let id = tab.id;
        let git_dir = ready_info(tab).map(|info| info.git_dir.clone());
        self.spawn(async move {
            let event = work.await;
            let stamp = match (stamp_after, git_dir) {
                (true, Some(git_dir)) => Some(watcher::index_stamp(&git_dir)),
                _ => None,
            };
            Event::Work {
                tab: id,
                stamp,
                event,
            }
        });
    }

    // ----- lifecycle --------------------------------------------------------------------------

    /// Called once when a tab's repository has been identified.
    pub(crate) fn start_workspace(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(info) = ready_info(tab).cloned() else {
            return;
        };
        let id = tab.id;

        // File system watcher → debounced `Fs` events.
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let watcher = watcher::watch(&info, self.runtime.handle(), move |change| {
            let event = Event::Work {
                tab: id,
                stamp: None,
                event: WorkEvent::Fs(change),
            };
            if tx.send(event).is_ok() {
                ctx.request_repaint();
            }
        });
        match watcher {
            Ok(watcher) => self.tabs[index].watcher = Some(watcher),
            Err(error) => self.error_toast(
                "File changes can't be watched",
                Some(format!("{error}\nPress F5 to refresh manually.")),
            ),
        }

        // Branches, tags, remotes and stashes for the sidebar (and the history's badges).
        self.request_refs(index);

        let tab = &self.tabs[index];
        let git = self.git.clone();
        let root = info.root().to_owned();
        self.spawn_work(tab, false, async move {
            WorkEvent::Perf(facet_git::perf_config(&git, &root).await)
        });
        let undo_info = info.clone();
        self.spawn_work(tab, false, async move {
            facet_git::prune_undo(&undo_info, UNDO_KEEP, UNDO_MAX_AGE).await;
            WorkEvent::UndoList(facet_git::list_undo(&undo_info).await)
        });
        self.request_status(index, true);
    }

    // ----- status -----------------------------------------------------------------------------

    /// Refresh the status of a tab. `foreground` refreshes may take git's optional locks
    /// (used after Facet's own operations and for explicit refreshes); automatic ones never.
    pub(crate) fn request_status(&mut self, index: usize, foreground: bool) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let TabState::Ready { info, head } = &tab.state else {
            return;
        };
        if info.is_bare() {
            return;
        }
        // Explicit refreshes and Facet's own operations reload the open diff as well;
        // automatic ones only when the watcher saw that file change (`on_fs_change`).
        if foreground {
            tab.work.diff_stale = true;
        }
        let refresh = &mut tab.work.refresh;
        if refresh.running {
            refresh.pending = Some(refresh.pending.unwrap_or(false) || foreground);
            return;
        }
        refresh.running = true;
        let epoch = refresh.epoch;
        let known_oid = head.commit().map(|commit| commit.oid.clone());
        let git = self.git.clone();
        let root = info.root().to_owned();
        let tab = &self.tabs[index];
        self.spawn_work(tab, true, async move {
            let started = Instant::now();
            let status = facet_git::status(&git, &root, !foreground, None).await;
            let elapsed_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
            let result = match status {
                Ok(status) => {
                    // Only ask for the commit when HEAD moved (and exists: G13).
                    let summary = match &status.branch.oid {
                        Some(oid) if Some(oid) != known_oid.as_ref() => {
                            match facet_git::commit_summary(&git, &root).await {
                                Ok(summary) => Ok(Some(summary)),
                                Err(error) => Err(error),
                            }
                        }
                        _ => Ok(None),
                    };
                    let counts = if status.entries.len() <= MAX_COUNTED_FILES {
                        let untracked: Vec<String> = status
                            .entries
                            .iter()
                            .filter(|entry| entry.kind == facet_git::parse::EntryKind::Untracked)
                            .map(|entry| entry.path.clone())
                            .collect();
                        let has_head = status.branch.oid.is_some();
                        facet_git::line_counts(&git, &root, has_head, &untracked)
                            .await
                            .unwrap_or_default()
                    } else {
                        std::collections::HashMap::new()
                    };
                    summary.map(|summary| StatusSnapshot {
                        status,
                        summary,
                        counts,
                    })
                }
                Err(error) => Err(error),
            };
            WorkEvent::Status {
                epoch,
                elapsed_ms,
                result,
            }
        });
    }

    fn on_status(
        &mut self,
        index: usize,
        epoch: u64,
        elapsed_ms: u32,
        result: Result<StatusSnapshot, GitError>,
    ) {
        let tab = &mut self.tabs[index];
        tab.work.refresh.running = false;
        if epoch != tab.work.refresh.epoch {
            // Facet changed the repository after this status started: it is already stale.
            tab.work.refresh.pending = None;
            self.request_status(index, true);
            return;
        }
        match result {
            Ok(snapshot) => {
                perf_log(&format!(
                    "status: {elapsed_ms} ms, {} entries\n",
                    snapshot.status.entries.len()
                ));
                if let TabState::Ready { head, .. } = &mut tab.state {
                    let branch = &snapshot.status.branch;
                    let commit = match (&branch.oid, snapshot.summary) {
                        (None, _) => None,
                        (Some(_), Some(summary)) => Some(summary),
                        (Some(_), None) => head.commit().cloned(),
                    };
                    *head = match (&branch.head, commit) {
                        (Some(name), commit) => Head::Branch {
                            name: name.clone(),
                            commit,
                        },
                        (None, Some(commit)) => Head::Detached { commit },
                        // Detached with an unknown commit cannot happen; keep what we had.
                        (None, None) => head.clone(),
                    };
                }
                let perf = &mut tab.work.perf;
                perf.last_ms = Some(elapsed_ms);
                perf.slow_refreshes = if elapsed_ms > STATUS_BUDGET_MS {
                    perf.slow_refreshes + 1
                } else {
                    0
                };
                tab.work.apply_status(snapshot.status);
                tab.work.counts = snapshot.counts;
                // A different current file always loads; the same one only when stale.
                let stale = std::mem::take(&mut tab.work.diff_stale);
                self.sync_diff(index, stale);
                self.request_lfs_attributes(index);
            }
            Err(GitError::Cancelled) => {}
            Err(error) => {
                let (title, detail) = describe_error(&error);
                self.error_toast(title, detail);
            }
        }
        if let Some(foreground) = self.tabs[index].work.refresh.pending.take() {
            self.request_status(index, foreground);
        }
    }

    fn on_fs_change(&mut self, index: usize, change: FsChange) {
        // Facet's own writes are followed by a refresh anyway; so is an index write whose
        // result we have already seen.
        if self.tabs[index].work.busy > 0 {
            return;
        }
        // Branches, tags, stashes or HEAD moved (a commit, a fetch, another tool).
        if change.refs {
            self.request_refs(index);
        }
        let tab = &self.tabs[index];
        let own_index_write = change.index
            && !change.worktree
            && !change.refs
            && change.index_stamp == tab.index_stamp;
        if own_index_write {
            return;
        }
        // Targeted: the open diff reloads only if this batch touched its file or the index.
        let touches_diff = tab.work.diff.key.as_ref().is_some_and(|key| {
            change.index
                || change.touches(&key.request.path)
                || key
                    .request
                    .orig_path
                    .as_deref()
                    .is_some_and(|orig| change.touches(orig))
        });
        if touches_diff {
            self.tabs[index].work.diff_stale = true;
        }
        self.request_status(index, false);
    }

    // ----- diff -------------------------------------------------------------------------------

    /// Make the diff pane show the current file. With `quiet`, a reload of the same file
    /// keeps what is on screen until the new result arrives (and keeps it for good if the
    /// result is identical).
    pub(crate) fn sync_diff(&mut self, index: usize, quiet: bool) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(info) = ready_info(tab).cloned() else {
            return;
        };
        let key = tab.work.current_key();
        let same = key == tab.work.diff.key;
        if same && !quiet {
            return;
        }
        let view = &mut tab.work.diff;
        if let Some(cancel) = view.cancel.take() {
            cancel.cancel();
        }
        view.generation += 1;
        let Some(key) = key else {
            view.key = None;
            view.body = DiffBody::None;
            return;
        };
        if !same {
            view.body = DiffBody::Loading;
            view.reset_scroll = true;
            view.key = Some(key.clone());
        }
        let cancel = CancellationToken::new();
        view.cancel = Some(cancel.clone());
        let generation = view.generation;

        let (head_oid, index_oid, submodule) = tab
            .work
            .current()
            .map(|(_, entry)| {
                (
                    entry.head_oid.clone(),
                    entry.index_oid.clone(),
                    entry.submodule,
                )
            })
            .unwrap_or_default();
        let cx = LoadContext {
            git: self.git.clone(),
            info,
            request: key.request,
            head_oid,
            index_oid,
            ctx: self.ctx.clone(),
            cancel,
            submodule,
        };
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let started = Instant::now();
            let result = tokio::select! {
                () = cx.cancel.cancelled() => Err(GitError::Cancelled),
                result = loader::load(&cx) => result,
            };
            if let Ok(DiffPayload::Text(diff)) = &result {
                perf_log(&format!(
                    "diff: {} ms, {} bytes, {} lines ({})\n",
                    started.elapsed().as_millis(),
                    diff.data.len(),
                    diff.lines.len(),
                    cx.request.path
                ));
            }
            WorkEvent::Diff {
                slot: DiffSlot::Work,
                generation,
                result,
            }
        });
    }

    pub(crate) fn on_diff(
        &mut self,
        index: usize,
        slot: DiffSlot,
        generation: u64,
        result: Result<DiffPayload, GitError>,
    ) {
        let tab = &mut self.tabs[index];
        let view = diff_view_mut(tab, slot);
        if generation != view.generation {
            return;
        }
        view.cancel = None;
        let Some(key) = view.key.clone() else {
            return;
        };
        match result {
            Ok(DiffPayload::Text(diff)) => {
                // An identical reload changes nothing: keep selection, colors and scroll.
                if let Some(text) = view.text().filter(|text| text.diff.data == diff.data) {
                    // The reload cancelled a decoration still in flight: start it again.
                    if text.decorations.is_none() {
                        self.decorate(index, slot);
                    }
                    return;
                }
                let view = diff_view_mut(&mut self.tabs[index], slot);
                let diff = Arc::new(diff);
                if diff.hunks.is_empty() {
                    view.body = DiffBody::Unchanged(diff);
                    return;
                }
                // Renames and conflicts are staged per file; everything else per line.
                // Commits of the history are read-only.
                let partial = key.request.orig_path.is_none()
                    && !matches!(
                        key.request.source,
                        facet_git::DiffSource::Conflict | facet_git::DiffSource::Commit { .. }
                    );
                view.body = DiffBody::Text(TextDiff::new(diff, partial));
                self.decorate(index, slot);
            }
            Ok(DiffPayload::Binary(info)) => view.body = DiffBody::Binary(info),
            Ok(DiffPayload::Image(images)) => view.body = DiffBody::Image(images),
            Ok(DiffPayload::Submodule(range)) => view.body = DiffBody::Submodule(range),
            Err(GitError::Cancelled) => {}
            Err(error) => {
                view.body = DiffBody::Failed(describe_error(&error).1.unwrap_or_default())
            }
        }
    }

    /// Compute syntax colors and word-level highlights for the text diff on a worker.
    pub(crate) fn decorate(&mut self, index: usize, slot: DiffSlot) {
        let dark = Palette::of(&self.ctx).dark;
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let view = diff_view(tab, slot);
        let (Some(text), Some(key), Some(_)) = (view.text(), &view.key, ready_info(tab)) else {
            return;
        };
        let diff = text.diff.clone();
        let path = key.request.path.clone();
        let generation = view.generation;
        // Opening another file cancels the token (like a diff load), which stops the worker
        // between hunks; the generation check drops anything that still arrives late.
        let cancel = CancellationToken::new();
        let worker = cancel.clone();
        self.spawn_work(tab, false, async move {
            let started = Instant::now();
            let lines = diff.lines.len();
            let decorations =
                tokio::task::spawn_blocking(move || decorate(&diff, &path, dark, &worker))
                    .await
                    .unwrap_or_default();
            perf_log(&format!(
                "decorate: {} ms, {lines} lines\n",
                started.elapsed().as_millis()
            ));
            WorkEvent::Decorated {
                slot,
                generation,
                decorations,
            }
        });
        if let Some(tab) = self.tabs.get_mut(index) {
            if let Some(old) = diff_view_mut(tab, slot).cancel.replace(cancel) {
                old.cancel();
            }
        }
    }

    // ----- operations that change the repository ---------------------------------------------

    /// Run an index-changing git operation for the active tab. Operations of one repository
    /// run strictly one after another (they would fight over `index.lock` otherwise).
    fn run_op<F, Fut>(&mut self, what: &'static str, op: F)
    where
        F: FnOnce(Git, RepoInfo) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), GitError>> + Send,
    {
        let Some(index) = self.active else {
            return;
        };
        let Some(info) = self.tabs.get(index).and_then(ready_info).cloned() else {
            return;
        };
        let tab = &mut self.tabs[index];
        tab.work.busy += 1;
        let lock = tab.ops.clone();
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, true, async move {
            let _serialized = lock.lock().await;
            WorkEvent::Changed {
                what,
                result: op(git, info).await,
            }
        });
    }

    /// Bookkeeping shared by every finished operation: the repository changed under any
    /// status still in flight, so bump the epoch and refresh.
    pub(crate) fn op_finished(&mut self, index: usize) {
        let work = &mut self.tabs[index].work;
        work.busy = work.busy.saturating_sub(1);
        work.refresh.epoch += 1;
        self.request_status(index, true);
    }

    fn selected_paths(&self, area: Area) -> Vec<String> {
        let Some(work) = self
            .active_work()
            .filter(|work| work.selection.area == area)
        else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        for entry in work.selected_entries() {
            paths.push(entry.path.clone());
            // Unstaging a rename has to reset its origin as well.
            if area == Area::Staged {
                paths.extend(entry.orig_path.clone());
            }
        }
        paths
    }

    pub(crate) fn stage_paths(&mut self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        self.run_op("stage", move |git, info| async move {
            facet_git::stage_paths(&git, info.root(), &paths).await
        });
    }

    pub(crate) fn unstage_paths(&mut self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        self.run_op("unstage", move |git, info| async move {
            facet_git::unstage_paths(&git, info.root(), &paths).await
        });
    }

    /// Build a patch from the diff on screen: one hunk, or the selected lines.
    fn diff_patch(
        &mut self,
        hunk: Option<usize>,
        direction: PatchDirection,
    ) -> Option<(Vec<u8>, DiffKey, usize)> {
        let work = self.active_work()?;
        let key = work.diff.key.clone()?;
        let text = work.diff.text().filter(|text| text.partial)?;
        let diff = &text.diff;
        let lines: Vec<usize> = match hunk {
            Some(hunk) => {
                let range = diff.hunks.get(hunk)?.lines.clone();
                (range.start as usize..range.end as usize)
                    .filter(|line| text.is_changed_line(*line))
                    .collect()
            }
            None => text.selected.iter().map(|line| *line as usize).collect(),
        };
        let count = lines.len();
        let built = facet_git::build_patch(diff, &key.request.path, direction, |line| {
            lines.binary_search(&line).is_ok()
        });
        match built {
            Ok(patch) => Some((patch, key, count)),
            Err(error) => {
                self.error_toast(
                    "These lines can't be applied on their own",
                    Some(error.to_string()),
                );
                None
            }
        }
    }

    fn clear_line_selection(&mut self) {
        if let Some(text) = self.active_work_mut().and_then(|work| work.diff.text_mut()) {
            text.selected.clear();
            text.drag = None;
        }
    }

    /// Stage (from "Changes") or unstage (from "Staged") a hunk or the selected lines.
    fn apply_partial(&mut self, hunk: Option<usize>, stage: bool) {
        let Some(area) = self
            .active_work()
            .and_then(|w| w.diff.key.as_ref().map(|k| k.area))
        else {
            return;
        };
        let (expected, direction, target, what) = if stage {
            (
                Area::Unstaged,
                PatchDirection::Forward,
                ApplyTo::Index,
                "stage",
            )
        } else {
            (
                Area::Staged,
                PatchDirection::Reverse,
                ApplyTo::IndexReverse,
                "unstage",
            )
        };
        if area != expected {
            return;
        }
        let Some((patch, _, _)) = self.diff_patch(hunk, direction) else {
            return;
        };
        self.clear_line_selection();
        self.run_op(what, move |git, info| async move {
            facet_git::apply_patch(&git, info.root(), &patch, target).await
        });
    }

    /// Ask before discarding a hunk or the selected lines.
    fn ask_discard_partial(&mut self, hunk: Option<usize>) {
        if self
            .active_work()
            .and_then(|w| w.diff.key.as_ref().map(|k| k.area))
            != Some(Area::Unstaged)
        {
            return;
        }
        let Some((patch, key, count)) = self.diff_patch(hunk, PatchDirection::Reverse) else {
            return;
        };
        let name = file_name(&key.request.path);
        let what = match (hunk, count) {
            (Some(_), _) => "this hunk".to_owned(),
            (None, 1) => "the selected line".to_owned(),
            (None, count) => format!("the {count} selected lines"),
        };
        let label = match hunk {
            Some(_) => format!("a hunk in {name}"),
            None => format!("{count} {} in {name}", plural(count, "line", "lines")),
        };
        self.confirm = Some(Confirm {
            title: format!("Discard {what}?"),
            body: format!("The change is removed from {name}. You can undo this."),
            confirm_label: "Discard".to_owned(),
            action: Action::ConfirmedDiscardPatch { patch, label },
        });
    }

    /// Ask before discarding whole files.
    fn ask_discard_files(&mut self, entries: Vec<StatusEntry>) {
        let total = entries.len();
        let files: Vec<DiscardFile> = entries
            .iter()
            .filter(|entry| entry.kind != EntryKind::Unmerged && !entry.submodule)
            .map(|entry| DiscardFile {
                path: entry.path.clone(),
                tracked: entry.kind == EntryKind::Tracked,
            })
            .collect();
        if files.is_empty() {
            if total > 0 {
                self.info_toast("Conflicted files and submodules can't be discarded here");
            }
            return;
        }
        let untracked = files.iter().filter(|file| !file.tracked).count();
        let (title, label) = if files.len() == 1 {
            let name = file_name(&files[0].path);
            (format!("Discard changes to {name}?"), name)
        } else {
            (
                format!("Discard changes to {} files?", files.len()),
                format!("{} files", files.len()),
            )
        };
        let mut body = String::from("Files return to their staged or committed version.");
        if untracked > 0 {
            body.push_str(&format!(
                " {untracked} new {} will be removed.",
                plural(untracked, "file", "files")
            ));
        }
        if files.len() < total {
            body.push_str(" Conflicted files and submodules are skipped.");
        }
        body.push_str(" You can undo this.");
        self.confirm = Some(Confirm {
            title,
            body,
            confirm_label: "Discard".to_owned(),
            action: Action::ConfirmedDiscardFiles { files, label },
        });
    }

    fn discard<F, Fut>(&mut self, op: F)
    where
        F: FnOnce(Git, RepoInfo) -> Fut + Send + 'static,
        Fut: Future<Output = Result<UndoEntry, GitError>> + Send,
    {
        let Some(index) = self.active else {
            return;
        };
        let Some(info) = self.tabs.get(index).and_then(ready_info).cloned() else {
            return;
        };
        let tab = &mut self.tabs[index];
        tab.work.busy += 1;
        let lock = tab.ops.clone();
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, true, async move {
            let _serialized = lock.lock().await;
            WorkEvent::Discarded(op(git, info).await)
        });
    }

    fn undo_discard(&mut self) {
        let Some(index) = self.active else {
            return;
        };
        let Some(info) = self.tabs.get(index).and_then(ready_info).cloned() else {
            return;
        };
        let tab = &mut self.tabs[index];
        let Some(entry) = tab.work.last_undo().cloned() else {
            return;
        };
        tab.work.busy += 1;
        let lock = tab.ops.clone();
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, true, async move {
            let _serialized = lock.lock().await;
            WorkEvent::Undone {
                consumed: entry.id.clone(),
                result: facet_git::undo(&git, &info, &entry).await,
            }
        });
    }

    // ----- commit -----------------------------------------------------------------------------

    pub fn can_commit(&self) -> bool {
        self.active_work().is_some_and(|work| {
            let draft = &work.commit;
            !draft.running
                && !draft.message.trim().is_empty()
                && (!work.staged.is_empty() || draft.amend)
                && (!draft.amend || work.branch.oid.is_some())
        })
    }

    fn commit(&mut self) {
        if !self.can_commit() {
            return;
        }
        let Some(index) = self.active else {
            return;
        };
        let Some(info) = self.tabs.get(index).and_then(ready_info).cloned() else {
            return;
        };
        let tab = &mut self.tabs[index];
        let draft = &mut tab.work.commit;
        draft.running = true;
        draft.error = None;
        let (message, amend) = (draft.message.clone(), draft.amend);
        tab.work.busy += 1;
        let lock = tab.ops.clone();
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, true, async move {
            let _serialized = lock.lock().await;
            WorkEvent::Committed {
                amend,
                result: facet_git::commit(&git, info.root(), &message, amend).await,
            }
        });
    }

    fn toggle_amend(&mut self) {
        let Some(index) = self.active else {
            return;
        };
        let Some(info) = self.tabs.get(index).and_then(ready_info).cloned() else {
            return;
        };
        let tab = &mut self.tabs[index];
        if tab.work.branch.oid.is_none() {
            return; // nothing to amend yet
        }
        let draft = &mut tab.work.commit;
        draft.amend = !draft.amend;
        if !draft.amend {
            // Drop the pre-filled message again if it was not edited.
            if draft.amend_original.as_deref() == Some(draft.message.as_str()) {
                draft.message.clear();
            }
            draft.amend_original = None;
            return;
        }
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            WorkEvent::AmendMessage(facet_git::last_commit_message(&git, info.root()).await)
        });
    }

    fn set_perf(&mut self, enable: bool) {
        let Some(index) = self.active else {
            return;
        };
        let Some(info) = self.tabs.get(index).and_then(ready_info).cloned() else {
            return;
        };
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            WorkEvent::PerfSet {
                enabled: enable,
                result: facet_git::set_perf_config(&git, info.root(), enable).await,
            }
        });
    }

    // ----- events -----------------------------------------------------------------------------

    pub(crate) fn handle_work_event(
        &mut self,
        id: u64,
        stamp: Option<IndexStamp>,
        event: WorkEvent,
    ) {
        let Some(index) = self.tab_index(id) else {
            return; // the tab was closed meanwhile
        };
        if let Some(stamp) = stamp {
            self.tabs[index].index_stamp = stamp;
        }
        match event {
            WorkEvent::Status {
                epoch,
                elapsed_ms,
                result,
            } => self.on_status(index, epoch, elapsed_ms, result),
            WorkEvent::Diff {
                slot,
                generation,
                result,
            } => self.on_diff(index, slot, generation, result),
            WorkEvent::Decorated {
                slot,
                generation,
                decorations,
            } => {
                let view = diff_view_mut(&mut self.tabs[index], slot);
                if generation == view.generation {
                    if let Some(text) = view.text_mut() {
                        if decorations.syntax.len() == text.diff.lines.len() {
                            text.decorations = Some(Arc::new(decorations));
                        }
                    }
                }
            }
            WorkEvent::Changed { what, result } => {
                if let Err(error) = result {
                    let detail = describe_error(&error).1;
                    self.error_toast(format!("Couldn't {what} the changes"), detail);
                }
                self.op_finished(index);
            }
            WorkEvent::Committed { amend, result } => {
                let draft = &mut self.tabs[index].work.commit;
                draft.running = false;
                match result {
                    Ok(outcome) if outcome.success => {
                        let subject = draft.message.lines().next().unwrap_or("").to_owned();
                        draft.message.clear();
                        draft.amend = false;
                        draft.amend_original = None;
                        draft.error = None;
                        let verb = if amend { "Amended" } else { "Committed" };
                        self.info_toast(format!("{verb}: {subject}"));
                        // The branch moved: the history and its badges follow.
                        self.request_refs(index);
                    }
                    Ok(outcome) => {
                        draft.error = Some(if outcome.output.is_empty() {
                            "git commit failed without a message.".to_owned()
                        } else {
                            outcome.output
                        });
                    }
                    Err(error) => draft.error = Some(error.to_string()),
                }
                self.op_finished(index);
            }
            WorkEvent::Discarded(result) => {
                match result {
                    Ok(entry) => {
                        let title = format!("Discarded {}", entry.label);
                        self.tabs[index].work.undo.push(entry);
                        self.action_toast(title, "Undo", Action::UndoDiscard);
                    }
                    Err(error) => {
                        let detail = describe_error(&error).1;
                        self.error_toast("Couldn't discard the changes", detail);
                    }
                }
                self.op_finished(index);
            }
            WorkEvent::Undone { consumed, result } => {
                match result {
                    Ok(entry) => {
                        let work = &mut self.tabs[index].work;
                        work.undo.retain(|existing| existing.id != consumed);
                        // After an undo the new entry redoes the discard, and vice versa.
                        let (title, button) = if entry.redo {
                            (format!("Restored {}", entry.label), "Redo")
                        } else {
                            (format!("Discarded {} again", entry.label), "Undo")
                        };
                        work.undo.push(entry);
                        self.action_toast(title, button, Action::UndoDiscard);
                    }
                    Err(error) => {
                        let detail = describe_error(&error).1;
                        self.error_toast("Couldn't undo the discard", detail);
                    }
                }
                self.op_finished(index);
            }
            WorkEvent::AmendMessage(result) => {
                let draft = &mut self.tabs[index].work.commit;
                if let (true, Ok(Some(message))) = (draft.amend, result) {
                    if draft.message.trim().is_empty() {
                        draft.message.clone_from(&message);
                    }
                    draft.amend_original = Some(message);
                }
            }
            WorkEvent::Perf(result) => {
                if let Ok(config) = result {
                    self.tabs[index].work.perf.config = Some(config);
                }
            }
            WorkEvent::PerfSet { enabled, result } => match result {
                Ok(()) => {
                    let perf = &mut self.tabs[index].work.perf;
                    perf.config = Some(PerfConfig {
                        fsmonitor: enabled,
                        untracked_cache: enabled,
                    });
                    perf.slow_refreshes = 0;
                    self.info_toast(if enabled {
                        "File system monitor and untracked cache enabled for this repository"
                    } else {
                        "File system monitor and untracked cache disabled for this repository"
                    });
                    // A foreground status lets git build its caches right away.
                    self.request_status(index, true);
                }
                Err(error) => {
                    let detail = describe_error(&error).1;
                    self.error_toast("Couldn't change the repository settings", detail);
                }
            },
            WorkEvent::UndoList(entries) => self.tabs[index].work.undo = entries,
            WorkEvent::Fs(change) => self.on_fs_change(index, change),
            WorkEvent::Repo(event) => self.handle_repo_event(index, event),
        }
    }

    // ----- actions ----------------------------------------------------------------------------

    /// Handle the working copy actions. Returns the action back if it is not one of them.
    pub(crate) fn run_work_action(&mut self, action: Action) -> Option<Action> {
        match action {
            Action::SelectFile {
                area,
                index,
                toggle,
                extend,
            } => {
                if let Some(work) = self.active_work_mut() {
                    work.click(area, index, toggle, extend);
                }
                self.sync_active_diff();
            }
            Action::SelectNextFile | Action::SelectPreviousFile => {
                let step = if action == Action::SelectNextFile {
                    1
                } else {
                    -1
                };
                if let Some(work) = self.active_work_mut() {
                    work.move_cursor(step, false);
                }
                self.sync_active_diff();
            }
            Action::ExtendFileSelection(step) => {
                if let Some(work) = self.active_work_mut() {
                    work.move_cursor(step, true);
                }
                self.sync_active_diff();
            }
            Action::SelectAllFiles => {
                if let Some(work) = self.active_work_mut() {
                    work.select_all();
                }
                self.sync_active_diff();
            }
            Action::StageSelected => {
                let paths = self.selected_paths(Area::Unstaged);
                self.stage_paths(paths);
            }
            Action::UnstageSelected => {
                let paths = self.selected_paths(Area::Staged);
                self.unstage_paths(paths);
            }
            Action::ToggleStageSelected => {
                match self.active_work().map(|work| work.selection.area) {
                    Some(Area::Unstaged) => return self.run_work_action(Action::StageSelected),
                    Some(Area::Staged) => return self.run_work_action(Action::UnstageSelected),
                    None => {}
                }
            }
            Action::StagePaths(paths) => self.stage_paths(paths),
            Action::UnstagePaths(paths) => self.unstage_paths(paths),
            Action::StageAll => {
                if self
                    .active_work()
                    .is_some_and(|work| !work.unstaged.is_empty())
                {
                    self.run_op("stage", |git, info| async move {
                        facet_git::stage_all(&git, info.root()).await
                    });
                }
            }
            Action::UnstageAll => {
                if self
                    .active_work()
                    .is_some_and(|work| !work.staged.is_empty())
                {
                    self.run_op("unstage", |git, info| async move {
                        facet_git::unstage_all(&git, info.root()).await
                    });
                }
            }
            Action::DiscardSelected => {
                let entries: Vec<StatusEntry> = self
                    .active_work()
                    .filter(|work| work.selection.area == Area::Unstaged)
                    .map(|work| work.selected_entries().into_iter().cloned().collect())
                    .unwrap_or_default();
                self.ask_discard_files(entries);
            }
            Action::DiscardAll => {
                let entries: Vec<StatusEntry> = self
                    .active_work()
                    .map(|work| {
                        (0..work.unstaged.len())
                            .filter_map(|i| work.entry(Area::Unstaged, i).cloned())
                            .collect()
                    })
                    .unwrap_or_default();
                self.ask_discard_files(entries);
            }
            Action::DiscardPath(path) => {
                let entries: Vec<StatusEntry> = self
                    .active_work()
                    .and_then(|work| {
                        (0..work.unstaged.len())
                            .filter_map(|i| work.entry(Area::Unstaged, i))
                            .find(|entry| entry.path == path)
                            .cloned()
                    })
                    .into_iter()
                    .collect();
                self.ask_discard_files(entries);
            }
            Action::ConfirmedDiscardFiles { files, label } => {
                self.discard(move |git, info| async move {
                    facet_git::discard_files(&git, &info, &files, &label).await
                });
            }
            Action::ConfirmedDiscardPatch { patch, label } => {
                self.clear_line_selection();
                self.discard(move |git, info| async move {
                    facet_git::discard_patch(&git, &info, &patch, &label).await
                });
            }
            Action::UndoDiscard => self.undo_discard(),
            Action::StageHunk(hunk) => self.apply_partial(Some(hunk), true),
            Action::UnstageHunk(hunk) => self.apply_partial(Some(hunk), false),
            Action::DiscardHunk(hunk) => self.ask_discard_partial(Some(hunk)),
            Action::StageLines => self.apply_partial(None, true),
            Action::UnstageLines => self.apply_partial(None, false),
            Action::DiscardLines => self.ask_discard_partial(None),
            Action::ClearLineSelection => self.clear_line_selection(),
            Action::CopySelectedLines => {
                let text = self
                    .active_work()
                    .and_then(|work| work.diff.text())
                    .map(|text| {
                        let mut copied = String::new();
                        for line in &text.selected {
                            if let Some(line) = text.diff.lines.get(*line as usize) {
                                copied.push_str(&display_text(text.diff.line_bytes(line)).0);
                                copied.push('\n');
                            }
                        }
                        copied
                    });
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    self.ctx.copy_text(text);
                    self.info_toast("Lines copied");
                }
            }
            Action::CopyFilePath => {
                let path = self
                    .active_work()
                    .and_then(|work| work.current())
                    .map(|(_, entry)| entry.path.clone());
                if let Some(path) = path {
                    self.ctx.copy_text(path);
                    self.info_toast("File path copied");
                }
            }
            Action::Commit => self.commit(),
            Action::ToggleAmend => self.toggle_amend(),
            Action::FocusCommitMessage => {
                if let Some(work) = self.active_work_mut() {
                    work.commit.focus_pending = true;
                }
            }
            Action::DismissCommitError => {
                if let Some(work) = self.active_work_mut() {
                    work.commit.error = None;
                }
            }
            Action::SetImageMode(mode) => self.settings.image_mode = mode,
            Action::SetSplitDiff(split) => self.settings.split_diff = split,
            Action::EnablePerf => self.set_perf(true),
            Action::DisablePerf => self.set_perf(false),
            Action::DeclinePerfOffer => {
                if let Some(path) = self.active_tab().map(|tab| tab.path.clone()) {
                    if !self.perf_offer_declined(&path) {
                        self.settings.perf_offer_declined.push(path);
                    }
                }
            }
            other => return Some(other),
        }
        None
    }

    pub fn perf_offer_declined(&self, path: &std::path::Path) -> bool {
        self.settings
            .perf_offer_declined
            .iter()
            .any(|declined| same_path(declined, path))
    }

    fn sync_active_diff(&mut self) {
        if let Some(index) = self.active {
            self.sync_diff(index, false);
        }
    }

    /// Syntax colors depend on the appearance: recompute them when it flips.
    pub(crate) fn redecorate_all(&mut self) {
        for index in 0..self.tabs.len() {
            self.decorate(index, DiffSlot::Work);
            self.decorate(index, DiffSlot::Commit);
            self.decorate(index, DiffSlot::Stash);
        }
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

pub fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 {
        one
    } else {
        many
    }
}
