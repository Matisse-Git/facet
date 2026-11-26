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
use crate::settings::{CodeFont, Density, UI_SCALE};
use crate::ui::prompt::{Prompt, PromptField, PromptKind};

/// The hosting services settings can sign in to: display name and host.
pub const ACCOUNT_HOSTS: [(&str, &str); 2] = [("GitHub", "github.com"), ("GitLab", "gitlab.com")];

/// Which page of settings is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsPage {
    #[default]
    Main,
    /// Git executable, interface size, diff and merge tools, open in, `.gitignore` templates.
    Advanced,
}

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
    pub page: SettingsPage,
    /// The "Find a setting" box.
    pub find: String,
    /// The default branch name as typed.
    pub default_branch: String,
    /// Accounts the credential store holds per host of [`ACCOUNT_HOSTS`] (`None`: still
    /// reading).
    pub accounts: Option<Vec<Vec<String>>>,
    /// A sign-in or sign-out running on this host.
    pub account_busy: Option<String>,
    /// Who commits are made as (`None`: still reading).
    pub identity: Option<facet_git::Identity>,
    /// `git lfs version` (`None`: still checking; `Some(None)`: not installed).
    pub lfs: Option<Option<String>>,
    /// The active repository's `core.autocrlf` (`None`: reading, or no repository).
    pub line_endings: Option<facet_git::LineEndings>,
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
    Accounts(Vec<Vec<String>>),
    Identity(Result<facet_git::Identity, GitError>),
    SignedIn {
        host: String,
        result: Result<String, GitError>,
    },
    SignedOut {
        host: String,
        result: Result<(), GitError>,
    },
    /// The commit author of a repository was set or removed (`done`: the toast on success).
    AuthorSet {
        root: PathBuf,
        done: String,
        result: Result<(), GitError>,
    },
    /// The author in the global Git config, for the first saved identity.
    GlobalIdentity(Result<facet_git::Identity, GitError>),
    Lfs(Option<String>),
    LineEndings(Option<facet_git::LineEndings>),
    LineEndingsSet(Result<(), GitError>),
    /// The background fetch of a repository finished (`Err`: it failed; nothing is shown).
    BackgroundFetched {
        root: PathBuf,
        ok: bool,
    },
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
            page: SettingsPage::Main,
            find: String::new(),
            default_branch: self.settings.default_branch.clone(),
            accounts: None,
            account_busy: None,
            identity: None,
            lfs: None,
            line_endings: None,
        });
        if let Some(template) = template {
            view.template = template;
            view.page = SettingsPage::Advanced;
            view.reveal_gitignore = true;
        }
        self.palette.open = false;
        self.request_gitignore_preview();
        self.read_accounts();
        self.read_identity();
        self.read_line_endings();
        if self.settings_view.as_ref().is_some_and(|view| view.lfs.is_none()) {
            let git = self.git.clone();
            self.spawn(async move {
                Event::Settings(SettingsEvent::Lfs(facet_git::lfs_version(&git).await))
            });
        }
    }

    /// `core.autocrlf` in the active repository.
    fn read_line_endings(&self) {
        let Some(root) = self.work_root() else {
            return;
        };
        let git = self.git.clone();
        self.spawn(async move {
            Event::Settings(SettingsEvent::LineEndings(
                facet_git::line_endings(&git, &root).await.ok(),
            ))
        });
    }

    fn set_line_endings(&mut self, mode: facet_git::LineEndings) {
        let Some(root) = self.work_root() else {
            return;
        };
        if let Some(view) = &mut self.settings_view {
            view.line_endings = Some(mode);
        }
        let git = self.git.clone();
        self.spawn(async move {
            Event::Settings(SettingsEvent::LineEndingsSet(
                facet_git::set_line_endings(&git, &root, mode).await,
            ))
        });
    }

    /// Versions and paths for a bug report, on the clipboard.
    fn copy_diagnostics(&mut self) {
        let git = match &self.git_status {
            GitStatus::Checking => "checking".to_owned(),
            GitStatus::Found(version) => version.to_string(),
            GitStatus::Missing(_) => "not found".to_owned(),
        };
        let lfs = self
            .settings_view
            .as_ref()
            .and_then(|view| view.lfs.clone())
            .flatten()
            .unwrap_or_else(|| "not installed or not checked".to_owned());
        let text = format!(
            "{}
Windows {}
Git {git} ({})
Git LFS: {lfs}
Interface size: {:.0}%
Theme: {:?}
",
            crate::ui::version_label(),
            std::env::consts::ARCH,
            self.settings
                .git_path
                .as_ref()
                .map_or_else(|| "PATH".to_owned(), |path| path.display().to_string()),
            self.settings.ui_scale * 100.0,
            self.settings.theme,
        );
        self.ctx.copy_text(text);
        self.info_toast("Diagnostics copied");
    }

    /// Who is signed in to each host of [`ACCOUNT_HOSTS`] (the Windows credential store).
    fn read_accounts(&self) {
        self.settings_work(|| {
            SettingsEvent::Accounts(
                ACCOUNT_HOSTS
                    .iter()
                    .map(|(_, host)| facet_git::stored_accounts(host))
                    .collect(),
            )
        });
    }

    /// The commit author in the active repository (or the global one without a repository).
    fn read_identity(&self) {
        let git = self.git.clone();
        let root = self.work_root();
        self.spawn(async move {
            Event::Settings(SettingsEvent::Identity(
                facet_git::identity(&git, root.as_deref()).await,
            ))
        });
    }

    fn sign_in(&mut self, host: String) {
        if let Some(view) = &mut self.settings_view {
            if view.account_busy.is_some() {
                return;
            }
            view.account_busy = Some(host.clone());
        }
        let git = self.git.clone();
        self.spawn(async move {
            let result = facet_git::sign_in(&git, &host).await;
            Event::Settings(SettingsEvent::SignedIn { host, result })
        });
    }

    fn sign_out(&mut self, host: String) {
        if let Some(view) = &mut self.settings_view {
            if view.account_busy.is_some() {
                return;
            }
            view.account_busy = Some(host.clone());
        }
        let git = self.git.clone();
        self.spawn(async move {
            let result = facet_git::sign_out(&git, &host, None).await;
            Event::Settings(SettingsEvent::SignedOut { host, result })
        });
    }

    /// Who commits are made as in the active repository, as last read (the Changes tab's, else
    /// the settings view's).
    fn current_identity(&self) -> Option<facet_git::Identity> {
        self.active_tab()
            .and_then(|tab| tab.work.identity.clone())
            .or_else(|| {
                self.settings_view
                    .as_ref()
                    .and_then(|view| view.identity.clone())
            })
    }

    fn edit_commit_author(&mut self) {
        if self.work_root().is_none() {
            self.info_toast("Open a repository first: Facet sets the author per repository");
            return;
        }
        let (name, email) = self
            .current_identity()
            .map(|identity| (identity.name, identity.email))
            .unwrap_or_default();
        self.prompt = Some(
            Prompt::new(PromptKind::CommitAuthor, "Commit Author", "Save")
                .body(
                    "Saved in this repository's own Git config. Facet never changes your \
                     global Git config.",
                )
                .field(PromptField::new("Name", "Your name", true).with_value(&name))
                .field(PromptField::new("Email", "you@example.com", true).with_value(&email)),
        );
    }

    fn set_commit_author(&mut self, name: String, email: String, done: String) {
        let Some(root) = self.work_root() else {
            self.info_toast("Open a repository first: Facet sets the author per repository");
            return;
        };
        let git = self.git.clone();
        self.spawn(async move {
            let result =
                facet_git::set_local_identity(&git, &root, name.trim(), email.trim()).await;
            Event::Settings(SettingsEvent::AuthorSet { root, done, result })
        });
    }

    /// Commit as saved identity `index` in the active repository.
    fn use_identity(&mut self, index: usize) {
        let Some(saved) = self.settings.identities.get(index).cloned() else {
            return;
        };
        self.set_commit_author(
            saved.name,
            saved.email,
            format!("Committing as {} in this repository", saved.label),
        );
    }

    /// Drop the active repository's own author: the global one applies again.
    fn use_global_identity(&mut self) {
        let Some(root) = self.work_root() else {
            return;
        };
        let git = self.git.clone();
        self.spawn(async move {
            let result = facet_git::unset_local_identity(&git, &root).await;
            Event::Settings(SettingsEvent::AuthorSet {
                root,
                done: "This repository uses your global identity again".to_owned(),
                result,
            })
        });
    }

    /// The first run takes the global author as the "Work" identity (once: an emptied list
    /// stays empty).
    pub(crate) fn seed_identities(&self) {
        if self.settings.identities_seeded {
            return;
        }
        let git = self.git.clone();
        self.spawn(async move {
            Event::Settings(SettingsEvent::GlobalIdentity(
                facet_git::global_identity(&git).await,
            ))
        });
    }

    /// Ask for a new saved identity (`None`) or edit one.
    fn edit_identity(&mut self, index: Option<usize>) {
        let saved = match index {
            Some(index) => match self.settings.identities.get(index) {
                Some(saved) => saved.clone(),
                None => return,
            },
            // A new identity starts from the current name: usually only the email differs.
            None => crate::settings::SavedIdentity {
                name: self
                    .current_identity()
                    .map(|identity| identity.name)
                    .unwrap_or_default(),
                ..Default::default()
            },
        };
        let (title, submit) = if index.is_some() {
            ("Edit Identity", "Save")
        } else {
            ("Add Identity", "Add")
        };
        self.palette.open = false;
        self.prompt = Some(
            Prompt::new(PromptKind::Identity { index }, title, submit)
                .body(
                    "Pick it per repository from the Changes tab; it is written to that \
                     repository's own Git config. Hosts are optional: Facet warns when a \
                     repository on one of them commits as someone else.",
                )
                .field(PromptField::new("Label", "Work, GitHub…", true).with_value(&saved.label))
                .field(PromptField::new("Name", "Your name", true).with_value(&saved.name))
                .field(
                    PromptField::new(
                        "Email",
                        "you@example.com, or ID+username@users.noreply.github.com",
                        true,
                    )
                    .with_value(&saved.email),
                )
                .field(
                    PromptField::new("Hosts (optional)", "github.com, *.example.co.jp", false)
                        .with_value(&saved.hosts.join(", ")),
                ),
        );
    }

    fn save_identity(&mut self, index: Option<usize>, identity: crate::settings::SavedIdentity) {
        let label = identity.label.clone();
        match index {
            Some(index) if index < self.settings.identities.len() => {
                self.settings.identities[index] = identity;
                self.info_toast(format!("Saved {label}"));
            }
            Some(_) => {}
            None => {
                self.settings.identities.push(identity);
                self.info_toast(format!("Added {label}"));
            }
        }
    }

    fn remove_identity(&mut self, index: usize) {
        if index < self.settings.identities.len() {
            let saved = self.settings.identities.remove(index);
            self.info_toast(format!(
                "Removed {}; repositories that use it keep their author",
                saved.label
            ));
        }
    }

    /// Fetch every ready repository whose last fetch is older than the interval in settings.
    /// Called every frame; does nothing most of the time.
    pub(crate) fn background_fetch(&mut self) {
        if !self.settings.auto_fetch || !matches!(self.git_status, GitStatus::Found(_)) {
            return;
        }
        let interval = std::time::Duration::from_secs(u64::from(self.settings.fetch_minutes) * 60);
        let now = std::time::Instant::now();
        let mut due = Vec::new();
        for tab in &mut self.tabs {
            let Some(root) = crate::app_work::ready_info(tab).map(|info| info.root().to_owned())
            else {
                continue;
            };
            if tab.refs.snapshot.remotes.is_empty() || tab.network.is_some() {
                continue;
            }
            match tab.auto_fetch {
                Some(last) if now.duration_since(last) < interval => continue,
                // The first fetch waits a little after the repository opens.
                None => {
                    tab.auto_fetch = Some(now - interval + std::time::Duration::from_secs(20));
                    continue;
                }
                _ => {}
            }
            tab.auto_fetch = Some(now);
            due.push(root);
        }
        for root in due {
            let git = self.git.clone();
            self.spawn(async move {
                let ok = facet_git::fetch_in_background(&git, &root).await.is_ok();
                Event::Settings(SettingsEvent::BackgroundFetched { root, ok })
            });
        }
        self.ctx.request_repaint_after(std::time::Duration::from_secs(10));
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
            SettingsEvent::Accounts(accounts) => {
                if let Some(view) = &mut self.settings_view {
                    view.accounts = Some(accounts);
                }
            }
            SettingsEvent::Identity(result) => {
                if let Some(view) = &mut self.settings_view {
                    view.identity = Some(result.unwrap_or_default());
                }
            }
            SettingsEvent::SignedIn { host, result } => {
                if let Some(view) = &mut self.settings_view {
                    view.account_busy = None;
                }
                match result {
                    Ok(user) if user.is_empty() => self.info_toast(format!("Signed in to {host}")),
                    Ok(user) => self.info_toast(format!("Signed in to {host} as {user}")),
                    Err(error) => {
                        let (title, detail) = describe_error(&error);
                        self.error_toast(format!("Couldn't sign in to {host}"), detail.or(Some(title)));
                    }
                }
                self.read_accounts();
            }
            SettingsEvent::SignedOut { host, result } => {
                if let Some(view) = &mut self.settings_view {
                    view.account_busy = None;
                }
                match result {
                    Ok(()) => self.info_toast(format!("Signed out of {host}")),
                    Err(error) => {
                        let (title, detail) = describe_error(&error);
                        self.error_toast(format!("Couldn't sign out of {host}"), detail.or(Some(title)));
                    }
                }
                self.read_accounts();
            }
            SettingsEvent::AuthorSet { root, done, result } => {
                match result {
                    Ok(()) => self.info_toast(done),
                    Err(error) => {
                        let (title, detail) = describe_error(&error);
                        self.error_toast("Couldn't set the commit author", detail.or(Some(title)));
                    }
                }
                if self.settings_view.is_some() {
                    self.read_identity();
                }
                let found = self.tabs.iter().position(|tab| {
                    crate::app_work::ready_info(tab)
                        .is_some_and(|info| crate::settings::same_path(info.root(), &root))
                });
                if let Some(index) = found {
                    self.request_identity(index);
                }
            }
            SettingsEvent::GlobalIdentity(result) => {
                if let Ok(identity) = result {
                    if !self.settings.identities_seeded {
                        self.settings.identities_seeded = true;
                        if !identity.is_unset() && self.settings.identities.is_empty() {
                            self.settings.identities.push(crate::settings::SavedIdentity {
                                label: "Work".to_owned(),
                                name: identity.name,
                                email: identity.email,
                                hosts: Vec::new(),
                            });
                        }
                    }
                }
            }
            SettingsEvent::Lfs(version) => {
                if let Some(view) = &mut self.settings_view {
                    view.lfs = Some(version);
                }
            }
            SettingsEvent::LineEndings(mode) => {
                if let Some(view) = &mut self.settings_view {
                    view.line_endings = mode;
                }
            }
            SettingsEvent::LineEndingsSet(result) => {
                if let Err(error) = result {
                    let (title, detail) = describe_error(&error);
                    self.error_toast("Couldn't change line endings", detail.or(Some(title)));
                }
                self.read_line_endings();
            }
            SettingsEvent::BackgroundFetched { root, ok } => {
                if ok {
                    let found = self.tabs.iter().position(|tab| {
                        crate::app_work::ready_info(tab).is_some_and(|info| {
                            crate::settings::same_path(info.root(), &root)
                        })
                    });
                    if let Some(index) = found {
                        self.request_refs(index);
                    }
                }
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
            Action::ShowSettings => {
                self.show_settings(None);
                if let Some(view) = &mut self.settings_view {
                    view.page = SettingsPage::Main;
                }
            }
            Action::ShowAdvancedSettings => {
                self.show_settings(None);
                if let Some(view) = &mut self.settings_view {
                    view.page = SettingsPage::Advanced;
                }
            }
            Action::SetWindowsAccent(on) => {
                self.settings.windows_accent = on;
                crate::theme::set_accent(&self.ctx, on.then(platform::windows_accent).flatten());
            }
            Action::SetCodeFontSize(size) => {
                self.settings.code_font_size = size.clamp(10.0, 20.0);
                crate::theme::set_mono_size(self.settings.code_font_size);
                crate::theme::restyle(&self.ctx);
            }
            Action::SetDensity(density) => {
                self.settings.density = density;
                crate::theme::set_compact(density == Density::Compact);
            }
            Action::SetAutoFetch(on) => self.settings.auto_fetch = on,
            Action::SetFetchMinutes(minutes) => {
                self.settings.fetch_minutes = minutes.clamp(1, 240);
                self.settings.auto_fetch = true;
            }
            Action::SetDefaultBranch(name) => {
                if !self.settings.set_default_branch(&name) {
                    self.error_toast(
                        format!("“{}” can't be a branch name", name.trim()),
                        Some("Branch names can't have spaces, “..”, or ~ ^ : ? * [ \\.".to_owned()),
                    );
                    if let Some(view) = &mut self.settings_view {
                        view.default_branch = self.settings.default_branch.clone();
                    }
                }
            }
            Action::SetEditor(editor) => self.settings.editor = editor,
            Action::SetTerminal(terminal) => self.settings.terminal = terminal,
            Action::SignIn(host) => self.sign_in(host),
            Action::SignOut(host) => self.sign_out(host),
            Action::EditCommitAuthor => self.edit_commit_author(),
            Action::SetLineEndings(mode) => self.set_line_endings(mode),
            Action::CopyDiagnostics => self.copy_diagnostics(),
            Action::SetCommitAuthor { name, email } => self.set_commit_author(
                name,
                email,
                "Commit author saved for this repository".to_owned(),
            ),
            Action::UseIdentity(index) => self.use_identity(index),
            Action::UseGlobalIdentity => self.use_global_identity(),
            Action::EditIdentity(index) => self.edit_identity(index),
            Action::SaveIdentity { index, identity } => self.save_identity(index, identity),
            Action::RemoveIdentity(index) => self.remove_identity(index),
            Action::ManageCredentials => {
                if let Err(error) = platform::open_credential_manager() {
                    self.error_toast("Couldn't open Credential Manager", Some(error.to_string()));
                }
            }
            Action::OpenTerminal => {
                if let Some(root) = self.work_root() {
                    let git = self.git.exe().to_owned();
                    let git = which_git(&git);
                    if let Err(error) = integrations::open_terminal(
                        self.settings.terminal,
                        &self.installed,
                        &root,
                        &git,
                    ) {
                        self.error_toast(
                            format!("Couldn't open {}", self.settings.terminal.label()),
                            Some(error.to_string()),
                        );
                    }
                }
            }
            Action::OpenInEditor(file) => {
                if let Some(root) = self.work_root() {
                    let file = file.map(PathBuf::from);
                    if let Err(error) = integrations::open_in_editor(
                        self.settings.editor,
                        &self.installed,
                        &root,
                        file.as_deref(),
                    ) {
                        self.error_toast(
                            format!("Couldn't open {}", self.settings.editor.label()),
                            Some(error.to_string()),
                        );
                    }
                }
            }
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

/// The full path of `git` (a bare name is looked up on `PATH`), for finding what came with it.
fn which_git(git: &std::path::Path) -> PathBuf {
    if git.components().count() > 1 {
        return git.to_owned();
    }
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("git.exe"))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| git.to_owned())
}
