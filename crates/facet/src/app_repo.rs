//! History, branches, tags, remotes, stashes and the network for a repository tab (M2).
//! Like `app_work.rs`, everything here only starts background work and applies results.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use facet_git::forge::Forge;
use facet_git::graph::{GraphBuilder, GraphRow};
use facet_git::parse::{ChangedFile, CommitDetail, EntryKind, LfsLock, LogCommit, RefKind};
use facet_git::{
    same_dir, CancellationToken, DiffRequest, DiffSource, Git, GitError, Head, LfsUsage, Network,
    Operation, ProgressFn, PushRequest, RefsSnapshot, RepoInfo, StashFile, StashOptions,
    SwitchTarget,
};

use crate::actions::{Action, RepoContext};
use crate::app::{describe_error, Event, FacetApp, RepoTab, TabState};
use crate::app_power::{PowerEvent, Rewrote};
use crate::app_trees::{native_path, TreeEvent};
use crate::app_work::{diff_view_mut, perf_log, plural, ready_info, DiffSlot, WorkEvent};
use crate::history::{badges, tips, DetailBody, StashBody, View};
use crate::loader::{self, LoadContext};
use crate::platform;
use crate::settings::ExternalTool;
use crate::ui::confirm::Confirm;
use crate::ui::prompt::{Prompt, PromptField, PromptKind};
use crate::workspace::{Area, DiffBody, DiffKey};

/// The first page of the history is small so the list appears at once; later pages are
/// large so a 100k-commit history costs few events.
const FIRST_PAGE: usize = 200;
const PAGE: usize = 5000;
/// Network operations longer than this notify when the window is in the background.
const NOTIFY_AFTER: Duration = Duration::from_secs(5);

/// Results of history, ref and network work for one tab.
pub enum RepoEvent {
    /// Refs, HEAD, the rebase / merge / cherry-pick… in progress, and how LFS is used.
    Refs(Result<(RefsSnapshot, Head, Option<Operation>, LfsUsage), GitError>),
    LogPage {
        generation: u64,
        commits: Vec<LogCommit>,
        rows: Vec<GraphRow>,
        elapsed_ms: u32,
    },
    LogDone {
        generation: u64,
        result: Result<usize, GitError>,
        elapsed_ms: u32,
    },
    Detail {
        generation: u64,
        result: Result<(CommitDetail, Vec<ChangedFile>), GitError>,
    },
    /// The files of the stash shown in the stash view.
    StashFiles {
        generation: u64,
        result: Result<Vec<StashFile>, GitError>,
    },
    /// A branch / tag / remote / stash operation finished.
    Op {
        what: String,
        result: Result<Outcome, GitError>,
    },
    Network {
        job: NetworkJob,
        result: Result<(), GitError>,
        elapsed: Duration,
    },
    /// Rewriting history, search, blame, conflicts (M3, `app_power.rs`).
    Power(PowerEvent),
    /// LFS attributes and locks (M4, `app_trees.rs`).
    Trees(TreeEvent),
}

/// What a finished operation has to say.
pub enum Outcome {
    Done(Option<String>),
    /// Offer to undo it.
    Undo {
        title: String,
        action: Action,
    },
    /// `git switch` refused because local changes would be overwritten.
    SwitchRefused(SwitchTarget),
    /// A cherry-pick, revert, reset or rebase: finished (undoable) or stopped half-way.
    Rewrote(Rewrote),
    /// Done, with a follow-up offered on the toast ("Open" a new worktree).
    Offer {
        title: String,
        button: &'static str,
        action: Action,
    },
    /// An LFS lock or unlock, with the lock list read afterwards (`None`: unreadable).
    Locks {
        title: String,
        undo: Option<Action>,
        locks: Option<Vec<LfsLock>>,
    },
}

/// A network command: one per repository at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkJob {
    Fetch(Option<String>),
    Pull(facet_git::PullMode),
    Push(PushRequest),
    PushTag {
        remote: String,
        tag: String,
    },
    /// `git lfs pull`.
    LfsPull,
    /// `git submodule update --init --recursive [-- path]`.
    UpdateSubmodules(Option<String>),
}

impl NetworkJob {
    /// Changes the working tree and index: waits for (and blocks) other operations.
    fn changes_worktree(&self) -> bool {
        matches!(
            self,
            Self::Pull(_) | Self::LfsPull | Self::UpdateSubmodules(_)
        )
    }

    fn label(&self) -> String {
        match self {
            Self::Fetch(None) => "Fetching".to_owned(),
            Self::Fetch(Some(remote)) => format!("Fetching {remote}"),
            Self::Pull(_) => "Pulling".to_owned(),
            Self::Push(push) => format!("Pushing {} to {}", push.branch, push.remote),
            Self::PushTag { remote, tag } => format!("Pushing tag {tag} to {remote}"),
            Self::LfsPull => "Downloading LFS files".to_owned(),
            Self::UpdateSubmodules(None) => "Updating submodules".to_owned(),
            Self::UpdateSubmodules(Some(path)) => format!("Updating submodule {path}"),
        }
    }

    fn done(&self) -> String {
        match self {
            Self::Fetch(None) => "Fetched all remotes".to_owned(),
            Self::Fetch(Some(remote)) => format!("Fetched {remote}"),
            Self::Pull(_) => "Pulled".to_owned(),
            Self::Push(push) if push.force_with_lease.is_some() => format!(
                "Force-pushed {} to {}/{}",
                push.branch, push.remote, push.remote_branch
            ),
            Self::Push(push) => format!(
                "Pushed {} to {}/{}",
                push.branch, push.remote, push.remote_branch
            ),
            Self::PushTag { remote, tag } => format!("Pushed tag {tag} to {remote}"),
            Self::LfsPull => "Downloaded LFS files".to_owned(),
            Self::UpdateSubmodules(None) => "Updated submodules".to_owned(),
            Self::UpdateSubmodules(Some(path)) => format!("Updated submodule {path}"),
        }
    }

    fn failed(&self) -> &'static str {
        match self {
            Self::Fetch(_) => "Couldn't fetch",
            Self::Pull(_) => "Couldn't pull",
            Self::Push(_) | Self::PushTag { .. } => "Couldn't push",
            Self::LfsPull => "Couldn't download the LFS files",
            Self::UpdateSubmodules(_) => "Couldn't update the submodules",
        }
    }
}

/// The network operation in flight: its label, git's latest progress line, and how to stop it.
pub struct NetworkOp {
    pub label: String,
    pub progress: Arc<Mutex<String>>,
    pub cancel: CancellationToken,
    pub started: Instant,
}

impl NetworkOp {
    pub fn progress(&self) -> String {
        self.progress
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }
}

fn millis(since: Instant) -> u32 {
    since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// The remote an upstream like `origin/feature/x` belongs to: the longest remote name that
/// prefixes it (remote names may contain slashes).
pub fn split_upstream<'a>(upstream: &'a str, remotes: &[String]) -> Option<(&'a str, &'a str)> {
    remotes
        .iter()
        .filter(|remote| {
            upstream.len() > remote.len()
                && upstream.starts_with(remote.as_str())
                && upstream.as_bytes()[remote.len()] == b'/'
        })
        .max_by_key(|remote| remote.len())
        .map(|remote| (&upstream[..remote.len()], &upstream[remote.len() + 1..]))
}

/// Did git refuse to switch because local changes would be overwritten?
fn overwrite_refusal(error: &GitError) -> bool {
    matches!(error, GitError::Failed { stderr, .. }
        if stderr.contains("would be overwritten by checkout")
            || stderr.contains("Please commit your changes or stash them"))
}

