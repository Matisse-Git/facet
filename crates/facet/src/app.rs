//! Application state and behavior. Views live in `crate::ui` as further `impl FacetApp` blocks.
//!
//! Threading model: this struct lives on the UI thread and never blocks. Anything slow (git,
//! file dialogs, disk writes) runs elsewhere and reports back as an [`Event`] over a channel;
//! the sender wakes the UI with `request_repaint`.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use facet_git::askpass::{self, AskpassRequest};
use facet_git::parse::GitVersion;
use facet_git::{CancellationToken, CommandLog, CommandRecord, Git, GitError, Head, RepoInfo};

use crate::actions::{shortcut, Action, Command, CommandContext, WorkContext, GLOBAL_SHORTCUTS};
use crate::app_repo::NetworkOp;
use crate::app_settings::{SettingsEvent, SettingsView};
use crate::app_trees::LfsState;
use crate::app_work::WorkEvent;
use crate::history::{History, RefsState, View};
use crate::integrations::Installed;
use crate::platform::{self, Chrome};
use crate::power::PowerState;
use crate::settings::{same_path, Settings};
use crate::theme::{self, Palette};
use crate::ui::confirm::Confirm;
use crate::ui::palette::PaletteState;
use crate::ui::prompt::Prompt;
use crate::ui::toast::{Toast, ToastKind};
use crate::watcher::{IndexStamp, RepoWatcher};
use crate::workspace::{Area, DiffBody, Workspace};

/// How long settings changes settle before being written to disk.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(800);
const COMMAND_LOG_CAPACITY: usize = 2000;

/// Results coming back from background work.
pub enum Event {
    GitVersion(Result<GitVersion, GitError>),
    FolderPicked(Result<Option<PathBuf>, String>),
    RepoLoaded {
        tab: u64,
        result: Result<(RepoInfo, Head), GitError>,
    },
    SettingsSaved(Result<(), String>),
    /// Git asks for a username, password or passphrase (G7).
    Askpass(AskpassRequest),
    /// Settings, integrations and second-launch hand-overs (see `app_settings.rs`).
    Settings(SettingsEvent),
    /// The Add Repository dialog: folder picks, finished clones and inits (`app_add.rs`).
    AddRepo(crate::app_add::AddRepoEvent),
    /// Working copy results for one tab (see `app_work.rs`). `stamp` is the state of
    /// `.git/index` right after work that may have written it.
    Work {
        tab: u64,
        stamp: Option<IndexStamp>,
        event: WorkEvent,
    },
}

pub enum TabState {
    /// First load in flight; the view shows skeletons.
    Loading,
    Ready {
        info: RepoInfo,
        head: Head,
    },
}

pub struct RepoTab {
    pub id: u64,
    /// The path as opened; replaced by the repository root once known.
    pub path: PathBuf,
    pub state: TabState,
    /// Cancels the load/refresh currently in flight for this tab, if any.
    load: Option<CancellationToken>,
    /// Reopened from the previous session rather than by the user; does not count as "recent".
    restored: bool,
    /// Status, selection, diff, commit draft and undo stack (unused for bare repositories).
    pub work: Workspace,
    /// Keeps the file system watch alive while the tab is open.
    pub(crate) watcher: Option<RepoWatcher>,
    /// Serializes this repository's index-changing git operations.
    pub(crate) ops: Arc<tokio::sync::Mutex<()>>,
    /// `.git/index` as Facet last left it, to tell its own writes from outside changes.
    pub(crate) index_stamp: IndexStamp,
    /// Changes or History.
    pub view: View,
    /// Branches, tags, remotes, stashes.
    pub refs: RefsState,
    pub history: History,
    /// The fetch / pull / push in flight (one per repository).
    pub network: Option<NetworkOp>,
    /// The operation in progress, undo points, search, blame, rebase and conflict editors.
    pub power: PowerState,
    /// Git LFS: usage, attributes of the listed files, locks.
    pub lfs: LfsState,
}

impl RepoTab {
    pub fn name(&self) -> String {
        match &self.state {
            TabState::Ready { info, .. } => info.name(),
            TabState::Loading => self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.path.display().to_string()),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state, TabState::Ready { .. })
    }
}

/// Git availability, shown in the sidebar footer.
pub enum GitStatus {
    Checking,
    Found(GitVersion),
    Missing(String),
}

pub struct FacetApp {
    pub settings: Settings,
    saved_settings: Settings,
    settings_dirty_since: Option<Instant>,
    save_in_flight: bool,

    pub(crate) runtime: tokio::runtime::Runtime,
    pub(crate) git: Git,
    pub command_log: Arc<CommandLog>,
    /// Snapshot of the command log for the panel, rebuilt only when the log's revision changes.
    pub log_view: (u64, Vec<CommandRecord>),
    pub(crate) events_tx: Sender<Event>,
    events_rx: Receiver<Event>,
    pub(crate) ctx: egui::Context,

