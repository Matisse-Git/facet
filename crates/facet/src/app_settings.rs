//! Settings and integrations (M4): the settings view's state and actions, the git executable,
//! interface size and code font, opening the repository in other programs, external tool
//! presets, `.gitignore` templates, and paths handed over by a second launch.

use std::path::PathBuf;

use eframe::egui;
use facet_git::parse::GitVersion;
use facet_git::GitError;

use crate::actions::Action;
use crate::app::{describe_error, Event, FacetApp, GitStatus};
use crate::gitignore::{self, Template, Written};
use crate::integrations::{self, Installed, OpenTarget, Places};
use crate::platform::{self, Chrome};
use crate::settings::{CodeFont, UI_SCALE};

/// The open settings view.
pub struct SettingsView {
    /// The git executable as typed (empty: git on `PATH`).
    pub git_path: String,
    pub checking_git: bool,
    /// Why the last checked executable can't be used.
    pub git_error: Option<String>,
    pub template: Template,
    pub preview: GitignorePreview,
    /// Scroll the `.gitignore` section into view on the next frame.
    pub reveal_gitignore: bool,
}

pub enum GitignorePreview {
    /// No repository with a working tree is open.
    None,
    Loading,
    /// The rules that would be appended (empty: none are missing).
    Ready(String),
    Failed(String),
}

/// Results of settings work done in the background.
pub enum SettingsEvent {
    GitChecked {
        path: Option<PathBuf>,
        result: Result<GitVersion, GitError>,
    },
    GitPicked(Result<Option<PathBuf>, String>),
    Installed(Installed),
    Fonts {
        font: CodeFont,
        definitions: egui::FontDefinitions,
    },
    Preview {
        root: PathBuf,
        template: Template,
        result: Result<String, String>,
    },
    GitignoreWritten {
        root: PathBuf,
        template: Template,
        result: Result<Option<Written>, String>,
    },
    GitignoreRestored(Result<(), String>),
    /// A second launch handed over (`None`: it had no path).
    Forwarded(Option<PathBuf>),
}

