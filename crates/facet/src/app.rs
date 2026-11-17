//! Application state and behavior. Views live in `crate::ui` as further `impl FacetApp` blocks.
//!
//! Threading model: this struct lives on the UI thread and never blocks. Anything slow (git,
//! file dialogs, disk writes) runs elsewhere and reports back as an [`Event`] over a channel;
//! the sender wakes the UI with `request_repaint`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use facet_git::parse::GitVersion;
use facet_git::{CancellationToken, CommandLog, CommandRecord, Git, GitError, Head, RepoInfo};

use crate::actions::{Action, Command, CommandContext, GLOBAL_SHORTCUTS};
use crate::platform::{self, Chrome};
use crate::settings::{same_path, Settings};
use crate::theme::{self, Palette};
use crate::ui::palette::PaletteState;
use crate::ui::toast::{Toast, ToastKind};

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

    runtime: tokio::runtime::Runtime,
    git: Git,
    pub command_log: Arc<CommandLog>,
    /// Snapshot of the command log for the panel, rebuilt only when the log's revision changes.
    pub log_view: (u64, Vec<CommandRecord>),
    events_tx: Sender<Event>,
    events_rx: Receiver<Event>,
    ctx: egui::Context,

    pub tabs: Vec<RepoTab>,
    pub active: Option<usize>,
    next_tab_id: u64,

    pub git_status: GitStatus,
    pub palette: PaletteState,
    pub toasts: Vec<Toast>,
    next_toast_id: u64,
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
}

impl FacetApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        runtime: tokio::runtime::Runtime,
        startup: Instant,
        open_on_launch: Option<PathBuf>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx, settings.theme);

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
        let git = Git::new("git", command_log.clone());
        let (events_tx, events_rx) = std::sync::mpsc::channel();

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
        };

        app.check_git();
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
    fn spawn(&self, future: impl std::future::Future<Output = Event> + Send + 'static) {
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
                if !self.tabs[index].is_ready() && !self.tabs[index].restored {
                    self.settings.touch_recent(&root);
                }
                let tab = &mut self.tabs[index];
                tab.path = root;
                tab.state = TabState::Ready { info, head };
                tab.load = None;
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

    fn remove_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        if let Some(load) = tab.load {
            load.cancel();
        }
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
        crate::actions::commands(&CommandContext {
            tabs: &names,
            active_tab: self.active,
            repo_ready: self.active_tab().is_some_and(RepoTab::is_ready),
            recent: &self.settings.recent_repos,
            theme: self.settings.theme,
            sidebar_visible: self.settings.sidebar_visible,
            command_log_visible: self.settings.command_log_visible,
            command_log_empty: self.command_log.is_empty(),
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
                    self.load_tab(index);
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
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        for (shortcut, action) in GLOBAL_SHORTCUTS {
            if ctx.input_mut(|input| input.consume_shortcut(shortcut)) {
                // While the palette is open it owns the keyboard, except its own toggle.
                if !self.palette.open || *action == Action::ShowPalette {
                    self.pending.push(action.clone());
                }
            }
        }
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

    /// Returning to the window is the cheapest moment to notice outside changes (a checkout in
    /// a terminal, say) until the filesystem watcher lands with status in M1.
    fn refresh_on_focus(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused {
            if let Some(index) = self.active.filter(|i| self.tabs[*i].is_ready()) {
                self.load_tab(index);
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.trace_startup();
        self.sync_appearance(&ctx);
        self.handle_shortcuts(&ctx);
        self.handle_dropped_files(&ctx);
        self.refresh_on_focus(&ctx);

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
            if let Some(load) = &tab.load {
                load.cancel();
            }
        }
        self.capture_window_placement();
        if self.settings != self.saved_settings || self.settings_dirty_since.is_some() {
            let _ = self.settings.save();
        }
    }
}

/// Turn a git error into a toast: a plain-language title plus git's own words as detail.
fn describe_error(error: &GitError) -> (String, Option<String>) {
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
