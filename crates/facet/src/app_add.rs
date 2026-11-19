//! Adding a repository: the Add Repository dialog's state and work. One dialog with three
//! tabs (Clone from a URL, Open a folder, New repository); clone runs like the other network
//! commands (live progress, cancellable, credentials through askpass / GCM).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use facet_git::{CancellationToken, GitError, ProgressFn};

use crate::actions::Action;
use crate::app::{describe_error, Event, FacetApp};
use crate::platform::{self, Chrome};

/// The branch a new repository starts on.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddTab {
    #[default]
    Clone,
    Open,
    New,
}

impl AddTab {
    pub const ALL: [Self; 3] = [Self::Clone, Self::Open, Self::New];

    pub fn label(self) -> &'static str {
        match self {
            Self::Clone => "Clone",
            Self::Open => "Open folder",
            Self::New => "New",
        }
    }
}

/// A clone or init in flight.
pub struct AddRunning {
    pub label: String,
    pub progress: Arc<Mutex<String>>,
    pub cancel: CancellationToken,
    pub started: Instant,
}

impl AddRunning {
    pub fn progress(&self) -> String {
        self.progress
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub struct AddRepoView {
    pub tab: AddTab,
    /// The first branch of a new repository (the default branch name in settings).
    pub initial_branch: String,
    /// Clone: the URL (or a local path) to clone.
    pub url: String,
    /// Clone: where the clone goes; follows the URL until the person edits it.
    pub clone_dest: String,
    pub clone_dest_edited: bool,
    /// New: the folder name and the folder it is created in.
    pub new_name: String,
    pub new_parent: String,
    /// Open: a folder typed or pasted by hand.
    pub open_path: String,
    pub running: Option<AddRunning>,
    pub error: Option<String>,
    /// Move keyboard focus to the tab's first field on the next frame.
    pub focus_first: bool,
}

impl AddRepoView {
    /// The clone destination the URL suggests, inside `parent`.
    pub fn suggested_dest(&self, parent: &Path) -> String {
        facet_git::clone_folder_name(&self.url)
            .map(|name| parent.join(name).display().to_string())
            .unwrap_or_default()
    }

    pub fn new_dest(&self) -> Option<PathBuf> {
        let name = self.new_name.trim();
        let parent = self.new_parent.trim().trim_matches('"');
        (!name.is_empty() && !parent.is_empty()).then(|| Path::new(parent).join(name))
    }
}

pub enum AddRepoEvent {
    /// Browse… in the dialog: a folder (or the picker's error), for the tab it was opened on.
    Picked(AddTab, Result<Option<PathBuf>, String>),
    Finished {
        dest: PathBuf,
        cloned: bool,
        result: Result<(), GitError>,
    },
}

impl FacetApp {
    /// Where new clones and repositories go unless the person picks a folder: next to the
    /// most recently used repository, else the user's profile folder.
    pub(crate) fn default_parent(&self) -> PathBuf {
        self.settings
            .recent_repos
            .first()
            .and_then(|path| path.parent())
            .map(Path::to_owned)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(r"C:\"))
    }

    fn show_add_repository(&mut self, tab: Option<AddTab>) {
        let parent = self.default_parent();
        let view = self.add_view.get_or_insert_with(|| AddRepoView {
            tab: self.last_add_tab,
            initial_branch: self.settings.default_branch.clone(),
            new_parent: parent.display().to_string(),
            ..AddRepoView::default()
        });
        if let Some(tab) = tab {
            view.tab = tab;
        }
        view.focus_first = true;
        self.settings_view = None;
        self.palette.open = false;
    }

    fn close_add_repository(&mut self) {
        if let Some(view) = self.add_view.take() {
            self.last_add_tab = view.tab;
            if let Some(running) = view.running {
                running.cancel.cancel();
            }
        }
    }

    fn browse_add_folder(&mut self) {
        let Some(tab) = self.add_view.as_ref().map(|view| view.tab) else {
            return;
        };
        let title = match tab {
            AddTab::Clone => "Choose Where to Clone",
            AddTab::Open => "Open Repository",
            AddTab::New => "Choose Where to Create the Repository",
        };
        let owner = self.chrome.map(Chrome::raw);
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("facet-add-picker".into())
            .spawn(move || {
                let result = platform::pick_folder(owner, title);
                if tx
                    .send(Event::AddRepo(AddRepoEvent::Picked(tab, result)))
                    .is_ok()
                {
                    ctx.request_repaint();
                }
            });
        if let Err(error) = spawned {
            self.error_toast("Couldn't show the folder picker", Some(error.to_string()));
        }
    }

    fn start_clone(&mut self) {
        let parent = self.default_parent();
        let Some(view) = self.add_view.as_mut() else {
            return;
        };
        if view.running.is_some() {
            return;
        }
        let url = view.url.trim().to_owned();
        let dest_text = if view.clone_dest_edited {
            view.clone_dest.trim().trim_matches('"').to_owned()
        } else {
            view.suggested_dest(&parent)
        };
        if url.is_empty() || dest_text.is_empty() {
            view.error = Some("Enter the repository's URL and where to save it.".into());
            return;
        }
        let dest = PathBuf::from(dest_text);
        let name = facet_git::clone_folder_name(&url).unwrap_or_else(|| url.clone());

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
        view.error = None;
        view.running = Some(AddRunning {
            label: format!("Cloning {name}"),
            progress: progress_text,
            cancel: cancel.clone(),
            started: Instant::now(),
        });
        let git = self.git.clone();
        self.spawn(async move {
            // A folder git refuses anyway; checked here so the message names it plainly.
            let existed = tokio::fs::metadata(&dest).await.is_ok();
            let occupied = existed
                && match tokio::fs::read_dir(&dest).await {
                    Ok(mut entries) => entries.next_entry().await.ok().flatten().is_some(),
                    Err(_) => true,
                };
            let result = if occupied {
                Err(GitError::Unsupported(format!(
                    "{} already exists and isn't empty. Choose another folder.",
                    dest.display()
                )))
            } else {
                facet_git::clone(&git, &url, &dest, &cancel, progress).await
            };
            // A cancelled clone leaves a half-written folder that only Facet created: remove
            // it so the same clone can simply be started again.
            if matches!(result, Err(GitError::Cancelled)) && !existed {
                let _ = tokio::fs::remove_dir_all(&dest).await;
            }
            Event::AddRepo(AddRepoEvent::Finished {
                dest,
                cloned: true,
                result,
            })
        });
    }

    fn create_repository(&mut self) {
        let Some(view) = self.add_view.as_mut() else {
            return;
        };
        if view.running.is_some() {
            return;
        }
        let Some(dest) = view.new_dest() else {
            view.error = Some("Enter a name and a location for the repository.".into());
            return;
        };
        view.error = None;
        view.running = Some(AddRunning {
            label: "Creating the repository".into(),
            progress: Arc::default(),
            cancel: CancellationToken::new(),
            started: Instant::now(),
        });
        let git = self.git.clone();
        let branch = self.settings.default_branch.clone();
        self.spawn(async move {
            let result = facet_git::init(&git, &dest, &branch).await;
            Event::AddRepo(AddRepoEvent::Finished {
                dest,
                cloned: false,
                result,
            })
        });
    }

    fn open_typed_path(&mut self) {
        let Some(view) = self.add_view.as_mut() else {
            return;
        };
        let text = view.open_path.trim().trim_matches('"');
        if text.is_empty() {
            view.error = Some("Enter a folder, or choose one with Browse….".into());
            return;
        }
        let path = PathBuf::from(text);
        self.close_add_repository();
        self.open_path(path, true);
    }

    pub(crate) fn handle_add_event(&mut self, event: AddRepoEvent) {
        match event {
            AddRepoEvent::Picked(_, Err(detail)) => {
                self.error_toast("Couldn't show the folder picker", Some(detail));
            }
            AddRepoEvent::Picked(_, Ok(None)) => {}
            AddRepoEvent::Picked(tab, Ok(Some(folder))) => {
                let Some(view) = self.add_view.as_mut() else {
                    return;
                };
                match tab {
                    AddTab::Clone => {
                        let dest = view.suggested_dest(&folder);
                        view.clone_dest = if dest.is_empty() {
                            folder.display().to_string()
                        } else {
                            dest
                        };
                        view.clone_dest_edited = true;
                    }
                    AddTab::New => view.new_parent = folder.display().to_string(),
                    AddTab::Open => {
                        self.close_add_repository();
                        self.open_path(folder, true);
                    }
                }
            }
            AddRepoEvent::Finished {
                dest,
                cloned,
                result,
            } => {
                let Some(view) = self.add_view.as_mut() else {
                    // Closed meanwhile: a clone that still finished is opened anyway.
                    if result.is_ok() {
                        self.open_path(dest, true);
                    }
                    return;
                };
                view.running = None;
                match result {
                    Ok(()) => {
                        let name = dest.file_name().map_or_else(
                            || dest.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        self.close_add_repository();
                        self.open_path(dest, true);
                        self.info_toast(if cloned {
                            format!("Cloned {name}")
                        } else {
                            format!("Created {name}")
                        });
                    }
                    Err(GitError::Cancelled) => view.error = None,
                    Err(GitError::Unsupported(text)) => view.error = Some(text),
                    Err(error) => {
                        let (title, detail) = describe_error(&error);
                        view.error = Some(match detail {
                            Some(detail) if !detail.trim().is_empty() => {
                                // git's last line is the one that says what went wrong.
                                let last = detail.trim().lines().last().unwrap_or_default();
                                last.trim_start_matches("fatal: ").to_owned()
                            }
                            _ => title,
                        });
                    }
                }
            }
        }
    }

    /// Actions of the Add Repository dialog; anything else is handed back.
    pub(crate) fn run_add_action(&mut self, action: Action) -> Option<Action> {
        match action {
            Action::ShowAddRepository(tab) => self.show_add_repository(tab),
            Action::CloseAddRepository => self.close_add_repository(),
            Action::BrowseAddFolder => self.browse_add_folder(),
            Action::StartClone => self.start_clone(),
            Action::CancelClone => {
                if let Some(running) = self.add_view.as_ref().and_then(|v| v.running.as_ref()) {
                    running.cancel.cancel();
                }
            }
            Action::CreateRepository => self.create_repository(),
            Action::OpenTypedPath => self.open_typed_path(),
            other => return Some(other),
        }
        None
    }
}