impl FacetApp {
    /// Run `work` on a blocking thread and deliver its result as a settings event.
    fn settings_work(&self, work: impl FnOnce() -> SettingsEvent + Send + 'static) {
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn_blocking(move || {
            if tx.send(Event::Settings(work())).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Look for Windows Terminal, VS Code, Rider and the diff / merge tools.
    pub(crate) fn detect_integrations(&self) {
        self.settings_work(|| SettingsEvent::Installed(integrations::detect(&Places::from_env())));
    }

    /// The working tree root of the active repository (none for bare repositories).
    fn work_root(&self) -> Option<PathBuf> {
        let tab = self.active_tab()?;
        crate::app_work::ready_info(tab).map(|info| info.root().to_owned())
    }

    fn show_settings(&mut self, template: Option<Template>) {
        if self.settings_view.is_none() {
            self.detect_integrations();
        }
        let view = self.settings_view.get_or_insert_with(|| SettingsView {
            git_path: self
                .settings
                .git_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            checking_git: false,
            git_error: None,
            template: Template::Unity,
            preview: GitignorePreview::None,
            reveal_gitignore: false,
        });
        if let Some(template) = template {
            view.template = template;
            view.reveal_gitignore = true;
        }
        self.palette.open = false;
        self.request_gitignore_preview();
    }

    /// Recompute the `.gitignore` preview for the active repository and chosen template.
    pub(crate) fn request_gitignore_preview(&mut self) {
        let root = self.work_root();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(root) = root else {
            view.preview = GitignorePreview::None;
            return;
        };
        view.preview = GitignorePreview::Loading;
        let template = view.template;
        self.settings_work(move || {
            let result = gitignore::read(&root)
                .map(|text| gitignore::addition(text.as_deref().unwrap_or(""), template))
                .map_err(|error| error.to_string());
            SettingsEvent::Preview {
                root,
                template,
                result,
            }
        });
    }

    fn set_git_path(&mut self, path: Option<PathBuf>) {
        let git = self
            .git
            .with_exe(path.clone().unwrap_or_else(|| PathBuf::from("git")));
        if let Some(view) = &mut self.settings_view {
            view.checking_git = true;
            view.git_error = None;
        }
        self.spawn(async move {
            let result = facet_git::version(&git).await;
            Event::Settings(SettingsEvent::GitChecked { path, result })
        });
    }

    fn browse_git_path(&mut self) {
        let owner = self.chrome.map(Chrome::raw);
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("facet-file-picker".into())
            .spawn(move || {
                let result = platform::pick_file(owner, "Choose Git", "Git (git.exe)", "git.exe");
                if tx
                    .send(Event::Settings(SettingsEvent::GitPicked(result)))
                    .is_ok()
                {
                    ctx.request_repaint();
                }
            });
        if let Err(error) = spawned {
            self.error_toast("Couldn't show the file picker", Some(error.to_string()));
        }
    }

    fn set_code_font(&mut self, font: CodeFont) {
        self.settings.code_font = font;
        // Reading the font files takes a moment: build the fonts off the UI thread.
        self.settings_work(move || SettingsEvent::Fonts {
            font,
            definitions: crate::theme::font_definitions(font),
        });
    }

    fn open_in(&mut self, target: OpenTarget) {
        let Some(root) = self.active_tab().map(|tab| match &tab.state {
            crate::app::TabState::Ready { info, .. } => info.root().to_owned(),
            crate::app::TabState::Loading => tab.path.clone(),
        }) else {
            return;
        };
        let Some(exe) = self.installed.launcher(target).map(ToOwned::to_owned) else {
            self.error_toast(
                format!("{} wasn't found", target.label()),
                Some("Install it, then open Settings to look again.".to_owned()),
            );
            return;
        };
        if let Err(error) = integrations::launch(target, &exe, &root) {
            self.error_toast(
                format!("Couldn't start {}", target.label()),
                Some(format!("{}: {error}", exe.display())),
            );
        }
    }

    fn add_gitignore_template(&mut self, template: Template) {
        let Some(root) = self.work_root() else {
            return;
        };
        self.settings_work(move || {
            let result = gitignore::append(&root, template).map_err(|error| error.to_string());
            SettingsEvent::GitignoreWritten {
                root,
                template,
                result,
            }
        });
    }

    pub(crate) fn handle_settings_event(&mut self, event: SettingsEvent) {
        match event {
            SettingsEvent::GitChecked { path, result } => self.on_git_checked(path, result),
            SettingsEvent::GitPicked(Ok(Some(path))) => {
                if let Some(view) = &mut self.settings_view {
                    view.git_path = path.display().to_string();
                }
                self.set_git_path(Some(path));
            }
            SettingsEvent::GitPicked(Ok(None)) => {}
            SettingsEvent::GitPicked(Err(detail)) => {
                self.error_toast("Couldn't show the file picker", Some(detail));
            }
            SettingsEvent::Installed(installed) => self.installed = installed,
            SettingsEvent::Fonts { font, definitions } => {
                // A later choice may have been made while these loaded.
                if font == self.settings.code_font {
                    self.ctx.set_fonts(definitions);
                }
            }
            SettingsEvent::Preview {
                root,
                template,
                result,
            } => {
                let current = self.work_root();
                if let Some(view) = &mut self.settings_view {
                    let same_root = current
                        .as_deref()
                        .is_some_and(|current| crate::settings::same_path(current, &root));
                    if same_root && view.template == template {
                        view.preview = match result {
                            Ok(block) => GitignorePreview::Ready(block),
                            Err(detail) => GitignorePreview::Failed(detail),
                        };
                    }
                }
            }
            SettingsEvent::GitignoreWritten {
                root,
                template,
                result,
            } => {
                match result {
                    Ok(Some(change)) => self.action_toast(
                        format!("Added {} rules to .gitignore", template.label()),
                        "Undo",
                        Action::RestoreGitignore { root, change },
                    ),
                    Ok(None) => self.info_toast(format!(
                        ".gitignore already has every {} rule",
                        template.label()
                    )),
                    Err(detail) => self.error_toast("Couldn't write .gitignore", Some(detail)),
                }
                self.request_gitignore_preview();
            }
            SettingsEvent::GitignoreRestored(result) => {
                match result {
                    Ok(()) => self.info_toast(".gitignore restored"),
                    Err(detail) => self.error_toast("Couldn't restore .gitignore", Some(detail)),
                }
                self.request_gitignore_preview();
            }
            SettingsEvent::Forwarded(path) => {
                if let Some(chrome) = self.chrome {
                    chrome.bring_to_front();
                }
                if let Some(path) = path {
                    self.open_path(path, true);
                }
            }
        }
    }

    fn on_git_checked(&mut self, path: Option<PathBuf>, result: Result<GitVersion, GitError>) {
        if let Some(view) = &mut self.settings_view {
            view.checking_git = false;
        }
        match result {
            Ok(version) => {
                self.git = self
                    .git
                    .with_exe(path.clone().unwrap_or_else(|| PathBuf::from("git")));
                let place = path
                    .as_ref()
                    .map_or_else(|| "PATH".to_owned(), |path| path.display().to_string());
                self.settings.git_path = path;
                self.git_status = GitStatus::Found(version);
                if let Some(view) = &mut self.settings_view {
                    view.git_error = None;
                    view.git_path = self
                        .settings
                        .git_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default();
                }
                self.info_toast(format!("Using Git {version} from {place}"));
            }
            Err(error) => {
                let (title, detail) = describe_error(&error);
                let message = detail.clone().unwrap_or_else(|| title.clone());
                if let Some(view) = &mut self.settings_view {
                    view.git_error = Some(message);
                }
                self.error_toast(
                    "That Git can't be used; nothing was changed",
                    detail.or(Some(title)),
                );
            }
        }
    }

    /// Keep the interface size setting in step with egui's own Ctrl+= / Ctrl+- / Ctrl+0.
    pub(crate) fn sync_zoom(&mut self, ctx: &egui::Context) {
        let zoom = ctx.zoom_factor();
        if (zoom - self.settings.ui_scale).abs() > 0.001 {
            let clamped = zoom.clamp(*UI_SCALE.start(), *UI_SCALE.end());
            if (clamped - zoom).abs() > 0.001 {
                ctx.set_zoom_factor(clamped);
            }
            self.settings.ui_scale = clamped;
        }
    }

    /// Handle settings and integration actions (they work without a repository). Returns the
    /// action back if it is not one of them.
    pub(crate) fn run_settings_action(&mut self, action: Action) -> Option<Action> {
        match action {
            Action::ShowSettings => self.show_settings(None),
            Action::CloseSettings => self.settings_view = None,
            Action::SetUiScale(scale) => {
                let scale = scale.clamp(*UI_SCALE.start(), *UI_SCALE.end());
                self.settings.ui_scale = scale;
                self.ctx.set_zoom_factor(scale);
            }
            Action::SetCodeFont(font) => self.set_code_font(font),
            Action::SetGitPath(path) => self.set_git_path(path),
            Action::BrowseGitPath => self.browse_git_path(),
            Action::SetPullMode(mode) => self.settings.pull_mode = mode,
            Action::AddExternalToolPreset(preset) => {
                let found = self.installed.preset(preset).map(ToOwned::to_owned);
                let exe = found.as_ref().map_or_else(
                    || preset.exe_name().to_owned(),
                    |path| path.display().to_string(),
                );
                let note = if found.is_some() {
                    format!("Found {} at {exe}.", preset.label())
                } else {
                    format!(
                        "{} wasn't found in the usual folders: put its full path in the commands.",
                        preset.label()
                    )
                };
                self.add_external_tool(Some((preset.tool(&exe), note)));
            }
            Action::OpenIn(target) => self.open_in(target),
            Action::ShowGitignoreTemplate(template) => self.show_settings(Some(template)),
            Action::AddGitignoreTemplate(template) => self.add_gitignore_template(template),
            Action::RestoreGitignore { root, change } => {
                self.settings_work(move || {
                    SettingsEvent::GitignoreRestored(
                        gitignore::restore(&root, &change).map_err(|error| error.to_string()),
                    )
                });
            }
            other => return Some(other),
        }
        None
    }
}