    pub tabs: Vec<RepoTab>,
    pub active: Option<usize>,
    next_tab_id: u64,

    pub git_status: GitStatus,
    pub palette: PaletteState,
    pub toasts: Vec<Toast>,
    next_toast_id: u64,
    /// The question currently asked before a destructive action, if any.
    pub confirm: Option<Confirm>,
    /// The text dialog currently open (new branch, stash message, credentials…).
    pub prompt: Option<Prompt>,
    /// The settings view, while open.
    pub settings_view: Option<SettingsView>,
    /// The Add Repository dialog, while open.
    pub add_view: Option<crate::app_add::AddRepoView>,
    /// The dialog's tab when it was last closed; it opens there again.
    pub(crate) last_add_tab: crate::app_add::AddTab,
    /// Programs found on this machine (terminal, editors, diff / merge tools).
    pub(crate) installed: Installed,
    /// Credential prompts from git, oldest first; the first is shown once no other dialog is.
    pub(crate) askpass: VecDeque<AskpassRequest>,
    /// A Windows notification is showing (cleared when the window is focused again).
    pub(crate) notified: bool,
    /// Actions queued by views during a frame; run after the frame's UI is built.
    pub pending: Vec<Action>,

    pub chrome: Option<Chrome>,
    applied_dark: Option<bool>,
    picking_folder: bool,
    was_focused: bool,

    startup: Instant,
    /// Which GPU adapter is rendering, for the startup trace.
    gpu: String,
    frames: u64,
    /// `FACET_BENCH_SCROLL`: the history scroll measurement in progress.
    bench: Option<crate::bench::ScrollBench>,
    /// `FACET_BENCH_SEARCH`: the search to run once the history has loaded.
    bench_search: Option<(facet_git::SearchKind, String)>,
}

