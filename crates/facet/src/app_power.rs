//! Rewriting history, the operation in progress, search / file history / reflog lists,
//! blame, conflict resolution and external tools (M3). Like `app_repo.rs`, everything here
//! only starts background work and applies results; the UI thread never waits on git.

use std::path::{Path, PathBuf};
use std::time::Instant;

use facet_git::conflict::{self, Region, Side};
use facet_git::parse::{BlameCommit, BlameRange, LogCommit};
use facet_git::rebase::{Editors, TodoItem};
use facet_git::{
    tools, CancellationToken, DiffRequest, Git, GitError, Progress, RepoInfo, ResetMode, UndoPoint,
};

use crate::actions::Action;
use crate::app::{describe_error, Event, FacetApp};
use crate::app_repo::{Outcome, RepoEvent};
use crate::app_work::{ready_info, WorkEvent};
use crate::history::{flat_row, FilterKind, View};
use crate::loader::{self, Side as DiffSide};
use crate::power::{BlameView, ConflictEditor, RebaseEditor, UndoItem};
use crate::settings::ExternalTool;
use crate::ui::confirm::Confirm;
use crate::ui::prompt::{Prompt, PromptField, PromptKind};

/// Search results arrive in pages like the history.
const FIRST_PAGE: usize = 200;
const PAGE: usize = 5000;
/// Reflog entries shown at most.
const REFLOG_LIMIT: usize = 5000;

/// Results of M3 background work for one tab.
pub enum PowerEvent {
    /// A file history or the reflog, read whole.
    FilterLoaded {
        generation: u64,
        result: Result<FilterRows, GitError>,
        elapsed_ms: u32,
    },
    BlameText {
        generation: u64,
        result: Result<Vec<u8>, GitError>,
    },
    BlameRanges {
        generation: u64,
        commits: Vec<BlameCommit>,
        ranges: Vec<BlameRange>,
    },
    BlameDone {
        generation: u64,
        result: Result<(), GitError>,
    },
    RebasePlan(Result<Vec<LogCommit>, GitError>),
    /// A conflicted file split into common text and chunks (`None`: no markers), with each
    /// chunk's base filled in where it could be.
    ConflictText {
        path: String,
        result: Result<Option<Vec<Region>>, GitError>,
    },
    /// An external tool exited: its exit code, and for a merge whether the file is now free
    /// of conflict markers.
    ToolExited {
        what: String,
        merged: Option<String>,
        result: Result<(i32, bool), GitError>,
    },
}

pub struct FilterRows {
    pub commits: Vec<LogCommit>,
    pub labels: Vec<String>,
    pub paths: Vec<String>,
}

/// Facet itself is git's editor during a rebase (G-rebase): it writes the todo list and the
/// reword messages prepared in the rebase editor.
fn editors() -> Result<Editors, GitError> {
    let exe = std::env::current_exe().map_err(|error| {
        GitError::Unsupported(format!("Couldn't find Facet's own program file: {error}"))
    })?;
    Ok(Editors::facet(&exe))
}

fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `path` (forward slashes) under `root`.
fn worktree_path(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_owned(), |dir, part| dir.join(part))
}

/// Run `op` with local changes auto-stashed around it (PLAN.md decision 33): put back when it
/// finishes or fails, kept with a note when it stops half-way.
async fn autostashed<F, Fut>(
    git: &Git,
    info: &RepoInfo,
    verb: &str,
    op: F,
) -> Result<(Progress, UndoPoint, Option<String>), GitError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(Progress, UndoPoint), GitError>>,
{
    let root = info.root();
    let stash = facet_git::autostash(git, root, &format!("Facet: before {verb}")).await?;
    let (progress, undo) = match op().await {
        Ok(done) => done,
        Err(error) => {
            // Nothing happened: put the changes back.
            if let Some(stash) = &stash {
                let _ = facet_git::restore_autostash(git, root, stash).await;
            }
            return Err(error);
        }
    };
    let note = match (&progress, stash) {
        (_, None) => None,
        (Progress::Finished, Some(stash)) => {
            match facet_git::restore_autostash(git, root, &stash).await {
                Ok(()) => None,
                Err(error) => Some(describe_error(&error).1.unwrap_or_default()),
            }
        }
        (Progress::Stopped { .. }, Some(_)) => Some(
            "Your local changes were stashed first (\"Facet: before …\"). Apply that stash \
             once the operation is finished."
                .to_owned(),
        ),
    };
    Ok((progress, undo, note))
}

/// What a sequenced operation reports; `note` adds to the toast (an auto-stash left behind).
pub struct Rewrote {
    pub title: String,
    pub progress: Progress,
    pub undo: Option<UndoPoint>,
    pub note: Option<String>,
}

impl FacetApp {
    // ----- the operation in progress and the undo stack ------------------------------------