impl FacetApp {
    // ----- refs ---------------------------------------------------------------------------

    /// Read branches, tags, remotes and stashes (and HEAD) again.
    pub(crate) fn request_refs(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(info) = ready_info(tab).cloned() else {
            return;
        };
        let root = info.root().to_owned();
        if tab.refs.running {
            tab.refs.pending = true;
            return;
        }
        tab.refs.running = true;
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            // Independent queries run at once (G5).
            let git_dir = info.git_dir.clone();
            let (lfs_root, lfs_dir) = (root.clone(), info.git_dir.clone());
            let (refs, head, operation, lfs) = tokio::join!(
                facet_git::refs(&git, &root),
                facet_git::head(&git, &root),
                tokio::task::spawn_blocking(move || facet_git::operation(&git_dir)),
                tokio::task::spawn_blocking(move || facet_git::lfs_usage(&lfs_root, &lfs_dir)),
            );
            let operation = operation.unwrap_or_default();
            let lfs = lfs.unwrap_or_default();
            WorkEvent::Repo(RepoEvent::Refs(
                refs.and_then(|refs| Ok((refs, head?, operation, lfs))),
            ))
        });
    }

    fn on_refs(
        &mut self,
        index: usize,
        result: Result<(RefsSnapshot, Head, Option<Operation>, LfsUsage), GitError>,
    ) {
        let tab = &mut self.tabs[index];
        tab.refs.running = false;
        match result {
            Ok((snapshot, head, operation, lfs)) => {
                self.on_lfs_usage(index, lfs);
                let tab = &mut self.tabs[index];
                tab.power.operation = operation;
                let head_oid = head.commit().map(|commit| commit.oid.clone());
                let detached = match &head {
                    Head::Detached { commit } => Some(commit.oid.as_str()),
                    Head::Branch { .. } => None,
                };
                tab.refs.badges = badges(&snapshot, detached);
                let current = tips(&snapshot, head_oid.as_deref());
                tab.refs.snapshot = snapshot;
                tab.refs.tips = Some(current.clone());
                tab.refs.loaded = true;
                tab.refs.last_fetch = ready_info(tab).and_then(facet_git::RepoInfo::last_fetch);
                self.sync_shown_stash(index);
                let tab = &mut self.tabs[index];
                // A filtered list stays as it is; leaving it reads the history again.
                let shown =
                    (tab.history.loaded || tab.history.loading) && tab.history.filter.is_none();
                if shown && tab.refs.history_tips.as_ref() != Some(&current) {
                    self.load_history(index);
                }
            }
            Err(GitError::Cancelled) => {}
            Err(error) => {
                let (title, detail) = describe_error(&error);
                self.error_toast(title, detail);
            }
        }
        if std::mem::take(&mut self.tabs[index].refs.pending) {
            self.request_refs(index);
        }
    }

    // ----- history ------------------------------------------------------------------------

    /// Read the whole history in one streamed `git log`, laying out the graph on the worker.
    pub(crate) fn load_history(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let cancel = CancellationToken::new();
        tab.history.first_page_ms = None;
        let generation = tab.history.begin(cancel.clone());
        tab.refs.history_tips = tab.refs.tips.clone();
        let id = tab.id;
        let git = self.git.clone();
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let scope = self.history_scope();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let started = Instant::now();
            let mut builder = GraphBuilder::default();
            let result = facet_git::stream_log_in(&git, &root, scope, &cancel, FIRST_PAGE, PAGE, {
                move |commits: Vec<LogCommit>| {
                    let rows = commits.iter().map(|commit| builder.push(commit)).collect();
                    let event = Event::Work {
                        tab: id,
                        stamp: None,
                        event: WorkEvent::Repo(RepoEvent::LogPage {
                            generation,
                            commits,
                            rows,
                            elapsed_ms: millis(started),
                        }),
                    };
                    if tx.send(event).is_ok() {
                        ctx.request_repaint();
                    }
                }
            })
            .await;
            WorkEvent::Repo(RepoEvent::LogDone {
                generation,
                result,
                elapsed_ms: millis(started),
            })
        });
    }

    pub(crate) fn on_log_done(
        &mut self,
        index: usize,
        generation: u64,
        result: Result<usize, GitError>,
        elapsed_ms: u32,
    ) {
        let tab = &mut self.tabs[index];
        if generation != tab.history.generation {
            return;
        }
        let before = tab.history.selected_oid();
        let first = !tab.history.loaded;
        let filtered = tab.history.filter.is_some();
        tab.history.finish(generation, result.is_ok());
        // A long read leaves its parse buffers freed but committed; hand them back.
        if matches!(result, Ok(count) if count > 10_000) {
            crate::platform::release_free_memory();
        }
        match result {
            Ok(count) if filtered => {
                if let Some(filter) = &tab.history.filter {
                    perf_log(&format!(
                        "{}: {count} commits, first page {} ms, all {elapsed_ms} ms
",
                        filter.kind.title(),
                        tab.history.first_page_ms.unwrap_or(elapsed_ms)
                    ));
                }
                if tab.history.selected.is_none() {
                    tab.history.select(0);
                }
            }
            Ok(count) => {
                tab.history.total_ms = Some(elapsed_ms);
                perf_log(&format!(
                    "history: {count} commits, first page {} ms, all {elapsed_ms} ms\n",
                    tab.history.first_page_ms.unwrap_or(0)
                ));
                // First time: start at the checked-out commit.
                if first && tab.history.selected.is_none() {
                    let head = match &tab.state {
                        TabState::Ready { head, .. } => head.commit().map(|c| c.oid.clone()),
                        TabState::Loading => None,
                    };
                    match head {
                        Some(oid) => {
                            tab.history.select_oid(&oid);
                        }
                        None => {
                            tab.history.select(0);
                        }
                    }
                }
            }
            Err(GitError::Cancelled) => {}
            Err(error) => {
                let (title, detail) = describe_error(&error);
                self.error_toast(title, detail);
            }
        }
        let after = self.tabs[index].history.selected_oid();
        if after != before {
            self.load_detail(index);
        }
    }

    fn select_commit(&mut self, index: usize, row: usize) {
        if self.tabs[index].history.select(row) {
            self.load_detail(index);
        }
    }

    /// Load the selected commit's message and changed files.
    pub(crate) fn load_detail(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let commit = tab.history.selected_commit().cloned();
        let view = &mut tab.history.commit;
        for token in [view.cancel.take(), view.diff.cancel.take()]
            .into_iter()
            .flatten()
        {
            token.cancel();
        }
        view.generation += 1;
        view.diff.generation += 1;
        view.diff.key = None;
        view.diff.body = DiffBody::None;
        view.selected_file = None;
        let Some(commit) = commit else {
            view.oid = None;
            view.body = DetailBody::None;
            return;
        };
        view.oid = Some(commit.oid.to_string());
        view.body = DetailBody::Loading;
        let cancel = CancellationToken::new();
        view.cancel = Some(cancel.clone());
        let generation = view.generation;
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let parent = commit.parents.first().map(ToString::to_string);
            let result = facet_git::commit_detail(
                &git,
                &root,
                &commit.oid.to_string(),
                parent.as_deref(),
                Some(&cancel),
            )
            .await;
            WorkEvent::Repo(RepoEvent::Detail { generation, result })
        });
    }

    fn on_detail(
        &mut self,
        index: usize,
        generation: u64,
        result: Result<(CommitDetail, Vec<ChangedFile>), GitError>,
    ) {
        let view = &mut self.tabs[index].history.commit;
        if generation != view.generation {
            return;
        }
        view.cancel = None;
        match result {
            Ok((detail, files)) => {
                // In a file's history, open that file (under its name at the time); otherwise
                // the commit's page lists its files and one is opened by clicking it.
                let wanted = self.tabs[index].history.filter_path().map(str::to_owned);
                let open = wanted.and_then(|path| files.iter().position(|file| file.path == path));
                let view = &mut self.tabs[index].history.commit;
                view.body = DetailBody::Loaded { detail, files };
                if let Some(file) = open {
                    self.select_commit_file(index, file);
                }
            }
            Err(GitError::Cancelled) => {}
            Err(error) => {
                view.body = DetailBody::Failed(describe_error(&error).1.unwrap_or_default());
            }
        }
    }

    /// Which commits the history lists.
    pub(crate) fn history_scope(&self) -> facet_git::Scope {
        if self.settings.history_all_branches {
            facet_git::Scope::All
        } else {
            facet_git::Scope::Current
        }
    }

    /// Back from a file's diff to the commit's page.
    fn close_commit_file(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let view = &mut tab.history.commit;
        if let Some(token) = view.diff.cancel.take() {
            token.cancel();
        }
        view.diff.generation += 1;
        view.diff.key = None;
        view.diff.body = crate::workspace::DiffBody::None;
        view.selected_file = None;
    }

    /// Show the diff of one file of the selected commit (against its first parent).
    fn select_commit_file(&mut self, index: usize, file: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let view = &mut tab.history.commit;
        let (Some(detail), Some(changed)) = (view.detail(), view.files().get(file)) else {
            return;
        };
        let old = detail.parents.first().cloned();
        let new = detail.oid.clone();
        let changed = changed.clone();
        view.selected_file = Some(file);
        self.load_readonly_diff(index, DiffSlot::Commit, &changed, old, new);
    }

    /// Load the diff of `file` between two commits (`old: None` is the empty tree) into a
    /// read-only diff pane: a commit's or a stash's.
    fn load_readonly_diff(
        &mut self,
        index: usize,
        slot: DiffSlot,
        file: &ChangedFile,
        old: Option<String>,
        new: String,
    ) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(info) = ready_info(tab).cloned() else {
            return;
        };
        let request = DiffRequest {
            path: file.path.clone(),
            orig_path: file.orig_path.clone(),
            source: DiffSource::Commit { old, new },
        };
        let diff = diff_view_mut(tab, slot);
        if let Some(cancel) = diff.cancel.take() {
            cancel.cancel();
        }
        diff.generation += 1;
        diff.body = DiffBody::Loading;
        diff.reset_scroll = true;
        // These diffs are read-only; the area only matters for staging actions.
        diff.key = Some(DiffKey {
            area: Area::Staged,
            request: request.clone(),
        });
        let cancel = CancellationToken::new();
        diff.cancel = Some(cancel.clone());
        let generation = diff.generation;
        let cx = LoadContext {
            git: self.git.clone(),
            info,
            request,
            head_oid: file.old_oid.clone(),
            index_oid: file.new_oid.clone(),
            ctx: self.ctx.clone(),
            cancel,
            submodule: file.submodule,
        };
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let result = tokio::select! {
                () = cx.cancel.cancelled() => Err(GitError::Cancelled),
                result = loader::load(&cx) => result,
            };
            WorkEvent::Diff {
                slot,
                generation,
                result,
            }
        });
    }

    // ----- stashes ------------------------------------------------------------------------

    /// Show the files of the stash at `position` in the list, opening the first one.
    fn show_stash(&mut self, index: usize, position: usize) {
        let Some(entry) = self.tabs[index]
            .refs
            .snapshot
            .stashes
            .get(position)
            .cloned()
        else {
            return;
        };
        self.show_view(View::Stash);
        let tab = &mut self.tabs[index];
        if tab.view != View::Stash {
            return;
        }
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let view = &mut tab.stash;
        // Clicking the stash already shown keeps its selection and scroll.
        if view
            .entry
            .as_ref()
            .is_some_and(|shown| shown.oid == entry.oid)
            && !matches!(view.body, StashBody::Failed(_))
        {
            view.entry = Some(entry);
            return;
        }
        view.cancel();
        view.entry = Some(entry.clone());
        view.body = StashBody::Loading;
        let cancel = CancellationToken::new();
        view.cancel = Some(cancel.clone());
        let generation = view.generation;
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let result = facet_git::stash_files(&git, &root, &entry, Some(&cancel)).await;
            WorkEvent::Repo(RepoEvent::StashFiles { generation, result })
        });
    }

    fn on_stash_files(
        &mut self,
        index: usize,
        generation: u64,
        result: Result<Vec<StashFile>, GitError>,
    ) {
        let view = &mut self.tabs[index].stash;
        if generation != view.generation {
            return;
        }
        view.cancel = None;
        match result {
            Ok(files) => {
                let any = !files.is_empty();
                view.body = StashBody::Loaded(files);
                if any {
                    self.select_stash_file(index, 0);
                }
            }
            Err(GitError::Cancelled) => {}
            Err(error) => {
                view.body = StashBody::Failed(describe_error(&error).1.unwrap_or_default());
            }
        }
    }

    fn select_stash_file(&mut self, index: usize, file: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let view = &mut tab.stash;
        let Some(stashed) = view.files().get(file).cloned() else {
            return;
        };
        view.selected_file = Some(file);
        self.load_readonly_diff(
            index,
            DiffSlot::Stash,
            &stashed.file,
            stashed.old,
            stashed.new,
        );
    }

    /// The stash list was read again: follow the shown stash to its new position, or leave
    /// the stash view when it is gone (popped or dropped).
    fn sync_shown_stash(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        let Some(shown) = tab.stash.entry.as_ref() else {
            return;
        };
        match tab
            .refs
            .snapshot
            .stashes
            .iter()
            .find(|entry| entry.oid == shown.oid)
        {
            Some(entry) => tab.stash.entry = Some(entry.clone()),
            None => {
                tab.stash.cancel();
                tab.stash.entry = None;
                tab.stash.body = StashBody::Loading;
                if tab.view == View::Stash {
                    tab.view = View::Changes;
                }
            }
        }
    }

    pub(crate) fn show_view(&mut self, view: View) {
        // Changes and History are pages like Settings: choosing one leaves Settings.
        self.settings_view = None;
        let Some(index) = self.active else {
            return;
        };
        if self.active_work().is_none() {
            return;
        }
        let tab = &mut self.tabs[index];
        tab.view = view;
        if view == View::History && !tab.history.loaded && !tab.history.loading {
            if !tab.refs.loaded {
                self.request_refs(index);
            }
            self.load_history(index);
        }
    }

    // ----- operations ---------------------------------------------------------------------

    /// Run a branch / tag / remote / stash operation for the active tab, serialized with the
    /// repository's other index-changing operations.
    pub(crate) fn repo_op<F, Fut>(&mut self, what: impl Into<String>, op: F)
    where
        F: FnOnce(Git, PathBuf) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Outcome, GitError>> + Send,
    {
        self.repo_op_info(what, move |git, info: RepoInfo| {
            let root = info.root().to_owned();
            op(git, root)
        });
    }

    /// [`Self::repo_op`] for operations that need the git directory too.
    pub(crate) fn repo_op_info<F, Fut>(&mut self, what: impl Into<String>, op: F)
    where
        F: FnOnce(Git, RepoInfo) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Outcome, GitError>> + Send,
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
        let what = what.into();
        let tab = &self.tabs[index];
        self.spawn_work(tab, true, async move {
            let _serialized = lock.lock().await;
            WorkEvent::Repo(RepoEvent::Op {
                what,
                result: op(git, info).await,
            })
        });
    }

    fn on_op(&mut self, index: usize, what: &str, result: Result<Outcome, GitError>) {
        match result {
            Ok(Outcome::Done(Some(message))) => self.info_toast(message),
            Ok(Outcome::Done(None)) => {}
            Ok(Outcome::Undo { title, action }) => self.action_toast(title, "Undo", action),
            Ok(Outcome::Rewrote(rewrote)) => self.on_rewrote(index, rewrote),
            Ok(Outcome::Offer {
                title,
                button,
                action,
            }) => self.action_toast(title, button, action),
            Ok(Outcome::Locks { title, undo, locks }) => {
                self.on_locks_outcome(index, title, undo, locks);
            }
            Ok(Outcome::SwitchRefused(target)) => {
                let name = match &target {
                    SwitchTarget::Branch(name) => name.clone(),
                    SwitchTarget::Track { local, .. } => local.clone(),
                    SwitchTarget::Detached(oid) => oid.chars().take(9).collect(),
                };
                self.confirm = Some(Confirm {
                    title: format!("Stash your changes and switch to {name}?"),
                    body: "Some of your uncommitted changes would be overwritten by the switch. \
                           Facet can put them in the stash first; apply the stash later to get \
                           them back."
                        .to_owned(),
                    confirm_label: "Stash and Switch".to_owned(),
                    action: Action::StashAndSwitch(target),
                });
            }
            Err(error) => {
                let detail = describe_error(&error).1;
                self.error_toast(format!("Couldn't {what}"), detail);
            }
        }
        self.op_finished(index);
        self.request_refs(index);
    }

    fn switch_to(&mut self, target: SwitchTarget) {
        let what = match &target {
            SwitchTarget::Branch(name) => format!("switch to {name}"),
            SwitchTarget::Track { remote_branch, .. } => format!("check out {remote_branch}"),
            SwitchTarget::Detached(oid) => {
                format!("check out {}", oid.chars().take(9).collect::<String>())
            }
        };
        self.repo_op(what, move |git, root| async move {
            match facet_git::switch(&git, &root, &target).await {
                Ok(()) => Ok(Outcome::Done(None)),
                Err(error) if overwrite_refusal(&error) => Ok(Outcome::SwitchRefused(target)),
                Err(error) => Err(error),
            }
        });
    }

    /// The commit a new branch or tag starts at: the one asked for, else the selected commit
    /// in the history, else HEAD.
    fn start_point(&self, at: Option<String>) -> Option<(String, String)> {
        let tab = self.active_tab()?;
        let describe = |oid: &str| {
            tab.history
                .commits
                .iter()
                .find(|commit| commit.oid == oid)
                .map(|commit| format!("{} {}", commit.short_oid(), commit.subject))
        };
        if let Some(oid) = at {
            let label = describe(&oid).unwrap_or_else(|| oid.chars().take(9).collect());
            return Some((oid, label));
        }
        if tab.view == View::History {
            if let Some(commit) = tab.history.selected_commit() {
                return Some((
                    commit.oid.to_string(),
                    format!("{} {}", commit.short_oid(), commit.subject),
                ));
            }
        }
        let TabState::Ready { head, .. } = &tab.state else {
            return None;
        };
        let commit = head.commit()?;
        Some((
            commit.oid.clone(),
            format!("{} {} (HEAD)", commit.short_oid, commit.subject),
        ))
    }

    // ----- network ------------------------------------------------------------------------

    pub(crate) fn start_network(&mut self, job: NetworkJob) {
        let Some(index) = self.active else {
            return;
        };
        let Some(root) = self
            .tabs
            .get(index)
            .and_then(ready_info)
            .map(|info| info.root().to_owned())
        else {
            return;
        };
        if let Some(running) = &self.tabs[index].network {
            let label = running.label.clone();
            self.info_toast(format!("{label} is still running"));
            return;
        }
        let progress_text = Arc::new(Mutex::new(String::new()));
        let progress: ProgressFn = {
            let text = progress_text.clone();
            let ctx = self.ctx.clone();
            Arc::new(move |line: &str| {
                let line = line.trim();
                if line.is_empty() {
                    return;
                }
                if let Ok(mut text) = text.lock() {
                    line.clone_into(&mut text);
                }
                ctx.request_repaint();
            })
        };
        let cancel = CancellationToken::new();
        // A pull changes the working tree and index: it waits for (and blocks) other
        // operations. Fetch and push only touch refs.
        let pull = job.changes_worktree();
        let tab = &mut self.tabs[index];
        tab.network = Some(NetworkOp {
            label: job.label(),
            progress: progress_text,
            cancel: cancel.clone(),
            started: Instant::now(),
        });
        if pull {
            tab.work.busy += 1;
        }
        let lock = tab.ops.clone();
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, pull, async move {
            let started = Instant::now();
            let _serialized = if pull { Some(lock.lock().await) } else { None };
            let net = Network {
                git: &git,
                root: &root,
                cancel: &cancel,
                progress,
            };
            let result = match &job {
                NetworkJob::Fetch(remote) => facet_git::fetch(&net, remote.as_deref()).await,
                NetworkJob::Pull(mode) => facet_git::pull(&net, *mode).await,
                NetworkJob::Push(request) => facet_git::push(&net, request).await,
                NetworkJob::PushTag { remote, tag } => facet_git::push_tag(&net, remote, tag).await,
                NetworkJob::LfsPull => facet_git::lfs_pull(&net).await,
                NetworkJob::UpdateSubmodules(path) => {
                    facet_git::update_submodules(&net, path.as_deref()).await
                }
            };
            WorkEvent::Repo(RepoEvent::Network {
                job,
                result,
                elapsed: started.elapsed(),
            })
        });
    }

    fn on_network(
        &mut self,
        index: usize,
        job: &NetworkJob,
        result: Result<(), GitError>,
        elapsed: Duration,
    ) {
        let tab = &mut self.tabs[index];
        let label = tab
            .network
            .take()
            .map_or_else(|| job.label(), |op| op.label);
        let (title, body) = match result {
            Ok(()) => {
                let title = job.done();
                self.info_toast(title.clone());
                (title, label)
            }
            Err(GitError::Cancelled) => {
                self.info_toast(format!("{label} cancelled"));
                (String::new(), String::new())
            }
            Err(error) => {
                let detail = describe_error(&error).1;
                self.error_toast(job.failed(), detail.clone());
                (job.failed().to_owned(), detail.unwrap_or_default())
            }
        };
        // Long operations finishing in the background get a Windows notification.
        if !title.is_empty() && elapsed >= NOTIFY_AFTER && !self.window_focused() {
            if let Some(chrome) = self.chrome {
                let body: String = body
                    .lines()
                    .last()
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect();
                if platform::notify(chrome.raw(), &title, &body) {
                    self.notified = true;
                }
            }
        }
        let tab = &mut self.tabs[index];
        if job.changes_worktree() {
            self.op_finished(index);
        } else {
            tab.work.refresh.epoch += 1;
            self.request_status(index, true);
        }
        self.request_refs(index);
    }

    fn window_focused(&self) -> bool {
        self.ctx
            .input(|input| input.viewport().focused.unwrap_or(true))
    }

    /// Push the current branch to its upstream, or publish it to `origin` (or the only
    /// remote) when it has none. With `force`, ask first and overwrite only if the remote
    /// branch is still where Facet last saw it.
    fn push(&mut self, force: bool) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let TabState::Ready { head, .. } = &tab.state else {
            return;
        };
        let Head::Branch { name: branch, .. } = head else {
            self.info_toast("Check out a branch to push");
            return;
        };
        let snapshot = &tab.refs.snapshot;
        let remotes: Vec<String> = snapshot.remotes.iter().map(|r| r.name.clone()).collect();
        let upstream = tab.work.branch.upstream.clone();
        let request = match upstream
            .as_deref()
            .and_then(|upstream| split_upstream(upstream, &remotes))
        {
            Some((remote, remote_branch)) => PushRequest {
                remote: remote.to_owned(),
                branch: branch.clone(),
                remote_branch: remote_branch.to_owned(),
                set_upstream: false,
                force_with_lease: None,
            },
            None => {
                let Some(remote) = remotes
                    .iter()
                    .find(|remote| *remote == "origin")
                    .or_else(|| remotes.first())
                else {
                    self.info_toast("Add a remote to push to");
                    return;
                };
                PushRequest {
                    remote: remote.clone(),
                    branch: branch.clone(),
                    remote_branch: branch.clone(),
                    set_upstream: true,
                    force_with_lease: None,
                }
            }
        };
        if !force {
            self.start_network(NetworkJob::Push(request));
            return;
        }
        let remote_ref = format!("refs/remotes/{}/{}", request.remote, request.remote_branch);
        let Some(seen) = snapshot.find(&remote_ref).map(|r| r.oid.clone()) else {
            self.info_toast("Fetch first: Facet has not seen the remote branch yet");
            return;
        };
        let target = format!("{}/{}", request.remote, request.remote_branch);
        self.confirm = Some(Confirm {
            title: format!("Force push {} to {target}?", request.branch),
            body: format!(
                "Commits on {target} that are not on your branch are removed from the remote. \
                 The push is refused if {target} has moved since you last fetched \
                 (it is at {} here).",
                seen.chars().take(9).collect::<String>()
            ),
            confirm_label: "Force Push".to_owned(),
            action: Action::ConfirmedForcePush(PushRequest {
                force_with_lease: Some(seen),
                ..request
            }),
        });
    }

    // ----- prompts --------------------------------------------------------------------------

    /// Show the oldest credential prompt from git once no other dialog is open.
    pub(crate) fn show_next_askpass(&mut self) {
        if self.prompt.is_some() || self.confirm.is_some() {
            return;
        }
        let Some(request) = self.askpass.front() else {
            return;
        };
        let secret = request.is_secret();
        let mut field = PromptField::new(
            if secret {
                "Password or passphrase"
            } else {
                "Answer"
            },
            "",
            false,
        );
        field.secret = secret;
        let title = request
            .prompt
            .trim()
            .trim_end_matches(':')
            .trim()
            .to_owned();
        self.prompt = Some(
            Prompt::new(PromptKind::Askpass, title, "Continue")
                .body(
                    "Git needs this to continue. Facet passes your answer to git and does not \
                     store it.",
                )
                .field(field),
        );
    }

    fn submit_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let action = match &prompt.kind {
            PromptKind::NewBranch { at } => Action::CreateBranch {
                name: prompt.value(0),
                at: at.clone(),
                switch: prompt.checked(),
            },
            PromptKind::RenameBranch { old } => Action::ConfirmedRenameBranch {
                old: old.clone(),
                new: prompt.value(0),
            },
            PromptKind::NewTag { at } => Action::CreateTag {
                name: prompt.value(0),
                at: at.clone(),
                message: Some(prompt.value(1)).filter(|message| !message.is_empty()),
            },
            PromptKind::AddRemote => Action::CreateRemote {
                name: prompt.value(0),
                url: prompt.value(1),
            },
            PromptKind::Stash { paths, untracked } => Action::CreateStash(StashOptions {
                message: prompt.value(0),
                include_untracked: *untracked || prompt.checked(),
                staged_only: false,
                paths: paths.clone(),
            }),
            PromptKind::AddWorktree => Action::CreateWorktree {
                branch: prompt.value(0),
                path: PathBuf::from(prompt.value(1)),
            },
            PromptKind::CommitAuthor => Action::SetCommitAuthor {
                name: prompt.value(0),
                email: prompt.value(1),
            },
            PromptKind::LfsTrack => Action::CreateLfsTrack {
                pattern: prompt.value(0),
                lockable: prompt.checked(),
            },
            PromptKind::ExternalTool => Action::CreateExternalTool(ExternalTool {
                extensions: ExternalTool::parse_extensions(&prompt.value(0)),
                diff: prompt.value(1),
                merge: prompt.value(2),
            }),
            PromptKind::Askpass => {
                if let Some(request) = self.askpass.pop_front() {
                    request.answer(Some(prompt.value(0)));
                }
                return;
            }
        };
        self.pending.push(action);
    }

    fn cancel_prompt(&mut self) {
        if let Some(prompt) = self.prompt.take() {
            if prompt.kind == PromptKind::Askpass {
                if let Some(request) = self.askpass.pop_front() {
                    request.answer(None);
                }
            }
        }
    }

    // ----- link-outs ----------------------------------------------------------------------

    /// The hosting service of the active repository: the upstream's remote, else `origin`,
    /// else the first remote with a URL Facet recognizes.
    pub(crate) fn forge(&self) -> Option<Forge> {
        let tab = self.active_tab()?;
        let remotes = &tab.refs.snapshot.remotes;
        let names: Vec<String> = remotes.iter().map(|r| r.name.clone()).collect();
        let preferred = tab
            .work
            .branch
            .upstream
            .as_deref()
            .and_then(|upstream| split_upstream(upstream, &names))
            .map(|(remote, _)| remote.to_owned());
        let ordered = preferred
            .iter()
            .map(String::as_str)
            .chain(std::iter::once("origin"))
            .filter_map(|name| remotes.iter().find(|r| r.name == name))
            .chain(remotes.iter());
        ordered
            .filter_map(|remote| remote.url.as_deref())
            .find_map(Forge::from_remote_url)
    }

    fn open_on_forge(&mut self, url: impl FnOnce(&Forge) -> String) {
        let Some(forge) = self.forge() else {
            self.info_toast("This repository's remote is not on GitHub, GitLab or Azure DevOps");
            return;
        };
        if let Err(error) = platform::open_url(&url(&forge)) {
            self.error_toast("Couldn't open the browser", Some(error.to_string()));
        }
    }

    // ----- context for commands -------------------------------------------------------------

    /// What the repository commands depend on (none without a working tree).
    pub(crate) fn repo_context(&self) -> Option<RepoContext> {
        let tab = self.active_tab()?;
        ready_info(tab)?;
        let TabState::Ready { head, .. } = &tab.state else {
            return None;
        };
        let snapshot = &tab.refs.snapshot;
        let work = &tab.work;
        Some(RepoContext {
            view: tab.view,
            bare: false,
            branch: match head {
                Head::Branch { name, .. } => Some(name.clone()),
                Head::Detached { .. } => None,
            },
            has_commits: head.commit().is_some(),
            upstream: work.branch.upstream.clone(),
            local_branches: snapshot
                .local()
                .filter(|r| !r.head)
                .map(|r| r.name.clone())
                .collect(),
            remote_branches: snapshot
                .refs
                .iter()
                .filter(|r| matches!(r.kind, RefKind::Remote { .. }) && r.short_name() != "HEAD")
                .map(|r| r.name.clone())
                .collect(),
            tags: snapshot.tags().map(|r| r.name.clone()).collect(),
            remotes: snapshot.remotes.iter().map(|r| r.name.clone()).collect(),
            stashes: snapshot.stashes.iter().map(|s| s.message.clone()).collect(),
            selected_commit: tab
                .history
                .selected_commit()
                .map(|c| (c.oid.to_string(), c.short_oid().to_string())),
            network_running: tab.network.is_some(),
            pull_mode: self.settings.pull_mode,
            all_branches: self.settings.history_all_branches,
            commit_file_open: tab.history.commit.selected_file.is_some(),
            forge: self.forge().map(|forge| forge.kind),
            dirty: !work.staged.is_empty() || !work.unstaged.is_empty(),
            staged: !work.staged.is_empty(),
            operation: tab.power.operation.as_ref().map(|operation| operation.kind),
            undo: tab.power.undo.last().map(|item| item.title.clone()),
            filter: tab
                .history
                .filter
                .as_ref()
                .map(|filter| filter.kind.title()),
            current_file: self.current_file(),
            conflicted_file: self.conflicted_file(),
            merge_tool: self
                .conflicted_file()
                .is_some_and(|path| self.settings.external_tool(&path, true).is_some()),
            diff_tool: self
                .current_file()
                .is_some_and(|(path, _)| self.settings.external_tool(&path, false).is_some()),
            external_tools: self
                .settings
                .external_tools
                .iter()
                .map(ExternalTool::label)
                .collect(),
            worktrees: snapshot
                .worktrees
                .iter()
                .filter(|w| !w.bare && !same_dir(std::path::Path::new(&w.path), &tab.path))
                .map(|w| (w.name().to_owned(), native_path(&w.path)))
                .collect(),
            prunable_worktrees: snapshot.worktrees.iter().any(|w| w.prunable.is_some()),
            submodules: snapshot
                .submodules
                .iter()
                .map(|s| (s.path.clone(), s.populated))
                .collect(),
            lfs: tab.lfs.usage.lfs,
            lfs_locks: tab.lfs.locks.clone(),
        })
    }

    // ----- events -----------------------------------------------------------------------------

    pub(crate) fn handle_repo_event(&mut self, index: usize, event: RepoEvent) {
        match event {
            RepoEvent::Refs(result) => self.on_refs(index, result),
            RepoEvent::LogPage {
                generation,
                commits,
                rows,
                elapsed_ms,
            } => {
                let history = &mut self.tabs[index].history;
                if generation == history.generation && history.first_page_ms.is_none() {
                    history.first_page_ms = Some(elapsed_ms);
                }
                history.page(generation, commits, rows);
            }
            RepoEvent::LogDone {
                generation,
                result,
                elapsed_ms,
            } => self.on_log_done(index, generation, result, elapsed_ms),
            RepoEvent::Detail { generation, result } => {
                self.on_detail(index, generation, result);
            }
            RepoEvent::StashFiles { generation, result } => {
                self.on_stash_files(index, generation, result);
            }
            RepoEvent::Op { what, result } => self.on_op(index, &what, result),
            RepoEvent::Network {
                job,
                result,
                elapsed,
            } => self.on_network(index, &job, result, elapsed),
            RepoEvent::Power(event) => self.handle_power_event(index, event),
            RepoEvent::Trees(event) => self.handle_tree_event(index, event),
        }
    }

    // ----- actions ----------------------------------------------------------------------------

    /// Handle history, branch, remote and stash actions. Returns the action back if it is
    /// not one of them.
    pub(crate) fn run_repo_action(&mut self, action: Action) -> Option<Action> {
        // Prompts work without a repository (credentials can arrive any time).
        match action {
            Action::SubmitPrompt => {
                self.submit_prompt();
                return None;
            }
            Action::CancelPrompt => {
                self.cancel_prompt();
                return None;
            }
            Action::ToggleSidebarSection(section) => {
                let collapsed = &mut self.settings.collapsed_sections;
                match collapsed.iter().position(|s| *s == section) {
                    Some(position) => {
                        collapsed.remove(position);
                    }
                    None => {
                        collapsed.push(section);
                        collapsed.sort_unstable();
                    }
                }
                return None;
            }
            _ => {}
        }
        let Some(index) = self.active.filter(|_| self.active_work().is_some()) else {
            // Not applicable without a working copy; nothing to do.
            return match action {
                Action::ShowChanges
                | Action::ShowHistory
                | Action::Fetch
                | Action::Pull
                | Action::Push
                | Action::Stash
                | Action::NewBranch { .. } => None,
                other => Some(other).filter(|other| !is_repo_action(other)),
            };
        };
        match action {
            Action::ShowChanges => self.show_view(View::Changes),
            Action::ShowHistory => self.show_view(View::History),
            Action::SelectCommit(row) => self.select_commit(index, row),
            Action::SelectNextCommit | Action::SelectPreviousCommit => {
                let step = if action == Action::SelectNextCommit {
                    1
                } else {
                    -1
                };
                if self.tabs[index].history.move_selection(step) {
                    self.load_detail(index);
                }
            }
            Action::RevealCommit(oid) => {
                self.show_view(View::History);
                let history = &mut self.tabs[index].history;
                if history.select_oid(&oid) {
                    self.load_detail(index);
                } else if history.loading {
                    history.pending_reveal = Some(oid);
                } else if !history.commits.iter().any(|c| c.oid == oid) {
                    self.info_toast("That commit is not in the history");
                }
            }
            Action::SelectCommitFile(file) => self.select_commit_file(index, file),
            Action::CloseCommitFile => self.close_commit_file(index),
            Action::ShowStash(position) => self.show_stash(index, position),
            Action::SelectStashFile(file) => self.select_stash_file(index, file),

            Action::NewBranch { at } => {
                let Some((oid, label)) = self.start_point(at) else {
                    self.info_toast("Make a first commit before creating branches");
                    return None;
                };
                self.prompt = Some(
                    Prompt::new(
                        PromptKind::NewBranch { at: oid },
                        "New Branch",
                        "Create Branch",
                    )
                    .body(format!("Starts at {label}"))
                    .field(PromptField::new("Name", "feature/my-change", true))
                    .check("Switch to the new branch", true),
                );
            }
            Action::CreateBranch { name, at, switch } => {
                let what = format!("create branch {name}");
                self.repo_op(what, move |git, root| async move {
                    facet_git::create_branch(&git, &root, &name, &at, switch).await?;
                    Ok(Outcome::Done(Some(if switch {
                        format!("Switched to new branch {name}")
                    } else {
                        format!("Created branch {name}")
                    })))
                });
            }
            Action::ShowBranches => {
                self.settings_view = None;
                self.open_branches = true;
            }
            Action::SwitchBranch(name) => {
                if !self.refuse_branch_in_other_worktree(&name) {
                    self.switch_to(SwitchTarget::Branch(name));
                }
            }
            Action::CheckoutRemoteBranch(remote_branch) => {
                let snapshot = &self.tabs[index].refs.snapshot;
                let local = snapshot
                    .refs
                    .iter()
                    .find(|r| r.name == remote_branch)
                    .map_or_else(|| remote_branch.clone(), |r| r.short_name().to_owned());
                // An existing local branch of that name is simply switched to.
                let exists = snapshot.local().any(|r| r.name == local);
                self.switch_to(if exists {
                    SwitchTarget::Branch(local)
                } else {
                    SwitchTarget::Track {
                        remote_branch,
                        local,
                    }
                });
            }
            Action::CheckoutCommit(oid) => self.switch_to(SwitchTarget::Detached(oid)),
            Action::StashAndSwitch(target) => {
                let what = "stash and switch".to_owned();
                self.repo_op(what, move |git, root| async move {
                    let options = StashOptions {
                        message: "Facet: changes set aside before switching".to_owned(),
                        ..StashOptions::default()
                    };
                    facet_git::stash_push(&git, &root, &options).await?;
                    facet_git::switch(&git, &root, &target).await?;
                    Ok(Outcome::Done(Some(
                        "Switched. Your changes are in the stash".to_owned(),
                    )))
                });
            }
            Action::RenameBranch(old) => {
                self.prompt = Some(
                    Prompt::new(
                        PromptKind::RenameBranch { old: old.clone() },
                        format!("Rename Branch {old}"),
                        "Rename",
                    )
                    .field(PromptField::new("New name", "", true).with_value(&old)),
                );
            }
            Action::ConfirmedRenameBranch { old, new } => {
                if old != new {
                    let what = format!("rename {old}");
                    self.repo_op(what, move |git, root| async move {
                        facet_git::rename_branch(&git, &root, &old, &new).await?;
                        Ok(Outcome::Undo {
                            title: format!("Renamed {old} to {new}"),
                            action: Action::ConfirmedRenameBranch { old: new, new: old },
                        })
                    });
                }
            }
            Action::DeleteBranch(name) => {
                let tip = self.tabs[index]
                    .refs
                    .snapshot
                    .local()
                    .find(|r| r.name == name)
                    .map(|r| r.oid.chars().take(9).collect::<String>())
                    .unwrap_or_default();
                self.confirm = Some(Confirm {
                    title: format!("Delete branch {name}?"),
                    body: format!(
                        "The branch is removed even if it is not merged. Its commits stay \
                         reachable through Undo (it points at {tip})."
                    ),
                    confirm_label: "Delete".to_owned(),
                    action: Action::ConfirmedDeleteBranch(name),
                });
            }
            Action::ConfirmedDeleteBranch(name) => {
                let upstream = self.tabs[index]
                    .refs
                    .snapshot
                    .local()
                    .find(|r| r.name == name)
                    .and_then(|r| r.upstream.clone());
                let what = format!("delete branch {name}");
                self.repo_op(what, move |git, root| async move {
                    let deleted = facet_git::delete_branch(&git, &root, &name, upstream).await?;
                    Ok(Outcome::Undo {
                        title: format!("Deleted branch {name}"),
                        action: Action::RestoreBranch(deleted),
                    })
                });
            }
            Action::RestoreBranch(deleted) => {
                let what = format!("restore branch {}", deleted.name);
                self.repo_op(what, move |git, root| async move {
                    facet_git::restore_branch(&git, &root, &deleted).await?;
                    Ok(Outcome::Done(Some(format!(
                        "Restored branch {}",
                        deleted.name
                    ))))
                });
            }
            Action::NewTag { at } => {
                let Some((oid, label)) = self.start_point(at) else {
                    self.info_toast("Make a first commit before creating tags");
                    return None;
                };
                self.prompt = Some(
                    Prompt::new(PromptKind::NewTag { at: oid }, "New Tag", "Create Tag")
                        .body(format!("On {label}"))
                        .field(PromptField::new("Name", "v1.0.0", true))
                        .field(PromptField::new(
                            "Message (optional — makes an annotated tag)",
                            "",
                            false,
                        )),
                );
            }
            Action::CreateTag { name, at, message } => {
                let what = format!("create tag {name}");
                self.repo_op(what, move |git, root| async move {
                    facet_git::create_tag(&git, &root, &name, &at, message.as_deref()).await?;
                    Ok(Outcome::Done(Some(format!("Created tag {name}"))))
                });
            }
            Action::DeleteTag(name) => {
                self.confirm = Some(Confirm {
                    title: format!("Delete tag {name}?"),
                    body: "The tag is removed from this repository (not from remotes). You can \
                           undo this."
                        .to_owned(),
                    confirm_label: "Delete".to_owned(),
                    action: Action::ConfirmedDeleteTag(name),
                });
            }
            Action::ConfirmedDeleteTag(name) => {
                let what = format!("delete tag {name}");
                self.repo_op(what, move |git, root| async move {
                    let deleted = facet_git::delete_tag(&git, &root, &name).await?;
                    Ok(Outcome::Undo {
                        title: format!("Deleted tag {name}"),
                        action: Action::RestoreTag(deleted),
                    })
                });
            }
            Action::RestoreTag(deleted) => {
                let what = format!("restore tag {}", deleted.name);
                self.repo_op(what, move |git, root| async move {
                    facet_git::restore_tag(&git, &root, &deleted).await?;
                    Ok(Outcome::Done(Some(format!(
                        "Restored tag {}",
                        deleted.name
                    ))))
                });
            }
            Action::PushTag(tag) => {
                let tab = &self.tabs[index];
                let remotes: Vec<String> = tab
                    .refs
                    .snapshot
                    .remotes
                    .iter()
                    .map(|r| r.name.clone())
                    .collect();
                let remote = tab
                    .work
                    .branch
                    .upstream
                    .as_deref()
                    .and_then(|upstream| split_upstream(upstream, &remotes))
                    .map(|(remote, _)| remote.to_owned())
                    .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
                    .or_else(|| remotes.first().cloned());
                match remote {
                    Some(remote) => self.start_network(NetworkJob::PushTag { remote, tag }),
                    None => self.info_toast("Add a remote to push to"),
                }
            }

            Action::AddRemote => {
                let first = self.tabs[index].refs.snapshot.remotes.is_empty();
                self.prompt = Some(
                    Prompt::new(PromptKind::AddRemote, "Add Remote", "Add Remote")
                        .field(
                            PromptField::new("Name", "origin", true).with_value(if first {
                                "origin"
                            } else {
                                ""
                            }),
                        )
                        .field(PromptField::new(
                            "URL",
                            "https://github.com/owner/repository.git",
                            true,
                        )),
                );
            }
            Action::CreateRemote { name, url } => {
                let what = format!("add remote {name}");
                self.repo_op(what, move |git, root| async move {
                    facet_git::add_remote(&git, &root, &name, &url).await?;
                    Ok(Outcome::Done(Some(format!("Added remote {name}"))))
                });
            }
            Action::RemoveRemote(name) => {
                self.confirm = Some(Confirm {
                    title: format!("Remove remote {name}?"),
                    body: "Its remote-tracking branches are removed too; nothing changes on the \
                           server. You can undo this (the branches return with the next fetch)."
                        .to_owned(),
                    confirm_label: "Remove".to_owned(),
                    action: Action::ConfirmedRemoveRemote(name),
                });
            }
            Action::ConfirmedRemoveRemote(name) => {
                let remote = self.tabs[index].refs.snapshot.remote(&name).cloned()?;
                let what = format!("remove remote {name}");
                self.repo_op(what, move |git, root| async move {
                    facet_git::remove_remote(&git, &root, &name).await?;
                    Ok(Outcome::Undo {
                        title: format!("Removed remote {name}"),
                        action: Action::RestoreRemote(remote),
                    })
                });
            }
            Action::RestoreRemote(remote) => {
                let what = format!("restore remote {}", remote.name);
                self.repo_op(what, move |git, root| async move {
                    facet_git::restore_remote(&git, &root, &remote).await?;
                    Ok(Outcome::Done(Some(format!(
                        "Restored remote {}",
                        remote.name
                    ))))
                });
            }
            Action::Fetch => {
                if !self.tabs[index].refs.snapshot.remotes.is_empty() {
                    self.start_network(NetworkJob::Fetch(None));
                }
            }
            Action::FetchRemote(remote) => self.start_network(NetworkJob::Fetch(Some(remote))),
            Action::Sync => match sync_step(&self.tabs[index]) {
                Some(next) => self.run(next),
                None => self.info_toast("This repository has no remotes to sync with"),
            },
            Action::Pull => {
                if self.tabs[index].work.branch.upstream.is_none() {
                    self.info_toast("This branch has no upstream to pull from");
                } else {
                    self.start_network(NetworkJob::Pull(self.settings.pull_mode.git()));
                }
            }
            Action::Push => self.push(false),
            Action::ForcePush => self.push(true),
            Action::ConfirmedForcePush(request) => {
                self.start_network(NetworkJob::Push(request));
            }
            Action::CancelNetwork => {
                if let Some(network) = &self.tabs[index].network {
                    network.cancel.cancel();
                }
            }

            Action::Stash => {
                let work = &self.tabs[index].work;
                if work.staged.is_empty() && work.unstaged.is_empty() {
                    self.info_toast("There are no changes to stash");
                    return None;
                }
                self.prompt = Some(
                    Prompt::new(
                        PromptKind::Stash {
                            paths: Vec::new(),
                            untracked: false,
                        },
                        "Stash Changes",
                        "Stash",
                    )
                    .body("Set every change aside and return to a clean working tree.")
                    .field(PromptField::new(
                        "Message (optional)",
                        "Work in progress",
                        false,
                    ))
                    .check("Include untracked files", true),
                );
            }
            Action::StashSelected => {
                let work = &self.tabs[index].work;
                let entries = work.selected_entries();
                if entries.is_empty() {
                    self.info_toast("Select the files to stash first");
                    return None;
                }
                let untracked = entries
                    .iter()
                    .any(|entry| entry.kind == EntryKind::Untracked);
                let mut paths: Vec<String> = Vec::new();
                for entry in &entries {
                    paths.push(entry.path.clone());
                    paths.extend(entry.orig_path.clone());
                }
                let count = entries.len();
                self.prompt = Some(
                    Prompt::new(
                        PromptKind::Stash { paths, untracked },
                        format!("Stash {count} {}", plural(count, "File", "Files")),
                        "Stash",
                    )
                    .body("Set the selected files' changes aside; everything else stays.")
                    .field(PromptField::new(
                        "Message (optional)",
                        "Work in progress",
                        false,
                    )),
                );
            }
            Action::StashStaged => {
                self.pending.push(Action::CreateStash(StashOptions {
                    message: String::new(),
                    include_untracked: false,
                    staged_only: true,
                    paths: Vec::new(),
                }));
            }
            Action::CreateStash(options) => {
                self.repo_op("stash the changes", move |git, root| async move {
                    facet_git::stash_push(&git, &root, &options).await?;
                    Ok(Outcome::Done(Some("Changes stashed".to_owned())))
                });
            }
            Action::ApplyStash(position) | Action::PopStash(position) => {
                let pop = matches!(action, Action::PopStash(_));
                let entry = self.tabs[index]
                    .refs
                    .snapshot
                    .stashes
                    .get(position)
                    .cloned()?;
                let what = if pop {
                    "pop the stash"
                } else {
                    "apply the stash"
                };
                self.repo_op(what, move |git, root| async move {
                    facet_git::stash_apply(&git, &root, &entry, pop).await?;
                    Ok(Outcome::Done(Some(
                        if pop {
                            "Stash applied and dropped"
                        } else {
                            "Stash applied"
                        }
                        .to_owned(),
                    )))
                });
            }
            Action::DropStash(position) => {
                let entry = self.tabs[index]
                    .refs
                    .snapshot
                    .stashes
                    .get(position)
                    .cloned()?;
                self.confirm = Some(Confirm {
                    title: "Drop this stash?".to_owned(),
                    body: format!(
                        "“{}” is removed from the stash list. You can undo this.",
                        entry.message
                    ),
                    confirm_label: "Drop".to_owned(),
                    action: Action::ConfirmedDropStash(entry),
                });
            }
            Action::ConfirmedDropStash(entry) => {
                self.repo_op("drop the stash", move |git, root| async move {
                    facet_git::stash_drop(&git, &root, &entry).await?;
                    Ok(Outcome::Undo {
                        title: "Dropped stash".to_owned(),
                        action: Action::RestoreStash(entry),
                    })
                });
            }
            Action::RestoreStash(entry) => {
                self.repo_op("restore the stash", move |git, root| async move {
                    facet_git::restore_stash(&git, &root, &entry).await?;
                    Ok(Outcome::Done(Some("Stash restored".to_owned())))
                });
            }

            Action::OpenCommitOnForge(oid) => self.open_on_forge(|forge| forge.commit_url(&oid)),
            Action::OpenBranchOnForge(branch) => {
                self.open_on_forge(|forge| forge.branch_url(&branch));
            }
            Action::CreatePullRequest(branch) => {
                self.open_on_forge(|forge| forge.new_pull_request_url(&branch));
            }
            other => return Some(other),
        }
        None
    }
}