impl FacetApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        runtime: tokio::runtime::Runtime,
        startup: Instant,
        open_on_launch: Option<PathBuf>,
        instance: Option<crate::instance::Server>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx, settings.theme, settings.code_font);
        ctx.set_zoom_factor(settings.ui_scale);

        let dark = ctx.theme() == egui::Theme::Dark;
        let chrome = Chrome::install(cc, dark);
        if let (Some(chrome), Some(placement)) = (chrome, settings.window) {
            chrome.restore_placement(placement);
        }

        let command_log = Arc::new(CommandLog::new(COMMAND_LOG_CAPACITY));
        {
            let ctx = ctx.clone();
            command_log.set_listener(move || ctx.request_repaint());
        }
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        let git = credential_git(
            Git::new(
                settings
                    .git_path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("git")),
                command_log.clone(),
            ),
            &runtime,
            chrome,
            &events_tx,
            &ctx,
        );

        let mut app = Self {
            saved_settings: settings.clone(),
            settings,
            settings_dirty_since: None,
            save_in_flight: false,
            runtime,
            git,
            command_log,
            log_view: (u64::MAX, Vec::new()),
            events_tx,
            events_rx,
            ctx,
            tabs: Vec::new(),
            active: None,
            next_tab_id: 1,
            git_status: GitStatus::Checking,
            palette: PaletteState::default(),
            toasts: Vec::new(),
            next_toast_id: 1,
            confirm: None,
            prompt: None,
            settings_view: None,
            add_view: None,
            last_add_tab: crate::app_add::AddTab::default(),
            installed: Installed::default(),
            askpass: VecDeque::new(),
            notified: false,
            pending: Vec::new(),
            chrome,
            applied_dark: Some(dark),
            gpu: cc
                .wgpu_render_state
                .as_ref()
                .map(|state| {
                    let info = state.adapter.get_info();
                    format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)
                })
                .unwrap_or_else(|| "unknown adapter".to_owned()),
            picking_folder: false,
            was_focused: true,
            startup,
            frames: 0,
            bench: crate::bench::ScrollBench::from_env(),
            bench_search: crate::bench::search_from_env(),
        };

        app.check_git();
        app.detect_integrations();
        if let Some(server) = instance {
            let (tx, ctx) = (app.events_tx.clone(), app.ctx.clone());
            let _entered = app.runtime.enter();
            crate::instance::serve(server, move |path| {
                if tx
                    .send(Event::Settings(SettingsEvent::Forwarded(path)))
                    .is_ok()
                {
                    ctx.request_repaint();
                }
            });
        }
        let restore: Vec<PathBuf> = app.settings.open_repos.clone();
        let active = app.settings.active_repo;
        for path in restore {
            app.open_path(path, false);
        }
        if let Some(index) = active.filter(|i| *i < app.tabs.len()) {
            app.active = Some(index);
        }
        if let Some(path) = open_on_launch {
            app.open_path(path, true);
        }
        app
    }

    // ----- background plumbing -------------------------------------------------------------

    /// Run `future` on the tokio runtime and deliver its event to the UI thread.
    pub(crate) fn spawn(&self, future: impl std::future::Future<Output = Event> + Send + 'static) {
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn(async move {
            let event = future.await;
            if tx.send(event).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn check_git(&mut self) {
        let git = self.git.clone();
        self.spawn(async move { Event::GitVersion(facet_git::version(&git).await) });
    }

    /// Drain finished background work. Called once per frame (and while hidden).
    pub fn drain_events(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::GitVersion(Ok(version)) => self.git_status = GitStatus::Found(version),
            Event::GitVersion(Err(error)) => {
                let detail = error.to_string();
                self.error_toast(
                    "Git was not found",
                    Some("Install Git for Windows and make sure it is on your PATH.".to_owned()),
                );
                self.git_status = GitStatus::Missing(detail);
            }
            Event::FolderPicked(result) => {
                self.picking_folder = false;
                match result {
                    Ok(Some(path)) => self.open_path(path, true),
                    Ok(None) => {}
                    Err(detail) => {
                        self.error_toast("Couldn't show the folder picker", Some(detail))
                    }
                }
            }
            Event::RepoLoaded { tab, result } => self.repo_loaded(tab, result),
            Event::SettingsSaved(result) => {
                self.save_in_flight = false;
                if let Err(detail) = result {
                    self.error_toast("Couldn't save settings", Some(detail));
                }
            }
            Event::Askpass(request) => self.askpass.push_back(request),
            Event::Settings(event) => self.handle_settings_event(event),
            Event::AddRepo(event) => self.handle_add_event(event),
            Event::Work { tab, stamp, event } => self.handle_work_event(tab, stamp, event),
        }
    }

    // ----- repositories and tabs -----------------------------------------------------------

    /// Open `path` in a tab (or focus the tab that already has it).
    pub fn open_path(&mut self, path: PathBuf, activate: bool) {
        if let Some(existing) = self.tabs.iter().position(|t| same_path(&t.path, &path)) {
            if activate {
                self.active = Some(existing);
            }
            return;
        }

        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(RepoTab {
            id,
            path,
            state: TabState::Loading,
            load: None,
            restored: !activate,
            work: Workspace::default(),
            watcher: None,
            ops: Arc::default(),
            index_stamp: None,
            view: View::default(),
            refs: RefsState::default(),
            history: History::default(),
            network: None,
            power: PowerState::default(),
            lfs: LfsState::default(),
        });
        let index = self.tabs.len() - 1;
        if activate || self.active.is_none() {
            self.active = Some(index);
        }
        self.load_tab(index);
    }

    /// (Re)load repository identity and HEAD for a tab, cancelling any load still in flight.
    fn load_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if let Some(previous) = tab.load.take() {
            previous.cancel();
        }
        let cancel = CancellationToken::new();
        tab.load = Some(cancel.clone());

        let git = self.git.clone();
        let id = tab.id;
        let path = tab.path.clone();
        // A refresh already knows where the repository is; only HEAD can have changed.
        // Every git process costs tens of milliseconds on Windows, so don't re-discover.
        let known = match &tab.state {
            TabState::Ready { info, .. } => Some(info.clone()),
            TabState::Loading => None,
        };
        self.spawn(async move {
            let work = async {
                let info = match known {
                    Some(info) => {
                        let exists = tokio::fs::metadata(info.root())
                            .await
                            .is_ok_and(|meta| meta.is_dir());
                        if !exists {
                            return Err(GitError::PathNotFound {
                                path: info.root().to_owned(),
                            });
                        }
                        info
                    }
                    None => {
                        // A dropped file means "the repository containing this file". The
                        // check touches the disk, so it happens here, not on the UI thread.
                        let path = match tokio::fs::metadata(&path).await {
                            Ok(meta) if meta.is_file() => {
                                path.parent().map(Path::to_owned).unwrap_or(path)
                            }
                            _ => path,
                        };
                        facet_git::discover(&git, &path).await?
                    }
                };
                let head = facet_git::head(&git, info.root()).await?;
                Ok((info, head))
            };
            let result = tokio::select! {
                () = cancel.cancelled() => Err(GitError::Cancelled),
                result = work => result,
            };
            Event::RepoLoaded { tab: id, result }
        });
    }

    fn repo_loaded(&mut self, id: u64, result: Result<(RepoInfo, Head), GitError>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return; // tab was closed while loading
        };
        match result {
            Ok((info, head)) => {
                let root = info.root().to_owned();
                // The same repository may have been opened through a different path
                // (a subfolder, different casing). Keep one tab per repository.
                if let Some(duplicate) = self
                    .tabs
                    .iter()
                    .position(|t| t.id != id && same_path(&t.path, &root))
                {
                    let was_active = self.active == Some(index);
                    self.remove_tab(index);
                    if was_active {
                        let duplicate = if duplicate > index {
                            duplicate - 1
                        } else {
                            duplicate
                        };
                        self.active = Some(duplicate);
                    }
                    return;
                }
                let first_load = !self.tabs[index].is_ready();
                if first_load && !self.tabs[index].restored {
                    self.settings.touch_recent(&root);
                }
                let tab = &mut self.tabs[index];
                tab.path = root;
                tab.state = TabState::Ready { info, head };
                tab.load = None;
                if first_load {
                    // Status, the file watcher and everything else about the working copy.
                    self.start_workspace(index);
                }
            }
            Err(GitError::Cancelled) => {}
            Err(error) => {
                let (title, detail) = describe_error(&error);
                let tab = &mut self.tabs[index];
                tab.load = None;
                if tab.is_ready() {
                    // A refresh failed: keep showing the last known state.
                    self.error_toast(title, detail);
                } else {
                    self.remove_tab(index);
                    self.error_toast(title, detail);
                }
            }
        }
    }

    pub(crate) fn remove_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        cancel_tab_work(&tab);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            other => other,
        };
    }

    pub fn active_tab(&self) -> Option<&RepoTab> {
        self.active.and_then(|index| self.tabs.get(index))
    }

    fn active_repo_root(&self) -> Option<PathBuf> {
        match &self.active_tab()?.state {
            TabState::Ready { info, .. } => Some(info.root().to_owned()),
            TabState::Loading => None,
        }
    }

    fn cycle_tab(&mut self, step: isize) {
        let count = self.tabs.len() as isize;
        if count < 2 {
            return;
        }
        let current = self.active.unwrap_or(0) as isize;
        self.active = Some((current + step).rem_euclid(count) as usize);
    }

    // ----- actions --------------------------------------------------------------------------

    /// The commands available right now (for the palette).
    pub fn commands(&self) -> Vec<Command> {
        let names: Vec<String> = self.tabs.iter().map(RepoTab::name).collect();
        let work = self.work_context();
        let repo = self.repo_context();
        let open_in = self.installed.open_targets();
        crate::actions::commands(&CommandContext {
            tabs: &names,
            active_tab: self.active,
            repo_ready: self.active_tab().is_some_and(RepoTab::is_ready),
            work: work.as_ref(),
            repo: repo.as_ref(),
            recent: &self.settings.recent_repos,
            theme: self.settings.theme,
            sidebar_visible: self.settings.sidebar_visible,
            command_log_visible: self.settings.command_log_visible,
            command_log_empty: self.command_log.is_empty(),
            ui_scale: self.settings.ui_scale,
            code_font: self.settings.code_font,
            custom_git: self.settings.git_path.is_some(),
            open_in: &open_in,
        })
    }

    /// What the working copy commands depend on (none without a working tree).
    fn work_context(&self) -> Option<WorkContext> {
        let work = self.active_work()?;
        let text = work.diff.text();
        Some(WorkContext {
            staged: work.staged.len(),
            unstaged: work.unstaged.len(),
            selection: Some((work.selection.area, work.selection.items.len())),
            has_current: work.current().is_some(),
            diff_area: work.diff.key.as_ref().map(|key| key.area),
            hunks: text
                .filter(|text| text.partial)
                .map_or(0, |text| text.diff.hunks.len()),
            selected_lines: text.map_or(0, |text| text.selected.len()),
            can_commit: self.can_commit(),
            can_amend: work.branch.oid.is_some() && !work.commit.running,
            amend: work.commit.amend,
            undo: work
                .last_undo()
                .map(|entry| (entry.label.clone(), entry.redo)),
            perf: work.perf.config,
            image_mode: matches!(work.diff.body, DiffBody::Image(_))
                .then_some(self.settings.image_mode),
        })
    }

    /// Perform one action. Actions that do not apply in the current state are ignored.
    pub fn run(&mut self, action: Action) {
        match action {
            Action::ShowPalette => self.palette.toggle(),
            Action::OpenRepository => self.pick_repository(),
            Action::OpenPath(path) => self.open_path(path, true),
            Action::ForgetRecent(path) => self.settings.forget_recent(&path),
            Action::CloseTab(index) => self.remove_tab(index),
            Action::CloseActiveTab => {
                if let Some(index) = self.active {
                    self.remove_tab(index);
                }
            }
            Action::NextTab => self.cycle_tab(1),
            Action::PreviousTab => self.cycle_tab(-1),
            Action::SwitchTab(index) => {
                if index < self.tabs.len() {
                    self.active = Some(index);
                }
            }
            Action::Refresh => {
                if let Some(index) = self.active {
                    if self.active_work().is_some() {
                        // An explicit refresh may take git's optional locks.
                        self.request_status(index, true);
                    } else {
                        self.load_tab(index);
                    }
                }
            }
            Action::RevealInExplorer => {
                if let Some(root) = self.active_repo_root() {
                    if let Err(error) = platform::reveal_in_explorer(&root) {
                        self.error_toast("Couldn't open File Explorer", Some(error.to_string()));
                    }
                }
            }
            Action::CopyRepositoryPath => {
                if let Some(root) = self.active_repo_root() {
                    self.ctx.copy_text(root.display().to_string());
                    self.info_toast("Repository path copied");
                }
            }
            Action::ToggleSidebar => {
                self.settings.sidebar_visible = !self.settings.sidebar_visible;
            }
            Action::ToggleCommandLog => {
                self.settings.command_log_visible = !self.settings.command_log_visible;
            }
            Action::ClearCommandLog => self.command_log.clear(),
            Action::CopyText { text, what } => {
                self.ctx.copy_text(text);
                self.info_toast(format!("{what} copied"));
            }
            Action::SetTheme(mode) => {
                self.settings.theme = mode;
                theme::set_mode(&self.ctx, mode);
            }
            Action::Quit => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            // Everything else belongs to the working copy (`app_work.rs`) or to history,
            // branches, remotes and stashes (`app_repo.rs`).
            work => {
                let unhandled = self
                    .run_settings_action(work)
                    .and_then(|other| self.run_add_action(other))
                    .and_then(|other| self.run_work_action(other))
                    .and_then(|other| self.run_repo_action(other))
                    .and_then(|other| self.run_power_action(other))
                    .and_then(|other| self.run_tree_action(other));
                debug_assert!(unhandled.is_none(), "unhandled action {unhandled:?}");
            }
        }
    }

    /// Show the native folder picker on its own thread (it blocks until dismissed).
    fn pick_repository(&mut self) {
        if self.picking_folder {
            return;
        }
        self.picking_folder = true;
        let owner = self.chrome.map(Chrome::raw);
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("facet-folder-picker".into())
            .spawn(move || {
                let result = platform::pick_folder(owner, "Open Repository");
                if tx.send(Event::FolderPicked(result)).is_ok() {
                    ctx.request_repaint();
                }
            });
        if let Err(error) = spawned {
            self.picking_folder = false;
            self.error_toast("Couldn't show the folder picker", Some(error.to_string()));
        }
    }

    // ----- toasts ---------------------------------------------------------------------------

    pub fn info_toast(&mut self, title: impl Into<String>) {
        self.push_toast(ToastKind::Info, title.into(), None);
    }

    pub fn error_toast(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push_toast(ToastKind::Error, title.into(), detail);
    }

    /// A toast with a button (used for "Undo" after a discard).
    pub fn action_toast(&mut self, title: impl Into<String>, button: &str, action: Action) {
        // Only the newest offer stays valid: an older "Undo" would act on the wrong entry.
        for toast in &mut self.toasts {
            if toast.action.is_some() {
                toast.dismiss();
            }
        }
        self.push_toast(ToastKind::Info, title.into(), None);
        if let Some(toast) = self.toasts.last_mut() {
            toast.action = Some((button.to_owned(), action));
        }
    }

    fn push_toast(&mut self, kind: ToastKind, title: String, detail: Option<String>) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        // Repeating the same message replaces the old toast instead of stacking.
        self.toasts
            .retain(|t| t.title != title || t.detail != detail);
        self.toasts.push(Toast::new(id, kind, title, detail));
        self.ctx.request_repaint();
    }

    // ----- per-frame housekeeping -----------------------------------------------------------

    /// Keep the DWM frame (caption buttons, Mica tint) in step with the effective appearance.
    fn sync_appearance(&mut self, ctx: &egui::Context) {
        let dark = Palette::of(ctx).dark;
        if self.applied_dark != Some(dark) {
            self.applied_dark = Some(dark);
            if let Some(chrome) = self.chrome {
                chrome.set_dark(dark);
            }
            // Syntax colors are made for one appearance.
            self.redecorate_all();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // A dialog owns the keyboard entirely (it handles Enter and Escape).
        if self.confirm.is_some()
            || self.prompt.is_some()
            || self.add_view.is_some()
            || self.power_modal_open()
        {
            return;
        }
        for (shortcut, action) in GLOBAL_SHORTCUTS {
            if ctx.input_mut(|input| input.consume_shortcut(shortcut)) {
                // While the palette is open it owns the keyboard, except its own toggle.
                if !self.palette.open || *action == Action::ShowPalette {
                    self.pending.push(action.clone());
                }
            }
        }
        self.handle_work_keys(ctx);
    }

    /// Keys of the file list and the diff. They apply while no widget has keyboard focus, so
    /// typing a commit message or tabbing through buttons never stages or discards anything.
    fn handle_work_keys(&mut self, ctx: &egui::Context) {
        // The settings page covers the file list and the diff: their keys must not act on
        // a selection nobody can see.
        if self.palette.open
            || self.settings_view.is_some()
            || ctx.memory(|memory| memory.focused().is_some())
        {
            return;
        }
        // Escape leaves the blame view, then a filtered commit list.
        let (blame, filtered) = self.active_tab().map_or((false, false), |tab| {
            (
                tab.power.blame.is_some(),
                tab.view == View::History
                    && (tab.history.filter.is_some() || tab.power.search.open),
            )
        });
        if (blame || filtered)
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.pending.push(if blame {
                Action::CloseBlame
            } else {
                Action::ClearFilter
            });
            return;
        }
        if self
            .active_tab()
            .is_some_and(|tab| tab.view == View::History)
        {
            let mut actions = Vec::new();
            ctx.input_mut(|input| {
                if input.consume_shortcut(&shortcut::NEXT_FILE) {
                    actions.push(Action::SelectNextCommit);
                }
                if input.consume_shortcut(&shortcut::PREVIOUS_FILE) {
                    actions.push(Action::SelectPreviousCommit);
                }
            });
            self.pending.extend(actions);
            return;
        }
        let Some(work) = self.active_work() else {
            return;
        };
        let lines_selected = work
            .diff
            .text()
            .is_some_and(|text| !text.selected.is_empty());
        let diff_area = work.diff.key.as_ref().map(|key| key.area);

        let mut actions = Vec::new();
        ctx.input_mut(|input| {
            // Most specific first: shortcuts match logically (Shift+Down also matches Down).
            if input.consume_shortcut(&shortcut::UNDO) {
                actions.push(Action::UndoDiscard);
            }
            if input.consume_shortcut(&shortcut::SELECT_ALL) {
                actions.push(Action::SelectAllFiles);
            }
            if input.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) {
                actions.push(Action::ExtendFileSelection(1));
            }
            if input.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp) {
                actions.push(Action::ExtendFileSelection(-1));
            }
            if input.consume_shortcut(&shortcut::NEXT_FILE) {
                actions.push(Action::SelectNextFile);
            }
            if input.consume_shortcut(&shortcut::PREVIOUS_FILE) {
                actions.push(Action::SelectPreviousFile);
            }
            // With lines selected in the diff, Enter / Delete / Escape act on those lines;
            // otherwise on the selected files.
            let toggle = input.consume_shortcut(&shortcut::TOGGLE_STAGE)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Space);
            if toggle {
                actions.push(match (lines_selected, diff_area) {
                    (true, Some(Area::Unstaged)) => Action::StageLines,
                    (true, Some(Area::Staged)) => Action::UnstageLines,
                    _ => Action::ToggleStageSelected,
                });
            }
            if input.consume_shortcut(&shortcut::DISCARD) {
                actions.push(if lines_selected {
                    Action::DiscardLines
                } else {
                    Action::DiscardSelected
                });
            }
            if lines_selected {
                if input.consume_shortcut(&shortcut::CLEAR_LINES) {
                    actions.push(Action::ClearLineSelection);
                }
                // The OS delivers Ctrl+C as a copy event, not as a key press.
                if input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy))
                {
                    actions.push(Action::CopySelectedLines);
                }
            }
        });
        self.pending.extend(actions);
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        });
        for path in dropped {
            self.pending.push(Action::OpenPath(path));
        }
    }

    /// Returning to the window refreshes the active repository: a safety net under the file
    /// watcher (which can miss events on network drives or when the OS buffer overflows).
    fn refresh_on_focus(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        if focused && self.notified {
            self.notified = false;
            if let Some(chrome) = self.chrome {
                platform::clear_notification(chrome.raw());
            }
        }
        if focused && !self.was_focused {
            if let Some(index) = self.active.filter(|i| self.tabs[*i].is_ready()) {
                if self.active_work().is_some() {
                    self.request_status(index, false);
                    self.request_refs(index);
                } else {
                    self.load_tab(index);
                }
            }
        }
        self.was_focused = focused;
    }

    /// Mirror tab state into settings and write settings to disk once they settle.
    fn persist_settings(&mut self) {
        self.settings.open_repos = self
            .tabs
            .iter()
            .filter(|tab| tab.is_ready())
            .map(|tab| tab.path.clone())
            .collect();
        self.settings.active_repo =
            self.active_tab()
                .filter(|tab| tab.is_ready())
                .and_then(|active| {
                    self.settings
                        .open_repos
                        .iter()
                        .position(|path| same_path(path, &active.path))
                });

        if self.settings == self.saved_settings {
            self.settings_dirty_since = None;
            return;
        }
        let since = *self.settings_dirty_since.get_or_insert_with(Instant::now);
        let waited = since.elapsed();
        if waited < SAVE_DEBOUNCE {
            self.ctx.request_repaint_after(SAVE_DEBOUNCE - waited);
            return;
        }
        if self.save_in_flight {
            return;
        }
        self.capture_window_placement();
        self.saved_settings = self.settings.clone();
        self.settings_dirty_since = None;
        self.save_in_flight = true;
        let snapshot = self.settings.clone();
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn_blocking(move || {
            let result = snapshot.save().map_err(|e| e.to_string());
            if tx.send(Event::SettingsSaved(result)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn capture_window_placement(&mut self) {
        if let Some(placement) = self.chrome.and_then(Chrome::placement) {
            self.settings.window = Some(placement);
        }
    }

    /// `FACET_BENCH_SCROLL`: show the history, then scroll it every frame and log frame times.
    fn step_scroll_bench(&mut self, previous_cpu_secs: Option<f32>) {
        if self.bench.is_none() && self.bench_search.is_none() {
            return;
        }
        let Some(index) = self.active.filter(|index| {
            self.tabs
                .get(*index)
                .is_some_and(|tab| crate::app_work::ready_info(tab).is_some())
        }) else {
            return;
        };
        if let Some(crate::bench::Target::Diff(path)) =
            self.bench.as_ref().map(|bench| bench.target.clone())
        {
            self.step_diff_bench(index, &path, previous_cpu_secs);
            return;
        }
        let tab = &mut self.tabs[index];
        if tab.view != View::History {
            self.pending.push(Action::ShowHistory);
            return;
        }
        if !tab.history.loaded {
            return;
        }
        // `FACET_BENCH_SEARCH`: search first, then scroll the results.
        if let Some((kind, text)) = self.bench_search.take() {
            tab.power.search.kind = kind;
            tab.power.search.text = text;
            tab.power.search.open = true;
            self.pending.push(Action::RunSearch);
            return;
        }
        let Some(bench) = &mut self.bench else {
            return;
        };
        let history = &mut tab.history;
        let height = history.commits.len() as f32 * crate::ui::COMMIT_ROW;
        match bench.step(previous_cpu_secs, height) {
            Some(offset) => {
                history.scroll_to = Some(offset);
                self.ctx.request_repaint();
            }
            None => {
                let report = bench.report(&format!(
                    "{} commits, {} lanes",
                    history.commits.len(),
                    history.max_lanes
                ));
                crate::app_work::perf_log(&report);
                self.bench = None;
                self.info_toast("Scroll benchmark finished (see FACET_PERF_LOG)");
            }
        }
    }

    /// `FACET_BENCH_SCROLL=diff:<path>`: open the working-copy diff of `path`, then scroll it
    /// every frame and log frame times.
    fn step_diff_bench(&mut self, index: usize, path: &str, previous_cpu_secs: Option<f32>) {
        use crate::workspace::{Area, DiffBody};
        let tab = &mut self.tabs[index];
        if tab.view != View::Changes {
            self.pending.push(Action::ShowChanges);
            return;
        }
        let Some(bench) = &mut self.bench else {
            return;
        };
        if !tab.work.loaded {
            return;
        }
        let work = &mut tab.work;
        if !bench.opened {
            let found = [Area::Unstaged, Area::Staged].into_iter().find_map(|area| {
                (0..work.list(area).len())
                    .find(|&row| work.entry(area, row).is_some_and(|e| e.path == path))
                    .map(|row| (area, row))
            });
            match found {
                Some((area, row)) => {
                    self.pending.push(Action::SelectFile {
                        area,
                        index: row,
                        toggle: false,
                        extend: false,
                    });
                    bench.opened = true;
                }
                None => {
                    crate::app_work::perf_log(&format!("scroll bench: {path} has no changes\n"));
                    self.bench = None;
                }
            }
            return;
        }
        let DiffBody::Text(text) = &work.diff.body else {
            return; // still loading
        };
        let rows = text.row_count();
        match bench.step(previous_cpu_secs, rows as f32 * crate::ui::DIFF_LINE) {
            Some(offset) => {
                work.diff.scroll_to = Some(offset);
                self.ctx.request_repaint();
            }
            None => {
                let report = bench.report(&format!("diff of {path}, {rows} rows"));
                crate::app_work::perf_log(&report);
                self.bench = None;
                self.info_toast("Scroll benchmark finished (see FACET_PERF_LOG)");
            }
        }
    }

    /// Log time-to-first-frame when `FACET_STARTUP_LOG` names a file (used for profiling the
    /// cold-start budget). The second `ui` call means the first frame has been presented.
    fn trace_startup(&mut self) {
        self.frames += 1;
        if self.frames == 1 {
            self.ctx.request_repaint();
        } else if self.frames == 2 {
            if let Some(path) = std::env::var_os("FACET_STARTUP_LOG") {
                let line = format!(
                    "first frame presented after {:.1} ms on {}
",
                    self.startup.elapsed().as_secs_f64() * 1000.0,
                    self.gpu
                );
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
            }
        }
    }
}

impl eframe::App for FacetApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.persist_settings();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.trace_startup();
        self.step_scroll_bench(frame.info().cpu_usage);
        self.sync_appearance(&ctx);
        self.sync_zoom(&ctx);
        self.handle_shortcuts(&ctx);
        self.handle_dropped_files(&ctx);
        self.refresh_on_focus(&ctx);
        self.show_next_askpass();

        self.layout(ui);

        for action in std::mem::take(&mut self.pending) {
            self.run(action);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fully transparent: the Mica backdrop shows wherever a view paints nothing.
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self) {
        for tab in &self.tabs {
            cancel_tab_work(tab);
        }
        // Prompts nobody will answer: let git fail instead of waiting.
        for request in self.askpass.drain(..) {
            request.answer(None);
        }
        if let Some(chrome) = self.chrome {
            platform::clear_notification(chrome.raw());
        }
        self.capture_window_placement();
        if self.settings != self.saved_settings || self.settings_dirty_since.is_some() {
            let _ = self.settings.save();
        }
    }
}

/// Git, set up so credential prompts reach the person (G7): Git Credential Manager's own
/// windows open modal to Facet's window, and every other prompt (username without a helper,
/// SSH passphrase, unknown host key) comes to Facet's prompt dialog through the askpass
/// helper, which is Facet's own executable.
fn credential_git(
    git: Git,
    runtime: &tokio::runtime::Runtime,
    chrome: Option<Chrome>,
    events: &Sender<Event>,
    ctx: &egui::Context,
) -> Git {
    let mut vars: Vec<(OsString, OsString)> = Vec::new();
    if let Some(chrome) = chrome {
        vars.push((
            "GCM_MODAL_PARENTHWND".into(),
            chrome.raw().to_string().into(),
        ));
    }
    if let Ok(exe) = std::env::current_exe() {
        let _entered = runtime.enter();
        let (tx, ctx) = (events.clone(), ctx.clone());
        // A request that can't be delivered is dropped, which cancels it: git then fails
        // with its own message instead of waiting forever.
        let started = askpass::start(&exe, move |request| {
            if tx.send(Event::Askpass(request)).is_ok() {
                ctx.request_repaint();
            }
        });
        if let Ok(env) = started {
            vars.extend(env.vars);
        }
    }
    git.with_envs(&vars)
}

/// Stop everything still running for a tab that is going away (git processes die with their
/// job objects, G6).
fn cancel_tab_work(tab: &RepoTab) {
    let tokens = [
        tab.load.as_ref(),
        tab.work.diff.cancel.as_ref(),
        tab.history.cancel.as_ref(),
        tab.history.commit.cancel.as_ref(),
        tab.history.commit.diff.cancel.as_ref(),
        tab.network.as_ref().map(|network| &network.cancel),
        tab.power
            .blame
            .as_ref()
            .and_then(|blame| blame.cancel.as_ref()),
    ];
    for token in tokens.into_iter().flatten() {
        token.cancel();
    }
}

/// Turn a git error into a toast: a plain-language title plus git's own words as detail.
pub(crate) fn describe_error(error: &GitError) -> (String, Option<String>) {
    match error {
        GitError::NotARepository { path, detail } => (
            format!("“{}” is not a Git repository", display_name(path)),
            Some(if detail.is_empty() {
                path.display().to_string()
            } else {
                format!("{}\n{detail}", path.display())
            }),
        ),
        GitError::PathNotFound { path } => (
            format!("“{}” could not be found", display_name(path)),
            Some(path.display().to_string()),
        ),
        GitError::Spawn { .. } => (
            "Git could not be started".to_owned(),
            Some(error.to_string()),
        ),
        GitError::Failed {
            command, stderr, ..
        } => (
            "A git command failed".to_owned(),
            Some(format!("{command}\n{stderr}")),
        ),
        other => ("Something went wrong".to_owned(), Some(other.to_string())),
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_described_in_plain_language() {
        let (title, detail) = describe_error(&GitError::NotARepository {
            path: PathBuf::from(r"D:\Stuff\Photos"),
            detail: "fatal: not a git repository".into(),
        });
        assert_eq!(title, "“Photos” is not a Git repository");
        let detail = detail.unwrap();
        assert!(detail.contains(r"D:\Stuff\Photos"));
        assert!(detail.contains("fatal: not a git repository"));

        let (title, detail) = describe_error(&GitError::Failed {
            command: "git log".into(),
            code: 128,
            stderr: "fatal: bad revision".into(),
        });
        assert_eq!(title, "A git command failed");
        assert_eq!(detail.unwrap(), "git log\nfatal: bad revision");
    }
}