    pub(crate) fn on_rewrote(&mut self, index: usize, rewrote: Rewrote) {
        let Rewrote {
            title,
            progress,
            undo,
            note,
        } = rewrote;
        match progress {
            Progress::Finished => {
                self.tabs[index].power.operation = None;
                match undo {
                    Some(point) => {
                        let item = UndoItem {
                            title: title.clone(),
                            point,
                        };
                        self.tabs[index].power.push_undo(item.clone());
                        self.action_toast(title, "Undo", Action::UndoOperation(item));
                    }
                    None => self.info_toast(title),
                }
                if let Some(note) = note {
                    self.error_toast("Your local changes are still in the stash", Some(note));
                }
            }
            Progress::Stopped { operation, message } => {
                let label = operation.kind.label();
                self.tabs[index].power.operation = Some(operation);
                let detail = match note {
                    Some(note) => format!("{message}\n{note}"),
                    None => message.clone(),
                };
                if message.contains("CONFLICT") || message.contains("error:") {
                    self.error_toast(
                        format!("{label} stopped on conflicts: resolve them, then Continue"),
                        Some(detail).filter(|detail| !detail.is_empty()),
                    );
                } else {
                    self.error_toast(
                        format!("{label} paused: Continue when you are ready"),
                        Some(detail).filter(|detail| !detail.is_empty()),
                    );
                }
            }
        }
    }

    /// Run a history-rewriting operation for the active tab (serialized with the others).
    fn rewrite<F, Fut>(&mut self, what: impl Into<String>, op: F)
    where
        F: FnOnce(Git, RepoInfo) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Rewrote, GitError>> + Send,
    {
        self.repo_op_info(what, move |git, info| async move {
            op(git, info).await.map(Outcome::Rewrote)
        });
    }

    fn operation_in_progress(&mut self, index: usize) -> bool {
        match &self.tabs[index].power.operation {
            Some(operation) => {
                let label = operation.kind.label();
                self.info_toast(format!(
                    "A {} is in progress: continue or abort it first",
                    label.to_lowercase()
                ));
                true
            }
            None => false,
        }
    }

    fn cherry_pick_or_revert(&mut self, index: usize, oid: String, revert: bool) {
        if self.operation_in_progress(index) {
            return;
        }
        let merge = self.tabs[index]
            .history
            .commits
            .iter()
            .find(|commit| commit.oid == oid)
            .is_some_and(|commit| commit.parents.len() > 1);
        let (verb, done) = if revert {
            ("revert", "Reverted")
        } else {
            ("cherry-pick", "Cherry-picked")
        };
        let short = short(&oid);
        self.rewrite(format!("{verb} {short}"), move |git, info| async move {
            let commits = [(oid, merge)];
            let (progress, undo, note) = autostashed(&git, &info, verb, || async {
                if revert {
                    facet_git::revert(&git, &info, &commits).await
                } else {
                    facet_git::cherry_pick(&git, &info, &commits).await
                }
            })
            .await?;
            Ok(Rewrote {
                title: format!("{done} {short}"),
                progress,
                undo: Some(undo),
                note,
            })
        });
    }