/// Actions that belong to this file (ignored, not reported, when there is no working copy).
fn is_repo_action(action: &Action) -> bool {
    !matches!(
        action,
        Action::ShowPalette
            | Action::OpenRepository
            | Action::OpenPath(_)
            | Action::Quit
            // External tools are settings: they can be managed without a repository.
            | Action::AddExternalTool
            | Action::CreateExternalTool(_)
            | Action::RemoveExternalTool(_)
    )
}

/// What the Sync button does for this tab: bring in the upstream's commits first, then send
/// ours, else just look for new ones. `None` when there is no remote at all.
pub(crate) fn sync_step(tab: &RepoTab) -> Option<Action> {
    if tab.refs.snapshot.remotes.is_empty() {
        return None;
    }
    let branch = &tab.work.branch;
    let on_branch = matches!(
        &tab.state,
        TabState::Ready {
            head: Head::Branch { .. },
            ..
        }
    );
    Some(
        if branch.upstream.is_some() && branch.behind.is_some_and(|n| n > 0) {
            Action::Pull
        } else if on_branch && (branch.upstream.is_none() || branch.ahead.is_some_and(|n| n > 0)) {
            Action::Push
        } else {
            Action::Fetch
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstreams_split_at_the_longest_matching_remote() {
        let remotes = vec!["origin".to_owned(), "team".to_owned(), "team/eu".to_owned()];
        assert_eq!(
            split_upstream("origin/feature/x", &remotes),
            Some(("origin", "feature/x"))
        );
        assert_eq!(
            split_upstream("team/eu/main", &remotes),
            Some(("team/eu", "main"))
        );
        assert_eq!(
            split_upstream("team/main", &remotes),
            Some(("team", "main"))
        );
        assert_eq!(split_upstream("originx/main", &remotes), None);
        assert_eq!(split_upstream("origin", &remotes), None);
    }

    #[test]
    fn switch_refusals_are_recognised() {
        let refused = GitError::Failed {
            command: "git switch topic".into(),
            code: 1,
            stderr: "error: Your local changes to the following files would be overwritten by \
                     checkout:\n\ta.txt\nPlease commit your changes or stash them before you \
                     switch branches."
                .into(),
        };
        assert!(overwrite_refusal(&refused));
        let other = GitError::Failed {
            command: "git switch nope".into(),
            code: 128,
            stderr: "fatal: invalid reference: nope".into(),
        };
        assert!(!overwrite_refusal(&other));
    }

    #[test]
    fn network_jobs_describe_themselves() {
        let push = PushRequest {
            remote: "origin".into(),
            branch: "main".into(),
            remote_branch: "main".into(),
            set_upstream: false,
            force_with_lease: None,
        };
        assert_eq!(
            NetworkJob::Push(push.clone()).label(),
            "Pushing main to origin"
        );
        assert_eq!(
            NetworkJob::Push(push.clone()).done(),
            "Pushed main to origin/main"
        );
        let forced = PushRequest {
            force_with_lease: Some("abc".into()),
            ..push
        };
        assert_eq!(
            NetworkJob::Push(forced).done(),
            "Force-pushed main to origin/main"
        );
        assert_eq!(NetworkJob::Fetch(None).failed(), "Couldn't fetch");
    }
}