    /// Merge a branch or commit into the current branch (local changes auto-stashed around it).
    fn merge_into_current(&mut self, rev: String) {
        let Some(index) = self.active else {
            return;
        };
        if self.operation_in_progress(index) {
            return;
        }
        let shown = if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
            short(&rev)
        } else {
            rev.clone()
        };
        self.rewrite(format!("merge {shown}"), move |git, info| async move {
            let (progress, undo, note) =
                autostashed(&git, &info, "merge", || facet_git::merge(&git, &info, &rev)).await?;
            Ok(Rewrote {
                title: format!("Merged {shown}"),
                progress,
                undo: Some(undo),
                note,
            })
        });
    }

    fn reset_to(&mut self, oid: String, mode: ResetMode) {
        let Some(index) = self.active else {
            return;
        };
        if self.operation_in_progress(index) {
            return;
        }
        let branch = self
            .repo_context()
            .and_then(|cx| cx.branch)
            .unwrap_or_else(|| "HEAD".to_owned());
        let how = match mode {
            ResetMode::Soft => "soft",
            ResetMode::Mixed => "mixed",
            ResetMode::Hard => "hard",
            ResetMode::Keep => "keep",
        };
        let short = short(&oid);
        self.rewrite(format!("reset {branch}"), move |git, info| async move {
            let undo = facet_git::reset(&git, info.root(), &oid, mode).await?;
            Ok(Rewrote {
                title: format!("Reset {branch} to {short} ({how})"),
                progress: Progress::Finished,
                undo: Some(undo),
                note: None,
            })
        });
    }

    fn rebase_onto(&mut self, oid: String) {
        let Some(index) = self.active else {
            return;
        };
        if self.operation_in_progress(index) {
            return;
        }
        let editors = match editors() {
            Ok(editors) => editors,
            Err(error) => {
                let (title, detail) = describe_error(&error);
                self.error_toast(title, detail);
                return;
            }
        };
        let short = short(&oid);
        self.rewrite(
            format!("rebase onto {short}"),
            move |git, info| async move {
                let head = head_oid(&git, info.root()).await?;
                let progress = facet_git::rebase::rebase_onto(&git, &info, &oid, &editors).await?;
                Ok(Rewrote {
                    title: format!("Rebased onto {short}"),
                    progress,
                    undo: Some(UndoPoint {
                        head,
                        mode: ResetMode::Keep,
                        stash: None,
                    }),
                    note: None,
                })
            },
        );
    }

    // ----- interactive rebase -----------------------------------------------------------------

    /// Open the editor for the commits from `from` (inclusive) up to HEAD.
    fn open_rebase_editor(&mut self, index: usize, from: Option<String>) {
        if self.operation_in_progress(index) {
            return;
        }
        let Some(root) = ready_info(&self.tabs[index]).map(|info| info.root().to_owned()) else {
            return;
        };
        let commit = from.as_ref().and_then(|oid| {
            self.tabs[index]
                .history
                .commits
                .iter()
                .find(|commit| commit.oid == *oid)
                .cloned()
        });
        let (base, label) = match (&from, &commit) {
            (Some(_), Some(commit)) => (
                commit.parents.first().map(ToString::to_string),
                format!("{} {}", commit.short_oid(), commit.subject),
            ),
            (Some(oid), None) => (Some(format!("{oid}^")), short(oid)),
            (None, _) => (None, "the first commit".to_owned()),
        };
        self.tabs[index].power.rebase = Some(RebaseEditor::new(base.clone(), label));
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let result = facet_git::rebase::rebase_plan(&git, &root, base.as_deref()).await;
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::RebasePlan(result)))
        });
    }

    fn start_rebase(&mut self, index: usize) {
        let Some(editor) = self.tabs[index].power.rebase.as_mut() else {
            return;
        };
        if editor.loading {
            return;
        }
        if !editor.changes_something() {
            editor.error = Some("Nothing to change yet: every commit is picked, in order.".into());
            return;
        }
        // Check the plan before anything runs; the editor stays open to fix it.
        if let Err(error) = facet_git::rebase::todo_text(&editor.items) {
            editor.error = Some(match error {
                GitError::Unsupported(message) => message,
                other => other.to_string(),
            });
            return;
        }
        let Some(editor) = self.tabs[index].power.rebase.take() else {
            return;
        };
        let items: Vec<TodoItem> = editor.items;
        let editors = match editors() {
            Ok(editors) => editors,
            Err(error) => {
                let (title, detail) = describe_error(&error);
                self.error_toast(title, detail);
                return;
            }
        };
        let base = editor.base;
        let count = items.len();
        self.rewrite("rebase", move |git, info| async move {
            let head = head_oid(&git, info.root()).await?;
            let progress = facet_git::rebase::interactive_rebase(
                &git,
                &info,
                base.as_deref(),
                &items,
                &editors,
            )
            .await?;
            Ok(Rewrote {
                title: format!("Rewrote {count} commits"),
                progress,
                undo: Some(UndoPoint {
                    head,
                    mode: ResetMode::Keep,
                    stash: None,
                }),
                note: None,
            })
        });
    }

    fn sequence(&mut self, index: usize, step: Step) {
        let Some(operation) = self.tabs[index].power.operation.clone() else {
            self.info_toast("No rebase, merge or cherry-pick is in progress");
            return;
        };
        let label = operation.kind.label().to_owned();
        let editors = match editors() {
            Ok(editors) => editors,
            Err(error) => {
                let (title, detail) = describe_error(&error);
                self.error_toast(title, detail);
                return;
            }
        };
        let what = match step {
            Step::Continue => format!("continue the {}", label.to_lowercase()),
            Step::Skip => format!("skip this {} step", label.to_lowercase()),
            Step::Abort => format!("abort the {}", label.to_lowercase()),
        };
        self.rewrite(what, move |git, info| async move {
            let progress = match step {
                Step::Continue => {
                    facet_git::continue_operation(&git, &info, &operation, &editors).await?
                }
                Step::Skip => facet_git::skip_operation(&git, &info, &operation, &editors).await?,
                Step::Abort => {
                    facet_git::abort_operation(&git, &info, &operation).await?;
                    Progress::Finished
                }
            };
            let title = match step {
                Step::Abort => format!("{label} aborted"),
                _ => format!("{label} finished"),
            };
            Ok(Rewrote {
                title,
                progress,
                undo: None,
                note: None,
            })
        });
    }

    fn undo_operation(&mut self, index: usize, item: UndoItem) {
        self.tabs[index].power.forget_undo(&item.point);
        let title = item.title;
        let point = item.point;
        self.repo_op("undo", move |git, root| async move {
            facet_git::undo_to(&git, &root, &point).await?;
            Ok(Outcome::Done(Some(format!("Undid: {title}"))))
        });
    }

    // ----- search, file history, reflog -------------------------------------------------------

    fn run_search(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        let text = tab.power.search.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let kind = tab.power.search.kind;
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let cancel = CancellationToken::new();
        let generation = tab.history.enter_filter(
            FilterKind::Search {
                kind,
                text: text.clone(),
            },
            cancel.clone(),
        );
        tab.history.first_page_ms = None;
        let id = tab.id;
        let git = self.git.clone();
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        let scope = self.history_scope();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let started = Instant::now();
            let result = facet_git::stream_search_in(
                &git,
                &root,
                scope,
                kind,
                &text,
                &cancel,
                FIRST_PAGE,
                PAGE,
                move |commits: Vec<LogCommit>| {
                    let rows = commits.iter().map(flat_row).collect();
                    let event = Event::Work {
                        tab: id,
                        stamp: None,
                        event: WorkEvent::Repo(RepoEvent::LogPage {
                            generation,
                            commits,
                            rows,
                            elapsed_ms: elapsed_ms(started),
                        }),
                    };
                    if tx.send(event).is_ok() {
                        ctx.request_repaint();
                    }
                },
            )
            .await;
            WorkEvent::Repo(RepoEvent::LogDone {
                generation,
                result,
                elapsed_ms: elapsed_ms(started),
            })
        });
    }

    fn show_filter(&mut self, index: usize, kind: FilterKind) {
        self.show_view(View::History);
        let tab = &mut self.tabs[index];
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        tab.power.search.open = false;
        let cancel = CancellationToken::new();
        let generation = tab.history.enter_filter(kind.clone(), cancel.clone());
        tab.history.first_page_ms = None;
        let git = self.git.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let started = Instant::now();
            let result = match kind {
                FilterKind::File { path } => facet_git::file_history(&git, &root, &path, &cancel)
                    .await
                    .map(|entries| {
                        let mut rows = FilterRows {
                            commits: Vec::with_capacity(entries.len()),
                            labels: Vec::with_capacity(entries.len()),
                            paths: Vec::with_capacity(entries.len()),
                        };
                        for entry in entries {
                            rows.labels.push(if entry.path == path {
                                String::new()
                            } else {
                                entry.path.clone()
                            });
                            rows.paths.push(entry.path);
                            rows.commits.push(entry.commit);
                        }
                        rows
                    }),
                FilterKind::Reflog => facet_git::reflog(&git, &root, "HEAD", REFLOG_LIMIT)
                    .await
                    .map(|entries| {
                        let mut rows = FilterRows {
                            commits: Vec::with_capacity(entries.len()),
                            labels: Vec::with_capacity(entries.len()),
                            paths: Vec::new(),
                        };
                        for entry in entries {
                            rows.labels.push(format!(
                                "{} · {}",
                                entry.selector("HEAD"),
                                entry.action
                            ));
                            rows.commits.push(entry.commit);
                        }
                        rows
                    }),
                FilterKind::Search { .. } => Err(GitError::Unsupported(
                    "Searches stream their results".to_owned(),
                )),
            };
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::FilterLoaded {
                generation,
                result,
                elapsed_ms: elapsed_ms(started),
            }))
        });
    }

    pub(crate) fn clear_filter(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        tab.power.search.open = false;
        if tab.history.filter.is_none() {
            return;
        }
        let before = tab.history.commit.oid.clone();
        let reload = tab.history.exit_filter();
        let stale = tab.refs.history_tips != tab.refs.tips;
        if reload || stale {
            self.load_history(index);
        }
        if self.tabs[index]
            .history
            .selected_oid()
            .map(|oid| oid.to_string())
            != before
        {
            self.load_detail(index);
        }
    }

    // ----- the file shown in the diff -----------------------------------------------------------

    /// The file shown in the diff pane of the active view: its path and the commit it is
    /// shown at (`None`: the working copy).
    pub(crate) fn current_file(&self) -> Option<(String, Option<String>)> {
        let tab = self.active_tab()?;
        match tab.view {
            View::Changes => {
                let key = tab.work.diff.key.as_ref()?;
                (key.request.source != facet_git::DiffSource::Untracked)
                    .then(|| (key.request.path.clone(), None))
            }
            View::History => {
                let key = tab.history.commit.diff.key.as_ref()?;
                match &key.request.source {
                    facet_git::DiffSource::Commit { new, .. } => {
                        Some((key.request.path.clone(), Some(new.clone())))
                    }
                    _ => None,
                }
            }
            View::Stash => {
                let key = tab.stash.diff.key.as_ref()?;
                match &key.request.source {
                    facet_git::DiffSource::Commit { new, .. } => {
                        Some((key.request.path.clone(), Some(new.clone())))
                    }
                    _ => None,
                }
            }
        }
    }

    /// The conflicted file shown in the diff, if any.
    pub(crate) fn conflicted_file(&self) -> Option<String> {
        let tab = self.active_tab()?;
        if tab.view != View::Changes {
            return None;
        }
        let key = tab.work.diff.key.as_ref()?;
        (key.request.source == facet_git::DiffSource::Conflict).then(|| key.request.path.clone())
    }

    /// The request and blobs behind the shown diff (for an external diff tool).
    fn current_diff(&self) -> Option<(DiffRequest, Option<String>, Option<String>)> {
        let tab = self.active_tab()?;
        match tab.view {
            View::Changes => {
                let key = tab.work.diff.key.as_ref()?;
                let (_, entry) = tab.work.current()?;
                Some((
                    key.request.clone(),
                    entry.head_oid.clone(),
                    entry.index_oid.clone(),
                ))
            }
            View::History => {
                let view = &tab.history.commit;
                let key = view.diff.key.as_ref()?;
                let file = view.files().get(view.selected_file?)?;
                Some((
                    key.request.clone(),
                    file.old_oid.clone(),
                    file.new_oid.clone(),
                ))
            }
            View::Stash => {
                let view = &tab.stash;
                let key = view.diff.key.as_ref()?;
                let file = &view.files().get(view.selected_file?)?.file;
                Some((
                    key.request.clone(),
                    file.old_oid.clone(),
                    file.new_oid.clone(),
                ))
            }
        }
    }

    // ----- blame --------------------------------------------------------------------------------

    fn open_blame(&mut self, index: usize, path: String, rev: Option<String>) {
        let tab = &mut self.tabs[index];
        let Some(root) = ready_info(tab).map(|info| info.root().to_owned()) else {
            return;
        };
        let generation = match tab.power.blame.take() {
            Some(previous) => {
                if let Some(cancel) = previous.cancel {
                    cancel.cancel();
                }
                previous.generation + 1
            }
            None => 1,
        };
        let cancel = CancellationToken::new();
        let mut view = BlameView::new(path.clone(), rev.clone(), generation);
        view.cancel = Some(cancel.clone());
        tab.power.blame = Some(view);

        // The text and the blame are independent: read both at once (G5).
        let git = self.git.clone();
        let (text_path, text_rev, text_root) = (path.clone(), rev.clone(), root.clone());
        let text_cancel = cancel.clone();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let result = match text_rev {
                Some(rev) => {
                    facet_git::read_blob(
                        &git,
                        &text_root,
                        &format!("{rev}:{text_path}"),
                        Some(&text_cancel),
                    )
                    .await
                }
                None => tokio::fs::read(worktree_path(&text_root, &text_path))
                    .await
                    .map_err(GitError::from),
            };
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::BlameText {
                generation,
                result,
            }))
        });

        let git = self.git.clone();
        let id = tab.id;
        let tx = self.events_tx.clone();
        let ctx = self.ctx.clone();
        self.spawn_work(tab, false, async move {
            let result = facet_git::blame(
                &git,
                &root,
                &path,
                rev.as_deref(),
                &cancel,
                move |commits, ranges| {
                    let event = Event::Work {
                        tab: id,
                        stamp: None,
                        event: WorkEvent::Repo(RepoEvent::Power(PowerEvent::BlameRanges {
                            generation,
                            commits,
                            ranges,
                        })),
                    };
                    if tx.send(event).is_ok() {
                        ctx.request_repaint();
                    }
                },
            )
            .await;
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::BlameDone {
                generation,
                result,
            }))
        });
    }

    fn close_blame(&mut self, index: usize) {
        if let Some(view) = self.tabs[index].power.blame.take() {
            if let Some(cancel) = view.cancel {
                cancel.cancel();
            }
        }
    }

    // ----- conflicts ------------------------------------------------------------------------------

    fn open_conflict_editor(&mut self, index: usize, path: String) {
        let tab = &mut self.tabs[index];
        let Some(info) = ready_info(tab).cloned() else {
            return;
        };
        tab.power.conflict = Some(ConflictEditor::new(path.clone()));
        let tab = &self.tabs[index];
        let git = self.git.clone();
        self.spawn_work(tab, false, async move {
            let root = info.root().to_owned();
            let result: Result<_, GitError> = async {
                let bytes = tokio::fs::read(worktree_path(&root, &path)).await?;
                let Some(mut regions) = std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(conflict::parse_markers)
                else {
                    return Ok(None);
                };
                // Without a base the chunks still resolve; it is only shown when found.
                let scratch = facet_git::tools::scratch_dir(&info.git_dir);
                let _ = conflict::fill_bases(&git, &root, &scratch, &path, &mut regions).await;
                Ok(Some(regions))
            }
            .await;
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::ConflictText { path, result }))
        });
    }

    fn save_resolution(&mut self, index: usize) {
        let Some(editor) = &self.tabs[index].power.conflict else {
            return;
        };
        let Some(regions) = &editor.regions else {
            return;
        };
        if editor.resolved() < editor.chunks() {
            self.info_toast("Choose how to resolve every conflict first");
            return;
        }
        let contents = conflict::apply_choices(regions, &editor.choices);
        let path = editor.path.clone();
        self.tabs[index].power.conflict = None;
        let name = file_name(&path).to_owned();
        self.repo_op(format!("resolve {name}"), move |git, root| async move {
            conflict::resolve_with(&git, &root, &path, contents.as_bytes()).await?;
            Ok(Outcome::Done(Some(format!("Resolved {name}"))))
        });
    }

    fn take_side(&mut self, index: usize, path: String, side: Side) {
        let code = self.tabs[index]
            .work
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.conflict);
        let Some(code) = code else {
            self.info_toast("That file is not conflicted");
            return;
        };
        // `DU`: deleted by us; `UD`: deleted by them.
        let exists = match side {
            Side::Ours => code[0] != b'D',
            Side::Theirs => code[1] != b'D',
        };
        let which = match side {
            Side::Ours => "ours",
            Side::Theirs => "theirs",
        };
        let name = file_name(&path).to_owned();
        self.repo_op(
            format!("take {which} for {name}"),
            move |git, root| async move {
                conflict::take_side(&git, &root, &path, side, exists).await?;
                Ok(Outcome::Done(Some(format!("Resolved {name} with {which}"))))
            },
        );
    }

    // ----- external tools -----------------------------------------------------------------------

    fn open_merge_tool(&mut self, index: usize, path: String) {
        let Some(line) = self.settings.external_tool(&path, true).map(str::to_owned) else {
            self.info_toast(format!(
                "No external merge tool is set up for {}: add one from the command palette",
                file_name(&path)
            ));
            return;
        };
        let Some(info) = ready_info(&self.tabs[index]).cloned() else {
            return;
        };
        let git = self.git.clone();
        let name = file_name(&path).to_owned();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let root = info.root().to_owned();
            let dir = tools::scratch_dir(&info.git_dir);
            let result: Result<_, GitError> = async {
                let versions = conflict::conflict_versions(&git, &root, &path).await?;
                let vars = tools::merge_files(&dir, &root, &path, &versions).await?;
                let vars: Vec<(&str, &Path)> = vars
                    .iter()
                    .map(|(name, file)| (*name, file.as_path()))
                    .collect();
                let code =
                    tools::run_tool(&git, &line, &vars, &root, &CancellationToken::new()).await?;
                let merged = tokio::fs::read(worktree_path(&root, &path)).await?;
                let clean = !conflict::has_markers(&String::from_utf8_lossy(&merged));
                Ok((code, clean))
            }
            .await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::ToolExited {
                what: name,
                merged: Some(path),
                result,
            }))
        });
    }

    fn open_diff_tool(&mut self, index: usize) {
        let Some((request, old_oid, new_oid)) = self.current_diff() else {
            self.info_toast("Select a file to compare first");
            return;
        };
        let Some(line) = self
            .settings
            .external_tool(&request.path, false)
            .map(str::to_owned)
        else {
            self.info_toast(format!(
                "No external diff tool is set up for {}: add one from the command palette",
                file_name(&request.path)
            ));
            return;
        };
        let Some(info) = ready_info(&self.tabs[index]).cloned() else {
            return;
        };
        let git = self.git.clone();
        let name = file_name(&request.path).to_owned();
        let tab = &self.tabs[index];
        self.spawn_work(tab, false, async move {
            let root = info.root().to_owned();
            let dir = tools::scratch_dir(&info.git_dir);
            let result: Result<_, GitError> = async {
                let (old, new) =
                    loader::sides(&request, &root, old_oid.as_deref(), new_oid.as_deref());
                let read = |side: DiffSide| {
                    let git = git.clone();
                    let root = root.clone();
                    async move {
                        match side {
                            DiffSide::Missing => Ok(Ok(Vec::new())),
                            DiffSide::Blob(oid) => {
                                facet_git::read_blob(&git, &root, &oid, None).await.map(Ok)
                            }
                            DiffSide::Worktree(path) => Ok(Err(path)),
                        }
                    }
                };
                // Both sides at once (G5).
                let (old, new) = tokio::join!(read(old), read(new));
                let old = match old? {
                    Ok(bytes) => bytes,
                    Err(path) => tokio::fs::read(path).await?,
                };
                let new = new?;
                let vars = tools::diff_files(
                    &dir,
                    &request.path,
                    Some(&old),
                    new.as_deref().map_err(Clone::clone),
                )
                .await?;
                let vars: Vec<(&str, &Path)> = vars
                    .iter()
                    .map(|(name, file)| (*name, file.as_path()))
                    .collect();
                let code =
                    tools::run_tool(&git, &line, &vars, &root, &CancellationToken::new()).await?;
                Ok((code, true))
            }
            .await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            WorkEvent::Repo(RepoEvent::Power(PowerEvent::ToolExited {
                what: name,
                merged: None,
                result,
            }))
        });
    }

    /// Ask for a new external tool; `preset` fills the dialog in (with a note on where the
    /// program was found).
    pub(crate) fn add_external_tool(&mut self, preset: Option<(ExternalTool, String)>) {
        let placeholders = "Placeholders: $LOCAL and $REMOTE for a diff; $BASE, $LOCAL, $REMOTE \
                            and $MERGED for a merge. Quote paths with spaces.";
        let (tool, body) = match preset {
            Some((tool, note)) => (tool, format!("{note} {placeholders}")),
            None => (ExternalTool::default(), placeholders.to_owned()),
        };
        self.prompt = Some(
            Prompt::new(PromptKind::ExternalTool, "Add External Tool", "Add Tool")
                .body(body)
                .field(
                    PromptField::new("File extensions", "unity, prefab   (* for all files)", true)
                        .with_value(&tool.extensions.join(", ")),
                )
                .field(
                    PromptField::new(
                        "Diff command",
                        "\"C:\\Program Files\\Beyond Compare 4\\BComp.exe\" $LOCAL $REMOTE",
                        false,
                    )
                    .with_value(&tool.diff),
                )
                .field(
                    PromptField::new(
                        "Merge command",
                        "UnityYAMLMerge.exe merge -p $BASE $REMOTE $LOCAL $MERGED",
                        false,
                    )
                    .with_value(&tool.merge),
                ),
        );
    }

    fn create_external_tool(&mut self, tool: ExternalTool) {
        if tool.extensions.is_empty() {
            self.info_toast("Name at least one file extension (or * for every file)");
            return;
        }
        if tool.diff.trim().is_empty() && tool.merge.trim().is_empty() {
            self.info_toast("Give a diff command, a merge command, or both");
            return;
        }
        let mut missing = Vec::new();
        if !tool.diff.trim().is_empty() {
            missing.extend(tools::missing_placeholders(
                &tool.diff,
                &["LOCAL", "REMOTE"],
            ));
        }
        if !tool.merge.trim().is_empty() {
            missing.extend(tools::missing_placeholders(&tool.merge, &["MERGED"]));
        }
        if !missing.is_empty() {
            missing.dedup();
            self.error_toast(
                "The tool's command is missing placeholders",
                Some(format!("Add {} to the command line.", missing.join(", "))),
            );
            return;
        }
        let label = tool.label();
        self.settings.external_tools.push(tool);
        self.info_toast(format!("Added {label}"));
    }

    // ----- events ---------------------------------------------------------------------------------

    pub(crate) fn handle_power_event(&mut self, index: usize, event: PowerEvent) {
        match event {
            PowerEvent::FilterLoaded {
                generation,
                result,
                elapsed_ms,
            } => {
                let history = &mut self.tabs[index].history;
                let ok = match result {
                    Ok(rows) => {
                        let count = rows.commits.len();
                        history.filter_rows(generation, rows.commits, rows.labels, rows.paths);
                        Ok(count)
                    }
                    Err(error) => Err(error),
                };
                self.on_log_done(index, generation, ok, elapsed_ms);
            }
            PowerEvent::BlameText { generation, result } => {
                let Some(view) = self.tabs[index]
                    .power
                    .blame
                    .as_mut()
                    .filter(|view| view.generation == generation)
                else {
                    return;
                };
                match result {
                    Ok(text) => view.set_text(&text),
                    Err(GitError::Cancelled) => {}
                    Err(error) => {
                        view.loading = false;
                        view.error = describe_error(&error).1;
                    }
                }
            }
            PowerEvent::BlameRanges {
                generation,
                commits,
                ranges,
            } => {
                if let Some(view) = self.tabs[index]
                    .power
                    .blame
                    .as_mut()
                    .filter(|view| view.generation == generation)
                {
                    view.add(commits, &ranges);
                }
            }
            PowerEvent::BlameDone { generation, result } => {
                if let Some(view) = self.tabs[index]
                    .power
                    .blame
                    .as_mut()
                    .filter(|view| view.generation == generation)
                {
                    view.loading = false;
                    view.cancel = None;
                    match result {
                        Ok(()) | Err(GitError::Cancelled) => {}
                        Err(error) => view.error = describe_error(&error).1,
                    }
                }
            }
            PowerEvent::RebasePlan(result) => {
                let Some(editor) = self.tabs[index].power.rebase.as_mut() else {
                    return;
                };
                match result {
                    Ok(commits) => editor.set_plan(commits),
                    Err(error) => {
                        self.tabs[index].power.rebase = None;
                        let detail = match &error {
                            GitError::Unsupported(message) => Some(message.clone()),
                            other => describe_error(other).1,
                        };
                        self.error_toast("Can't rebase from there", detail);
                    }
                }
            }
            PowerEvent::ConflictText { path, result } => {
                let Some(editor) = self.tabs[index]
                    .power
                    .conflict
                    .as_mut()
                    .filter(|editor| editor.path == path)
                else {
                    return;
                };
                editor.loading = false;
                match result {
                    Ok(regions) => match regions {
                        Some(regions) => editor.set_regions(regions),
                        None => {
                            editor.error = Some(
                                "This file has no conflict markers to resolve here (it may be \
                                 binary, or already edited). Take one side whole, use a merge \
                                 tool, or mark it resolved as it is."
                                    .to_owned(),
                            );
                        }
                    },
                    Err(error) => editor.error = describe_error(&error).1,
                }
            }
            PowerEvent::ToolExited {
                what,
                merged,
                result,
            } => match (result, merged) {
                (Ok((0, true)), Some(path)) => {
                    self.pending.push(Action::StagePaths(vec![path]));
                    self.info_toast(format!("Merged {what}"));
                }
                (Ok(_), Some(_)) => {
                    self.info_toast(format!("The merge tool closed; {what} still has conflicts"))
                }
                (Ok(_), None) => {}
                (Err(GitError::Cancelled), _) => {}
                (Err(error), _) => {
                    let detail = match &error {
                        GitError::Unsupported(message) => Some(message.clone()),
                        other => describe_error(other).1,
                    };
                    self.error_toast(format!("Couldn't open {what} in the external tool"), detail);
                }
            },
        }
    }

    // ----- actions --------------------------------------------------------------------------------

    /// Handle M3 actions for the active tab. Returns the action back if it is not one of them.
    pub(crate) fn run_power_action(&mut self, action: Action) -> Option<Action> {
        match action {
            Action::AddExternalTool => {
                self.add_external_tool(None);
                return None;
            }
            Action::CreateExternalTool(tool) => {
                self.create_external_tool(tool);
                return None;
            }
            Action::RemoveExternalTool(position) => {
                if position < self.settings.external_tools.len() {
                    let tool = self.settings.external_tools.remove(position);
                    self.info_toast(format!("Removed {}", tool.label()));
                }
                return None;
            }
            _ => {}
        }
        let index = self.active.filter(|_| self.active_work().is_some())?;
        match action {
            Action::CherryPick(oid) => self.cherry_pick_or_revert(index, oid, false),
            Action::MergeIntoCurrent(rev) => self.merge_into_current(rev),
            Action::Revert(oid) => self.cherry_pick_or_revert(index, oid, true),
            Action::ResetTo { oid, mode } => {
                if mode == ResetMode::Hard {
                    let branch = self
                        .repo_context()
                        .and_then(|cx| cx.branch)
                        .unwrap_or_else(|| "HEAD".to_owned());
                    self.confirm = Some(Confirm {
                        title: format!("Hard reset {branch} to {}?", short(&oid)),
                        body: "Commits after it leave the branch, and uncommitted changes to \
                               tracked files are put in the stash first (Facet re-applies them \
                               if you undo). You can undo this."
                            .to_owned(),
                        confirm_label: "Reset".to_owned(),
                        action: Action::ConfirmedResetTo { oid, mode },
                    });
                } else {
                    self.reset_to(oid, mode);
                }
            }
            Action::ConfirmedResetTo { oid, mode } => self.reset_to(oid, mode),
            Action::RebaseOnto(oid) => {
                let branch = self
                    .repo_context()
                    .and_then(|cx| cx.branch)
                    .unwrap_or_else(|| "HEAD".to_owned());
                self.confirm = Some(Confirm {
                    title: format!("Rebase {branch} onto {}?", short(&oid)),
                    body: format!(
                        "{branch}'s own commits are replayed on top of that commit, one by one. \
                         Local changes are stashed for the duration. If a commit conflicts, \
                         the rebase pauses so you can resolve it. You can undo this."
                    ),
                    confirm_label: "Rebase".to_owned(),
                    action: Action::ConfirmedRebaseOnto(oid),
                });
            }
            Action::ConfirmedRebaseOnto(oid) => self.rebase_onto(oid),
            Action::InteractiveRebase { base } => self.open_rebase_editor(index, base),
            Action::SetRebaseStep { row, action } => {
                if let Some(editor) = &mut self.tabs[index].power.rebase {
                    editor.set_action(row, action);
                    editor.error = None;
                }
            }
            Action::RebaseMoveRow { row, step } => {
                if let Some(editor) = &mut self.tabs[index].power.rebase {
                    editor.move_row(row, step);
                    editor.error = None;
                }
            }
            Action::StartRebase => self.start_rebase(index),
            Action::CancelRebase => self.tabs[index].power.rebase = None,
            Action::ContinueOperation => self.sequence(index, Step::Continue),
            Action::SkipOperation => self.sequence(index, Step::Skip),
            Action::AbortOperation => {
                let Some(operation) = &self.tabs[index].power.operation else {
                    return None;
                };
                let label = operation.kind.label();
                self.confirm = Some(Confirm {
                    title: format!("Abort the {}?", label.to_lowercase()),
                    body: "The branch goes back to where it was before it started, and \
                           conflict resolutions made so far are lost."
                        .to_owned(),
                    confirm_label: "Abort".to_owned(),
                    action: Action::ConfirmedAbortOperation,
                });
            }
            Action::ConfirmedAbortOperation => self.sequence(index, Step::Abort),
            Action::UndoOperation(item) => self.undo_operation(index, item),
            Action::UndoLastOperation => match self.tabs[index].power.undo.last().cloned() {
                Some(item) => self.undo_operation(index, item),
                None => self.info_toast("Nothing to undo"),
            },

            Action::FindCommits => {
                self.show_view(View::History);
                let search = &mut self.tabs[index].power.search;
                search.open = true;
                search.focus = true;
            }
            Action::SetSearchKind(kind) => {
                let tab = &mut self.tabs[index];
                tab.power.search.kind = kind;
                tab.power.search.focus = true;
                let searching = matches!(
                    tab.history.filter.as_ref().map(|filter| &filter.kind),
                    Some(FilterKind::Search { .. })
                );
                if searching {
                    self.run_search(index);
                }
            }
            Action::RunSearch => self.run_search(index),
            Action::ClearFilter => self.clear_filter(index),
            Action::SetHistoryAllBranches(all) => {
                if self.settings.history_all_branches != all {
                    self.settings.history_all_branches = all;
                    let searching = matches!(
                        self.tabs[index].history.filter.as_ref().map(|filter| &filter.kind),
                        Some(FilterKind::Search { .. })
                    );
                    if searching {
                        self.run_search(index);
                    } else if self.tabs[index].history.filter.is_none() {
                        self.load_history(index);
                    }
                }
            }
            Action::ShowFileHistory(path) => {
                self.show_filter(index, FilterKind::File { path });
            }
            Action::FileHistoryCurrentFile => match self.current_file() {
                Some((path, _)) => self.show_filter(index, FilterKind::File { path }),
                None => self.info_toast("Select a file first"),
            },
            Action::ShowReflog => self.show_filter(index, FilterKind::Reflog),
            Action::Blame { path, rev } => self.open_blame(index, path, rev),
            Action::BlameCurrentFile => match self.current_file() {
                Some((path, rev)) => self.open_blame(index, path, rev),
                None => self.info_toast("Select a file first"),
            },
            Action::CloseBlame => self.close_blame(index),

            Action::TakeSide { path, side } => self.take_side(index, path, side),
            Action::ResolveConflicts(path) => self.open_conflict_editor(index, path),
            Action::ChooseChunk { chunk, choice } => {
                if let Some(editor) = &mut self.tabs[index].power.conflict {
                    editor.choose(chunk, choice);
                }
            }
            Action::ChooseAllChunks(choice) => {
                if let Some(editor) = &mut self.tabs[index].power.conflict {
                    editor.choose_all(choice);
                }
            }
            Action::SaveResolution => self.save_resolution(index),
            Action::CloseConflictEditor => self.tabs[index].power.conflict = None,
            Action::OpenMergeTool(path) => self.open_merge_tool(index, path),
            Action::OpenDiffTool => self.open_diff_tool(index),
            other => return Some(other),
        }
        None
    }

    /// The tab's state says a modal M3 editor is open (it owns the keyboard).
    pub(crate) fn power_modal_open(&self) -> bool {
        self.active_tab()
            .is_some_and(|tab| tab.power.rebase.is_some() || tab.power.conflict.is_some())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Continue,
    Skip,
    Abort,
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// The commit HEAD points at, for an undo point.
async fn head_oid(git: &Git, root: &Path) -> Result<String, GitError> {
    match facet_git::head(git, root).await? {
        facet_git::Head::Branch {
            commit: Some(commit),
            ..
        }
        | facet_git::Head::Detached { commit } => Ok(commit.oid),
        facet_git::Head::Branch { commit: None, .. } => Err(GitError::Unsupported(
            "This branch has no commits yet.".to_owned(),
        )),
    }
}
