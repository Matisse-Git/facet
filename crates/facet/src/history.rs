//! The history of one repository tab: the streamed commit list with its graph rows, the ref
//! badges drawn on it, the selected commit and its detail (message, changed files, the diff of
//! one file). Pure data and logic; `app_repo.rs` drives it and `ui/history.rs` draws it.

use std::collections::HashMap;

use facet_git::graph::GraphRow;
use facet_git::parse::{ChangedFile, CommitDetail, GitRef, LogCommit, RefKind, StashEntry};
use facet_git::{CancellationToken, Oid, RefsSnapshot, SearchKind, StashFile};

use crate::workspace::DiffView;

/// Which main view a repository tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Changes,
    History,
    /// The files of the stash clicked in the sidebar (`RepoTab::stash`).
    Stash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// The checked-out branch.
    Head,
    /// `HEAD` itself, when it is detached.
    Detached,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub label: String,
    pub kind: BadgeKind,
}

/// Ref labels by the commit they point at, in display order (HEAD first, then local
/// branches, remote branches, tags).
pub fn badges(refs: &RefsSnapshot, detached_head: Option<&str>) -> HashMap<Oid, Vec<Badge>> {
    let mut map: HashMap<Oid, Vec<Badge>> = HashMap::new();
    if let Some(oid) = detached_head.and_then(Oid::from_hex) {
        map.entry(oid).or_default().push(Badge {
            label: "HEAD".to_owned(),
            kind: BadgeKind::Detached,
        });
    }
    let rank = |r: &GitRef| match (&r.kind, r.head) {
        (RefKind::Local, true) => 0,
        (RefKind::Local, false) => 1,
        (RefKind::Remote { .. }, _) => 2,
        (RefKind::Tag, _) => 3,
    };
    let mut ordered: Vec<&GitRef> = refs.refs.iter().collect();
    ordered.sort_by_key(|r| rank(r));
    for r in ordered {
        let kind = match (&r.kind, r.head) {
            (RefKind::Local, true) => BadgeKind::Head,
            (RefKind::Local, false) => BadgeKind::Local,
            (RefKind::Remote { .. }, _) => BadgeKind::Remote,
            (RefKind::Tag, _) => BadgeKind::Tag,
        };
        let Some(oid) = Oid::from_hex(&r.oid) else {
            continue;
        };
        map.entry(oid).or_default().push(Badge {
            label: r.name.clone(),
            kind,
        });
    }
    map
}

/// What decides whether the history must be read again: every ref tip and HEAD.
pub fn tips(refs: &RefsSnapshot, head: Option<&str>) -> Vec<(String, String)> {
    let mut tips: Vec<(String, String)> = refs
        .refs
        .iter()
        .map(|r| (r.full_name.clone(), r.oid.clone()))
        .collect();
    tips.push(("HEAD".to_owned(), head.unwrap_or_default().to_owned()));
    tips.sort();
    tips
}

/// Branches, tags, remotes and stashes of the tab, plus the badges derived from them.
#[derive(Default)]
pub struct RefsState {
    pub snapshot: RefsSnapshot,
    pub badges: HashMap<Oid, Vec<Badge>>,
    pub loaded: bool,
    pub running: bool,
    /// Another refresh was requested while one was running.
    pub pending: bool,
    /// Every ref tip and HEAD as last read.
    pub tips: Option<Vec<(String, String)>>,
    /// The tips the shown history was read for.
    pub history_tips: Option<Vec<(String, String)>>,
    /// When the repository was last fetched (read with the refs).
    pub last_fetch: Option<std::time::SystemTime>,
}

pub enum DetailBody {
    None,
    Loading,
    Loaded {
        detail: CommitDetail,
        files: Vec<ChangedFile>,
    },
    Failed(String),
}

/// The selected commit's detail and the diff of one of its files.
pub struct CommitView {
    pub oid: Option<String>,
    pub body: DetailBody,
    pub generation: u64,
    pub cancel: Option<CancellationToken>,
    pub selected_file: Option<usize>,
    pub diff: DiffView,
}

impl Default for CommitView {
    fn default() -> Self {
        Self {
            oid: None,
            body: DetailBody::None,
            generation: 0,
            cancel: None,
            selected_file: None,
            diff: DiffView::default(),
        }
    }
}

impl CommitView {
    pub fn files(&self) -> &[ChangedFile] {
        match &self.body {
            DetailBody::Loaded { files, .. } => files,
            _ => &[],
        }
    }

    pub fn detail(&self) -> Option<&CommitDetail> {
        match &self.body {
            DetailBody::Loaded { detail, .. } => Some(detail),
            _ => None,
        }
    }
}

pub enum StashBody {
    Loading,
    Loaded(Vec<StashFile>),
    Failed(String),
}

/// The stash shown in the stash view: its files and the diff of one of them.
pub struct StashView {
    /// `None` when no stash is shown (or the shown one was dropped).
    pub entry: Option<StashEntry>,
    pub body: StashBody,
    pub generation: u64,
    pub cancel: Option<CancellationToken>,
    pub selected_file: Option<usize>,
    pub diff: DiffView,
}

impl Default for StashView {
    fn default() -> Self {
        Self {
            entry: None,
            body: StashBody::Loading,
            generation: 0,
            cancel: None,
            selected_file: None,
            diff: DiffView::default(),
        }
    }
}

impl StashView {
    pub fn files(&self) -> &[StashFile] {
        match &self.body {
            StashBody::Loaded(files) => files,
            _ => &[],
        }
    }

    /// Stop the loads in flight and forget the shown diff.
    pub fn cancel(&mut self) {
        for token in [self.cancel.take(), self.diff.cancel.take()]
            .into_iter()
            .flatten()
        {
            token.cancel();
        }
        self.generation += 1;
        self.diff.generation += 1;
        self.diff.key = None;
        self.diff.body = crate::workspace::DiffBody::None;
        self.selected_file = None;
    }
}

/// A complete history read in the background, swapped in when done (so a refresh never
/// shows a half-read list).
pub struct Incoming {
    pub commits: Vec<LogCommit>,
    pub rows: Vec<GraphRow>,
}

/// What the commit list shows instead of the full history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterKind {
    /// Commits matching a search, newest first.
    Search { kind: SearchKind, text: String },
    /// Commits that changed one file, following renames.
    File { path: String },
    /// Where HEAD has been (the reflog), newest first.
    Reflog,
}

impl FilterKind {
    pub fn title(&self) -> String {
        match self {
            Self::Search { kind, text } => format!("{}: {text}", kind.label()),
            Self::File { path } => format!("History of {path}"),
            Self::Reflog => "Reflog: where HEAD has been".to_owned(),
        }
    }

    /// What an empty result means.
    pub fn empty_hint(&self) -> String {
        match self {
            Self::Search { kind, .. } => {
                format!(
                    "No commit matches this {} search",
                    kind.label().to_lowercase()
                )
            }
            Self::File { .. } => "No commit changed this file".to_owned(),
            Self::Reflog => "HEAD has no recorded moves".to_owned(),
        }
    }
}

/// A full history longer than this is not kept aside while a filter is shown: it is freed and
/// read again (in the background, ~1 s for 100k commits) when the filter is left. Keeping both
/// a 100k-commit history and 100k search results cost ~50 MB and broke the 300 MB budget.
pub const KEEP_ASIDE_MAX: usize = 20_000;

/// The full history, put aside while the list shows a filter's results.
struct Saved {
    commits: Vec<LogCommit>,
    rows: Vec<GraphRow>,
    max_lanes: u16,
    selected: Option<usize>,
    /// The selected commit, to select again after a re-read.
    selected_oid: Option<Oid>,
    loaded: bool,
    /// A read was cut short by the filter, or the history was freed; read again when coming
    /// back.
    stale: bool,
}

/// Free a large list on another thread: half a million small allocations take the UI thread
/// several milliseconds to release.
fn free_later(commits: Vec<LogCommit>, rows: Vec<GraphRow>) {
    if commits.len() + rows.len() > 1_000 {
        std::thread::spawn(move || {
            drop((commits, rows));
            crate::platform::compact_heap();
        });
    }
}

pub struct ListFilter {
    pub kind: FilterKind,
    saved: Saved,
    /// Per row, a label shown before the subject (the reflog entry; the file's path in that
    /// commit when it differs from today's).
    pub labels: Vec<String>,
    /// File history: the file's path in each commit (follows renames).
    pub paths: Vec<String>,
}

/// A graph row for a flat list (search results have no meaningful graph).
pub fn flat_row(commit: &LogCommit) -> GraphRow {
    GraphRow {
        lane: 0,
        color: 0,
        merge: commit.parents.len() > 1,
        segments: Box::default(),
        width: 1,
    }
}

#[derive(Default)]
pub struct History {
    pub commits: Vec<LogCommit>,
    pub rows: Vec<GraphRow>,
    /// Widest graph in lanes (the graph column is sized for it, within a cap).
    pub max_lanes: u16,
    /// A read is in flight.
    pub loading: bool,
    /// A complete history has been shown at least once.
    pub loaded: bool,
    /// Bumped per read; pages of an older read are dropped.
    pub generation: u64,
    pub cancel: Option<CancellationToken>,
    /// Reload in progress (the visible list stays until it completes).
    pub incoming: Option<Incoming>,
    pub selected: Option<usize>,
    /// Scroll the list to the selection on the next frame.
    pub reveal: bool,
    /// Scroll the list to this offset on the next frame (the scroll benchmark).
    pub scroll_to: Option<f32>,
    /// A commit to select once the read in flight has delivered it (a branch clicked in the
    /// sidebar before the history was loaded).
    pub pending_reveal: Option<String>,
    pub commit: CommitView,
    /// Milliseconds to the first page and to the end of the last read (profiling).
    pub first_page_ms: Option<u32>,
    pub total_ms: Option<u32>,
    /// Search results, a file's history or the reflog shown instead of the full history.
    pub filter: Option<ListFilter>,
}

impl History {
    /// Start a new read. The first read shows pages as they come; later ones are collected
    /// and swapped in at the end.
    pub fn begin(&mut self, cancel: CancellationToken) -> u64 {
        if let Some(previous) = self.cancel.replace(cancel) {
            previous.cancel();
        }
        self.generation += 1;
        self.loading = true;
        self.incoming = self.loaded.then(|| Incoming {
            commits: Vec::new(),
            rows: Vec::new(),
        });
        if !self.loaded {
            self.commits.clear();
            self.rows.clear();
            self.max_lanes = 0;
        }
        self.generation
    }

    /// A page of the read `generation` arrived.
    pub fn page(&mut self, generation: u64, commits: Vec<LogCommit>, rows: Vec<GraphRow>) {
        if generation != self.generation {
            return;
        }
        match &mut self.incoming {
            Some(incoming) => {
                incoming.commits.extend(commits);
                incoming.rows.extend(rows);
            }
            None => {
                self.max_lanes = rows
                    .iter()
                    .map(|row| row.width)
                    .fold(self.max_lanes, u16::max);
                self.commits.extend(commits);
                self.rows.extend(rows);
            }
        }
    }

    /// The read finished. Returns whether the selected commit changed (its detail must be
    /// loaded again).
    pub fn finish(&mut self, generation: u64, ok: bool) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        self.cancel = None;
        let selected_oid = self.selected_oid();
        if let Some(incoming) = self.incoming.take() {
            if !ok {
                return false; // keep showing what we had
            }
            self.max_lanes = incoming.rows.iter().map(|row| row.width).max().unwrap_or(0);
            free_later(
                std::mem::replace(&mut self.commits, incoming.commits),
                std::mem::replace(&mut self.rows, incoming.rows),
            );
        }
        if ok {
            self.loaded = true;
            // Pages grew the lists by doubling; a 100k list keeps up to 30 % spare capacity
            // for as long as it is shown.
            if self.commits.capacity() > self.commits.len() + self.commits.len() / 8 {
                self.commits.shrink_to_fit();
                self.rows.shrink_to_fit();
            }
        }
        // Keep the same commit selected wherever it moved; drop it if it is gone.
        let found = selected_oid.and_then(|oid| self.commits.iter().position(|c| c.oid == oid));
        self.selected = found;
        if let Some(oid) = self.pending_reveal.take() {
            self.select_oid(&oid);
        }
        selected_oid.is_some() && found.is_none()
    }

    pub fn selected_oid(&self) -> Option<Oid> {
        self.selected
            .and_then(|index| self.commits.get(index))
            .map(|commit| commit.oid)
    }

    pub fn selected_commit(&self) -> Option<&LogCommit> {
        self.selected.and_then(|index| self.commits.get(index))
    }

    /// Select row `index`; returns whether the selection changed.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.commits.len() || self.selected == Some(index) {
            return false;
        }
        self.selected = Some(index);
        self.reveal = true;
        true
    }

    /// Move the selection by `step` rows (starting at the top when nothing is selected).
    pub fn move_selection(&mut self, step: isize) -> bool {
        if self.commits.is_empty() {
            return false;
        }
        let target = match self.selected {
            None => 0,
            Some(current) => current
                .saturating_add_signed(step)
                .min(self.commits.len() - 1),
        };
        self.select(target)
    }

    /// Show a filter's results instead of the full history; returns the read's generation.
    /// The full history is kept aside (once, however many filters follow each other).
    pub fn enter_filter(&mut self, kind: FilterKind, cancel: CancellationToken) -> u64 {
        let saved = match self.filter.take() {
            Some(previous) => {
                // The previous filter's results go.
                free_later(
                    std::mem::take(&mut self.commits),
                    std::mem::take(&mut self.rows),
                );
                previous.saved
            }
            None => {
                let selected_oid = self.selected_oid();
                let commits = std::mem::take(&mut self.commits);
                let rows = std::mem::take(&mut self.rows);
                if commits.len() > KEEP_ASIDE_MAX {
                    free_later(commits, rows);
                    Saved {
                        commits: Vec::new(),
                        rows: Vec::new(),
                        max_lanes: 1,
                        selected: None,
                        selected_oid,
                        loaded: false,
                        stale: true,
                    }
                } else {
                    Saved {
                        commits,
                        rows,
                        max_lanes: self.max_lanes,
                        selected: self.selected,
                        selected_oid,
                        loaded: self.loaded,
                        stale: self.loading,
                    }
                }
            }
        };
        self.incoming = None;
        self.pending_reveal = None;
        self.filter = Some(ListFilter {
            kind,
            saved,
            labels: Vec::new(),
            paths: Vec::new(),
        });
        self.loaded = false;
        self.selected = None;
        self.begin(cancel)
    }

    /// The whole result of a file history or reflog read: rows plus their labels.
    pub fn filter_rows(
        &mut self,
        generation: u64,
        commits: Vec<LogCommit>,
        labels: Vec<String>,
        paths: Vec<String>,
    ) {
        if generation != self.generation {
            return;
        }
        if let Some(filter) = &mut self.filter {
            filter.labels = labels;
            filter.paths = paths;
        }
        let rows = commits.iter().map(flat_row).collect();
        self.page(generation, commits, rows);
    }

    /// Go back to the full history. Returns whether it must be read again (its read was
    /// cut short, or never finished).
    pub fn exit_filter(&mut self) -> bool {
        let Some(filter) = self.filter.take() else {
            return false;
        };
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation += 1;
        self.loading = false;
        self.incoming = None;
        let saved = filter.saved;
        free_later(
            std::mem::replace(&mut self.commits, saved.commits),
            std::mem::replace(&mut self.rows, saved.rows),
        );
        self.max_lanes = saved.max_lanes;
        self.selected = saved.selected;
        self.loaded = saved.loaded;
        self.reveal = true;
        let reload = saved.stale || !saved.loaded;
        if reload && self.commits.is_empty() {
            // Freed while the filter was shown: select the same commit once it is read again.
            self.pending_reveal = saved.selected_oid.map(|oid| oid.to_string());
        }
        reload
    }

    /// The selected row's path in a file history (to open that file of the commit).
    pub fn filter_path(&self) -> Option<&str> {
        let filter = self.filter.as_ref()?;
        filter.paths.get(self.selected?).map(String::as_str)
    }

    /// Select the commit `oid` if it is in the list.
    pub fn select_oid(&mut self, oid: &str) -> bool {
        let Some(oid) = Oid::from_hex(oid) else {
            return false;
        };
        match self.commits.iter().position(|c| c.oid == oid) {
            Some(index) => {
                let changed = self.select(index);
                self.reveal = true;
                changed
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_git::graph::layout;
    use facet_git::parse::Track;

    fn commit(oid: &str, parents: &[&str]) -> LogCommit {
        LogCommit {
            oid: Oid::fake(oid),
            parents: facet_git::Parents::from_vec(parents.iter().map(|p| Oid::fake(p)).collect()),
            author: "A".into(),
            time: 0,
            subject: oid.into(),
        }
    }

    fn git_ref(full: &str, name: &str, kind: RefKind, oid: &str, head: bool) -> GitRef {
        GitRef {
            full_name: full.into(),
            name: name.into(),
            kind,
            oid: Oid::fake(oid).to_string(),
            tag_object: None,
            head,
            upstream: None,
            track: Track::None,
            date: 0,
            author: String::new(),
        }
    }

    #[test]
    fn first_read_shows_pages_and_reloads_swap_at_the_end() {
        let commits = vec![commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])];
        let rows = layout(&commits);
        let mut history = History::default();

        let first = history.begin(CancellationToken::new());
        history.page(first, commits[..1].to_vec(), rows[..1].to_vec());
        assert_eq!(
            history.commits.len(),
            1,
            "pages show while the first read runs"
        );
        history.page(first, commits[1..].to_vec(), rows[1..].to_vec());
        assert!(!history.finish(first, true));
        assert!(history.loaded && !history.loading);
        assert_eq!(history.max_lanes, 1);

        history.select(1);
        assert_eq!(history.selected_oid(), Some(Oid::fake("b")));

        // A reload keeps the old list until it is complete, and keeps the selection by id.
        let newer = vec![
            commit("d", &["c"]),
            commit("c", &["b"]),
            commit("b", &["a"]),
        ];
        let second = history.begin(CancellationToken::new());
        history.page(second, newer.clone(), layout(&newer));
        // A stale page from the first read is ignored.
        history.page(
            first,
            vec![commit("zzz", &[])],
            layout(&[commit("zzz", &[])]),
        );
        assert_eq!(history.commits.len(), 3);
        assert_eq!(history.commits[0].oid, Oid::fake("c"));
        assert!(!history.finish(second, true));
        assert_eq!(history.commits[0].oid, Oid::fake("d"));
        assert_eq!(history.selected, Some(2));

        // A failed reload keeps what was shown; a selected commit that disappears is
        // reported.
        let third = history.begin(CancellationToken::new());
        assert!(!history.finish(third, false));
        assert_eq!(history.commits.len(), 3);
        let fourth = history.begin(CancellationToken::new());
        history.page(fourth, vec![commit("x", &[])], layout(&[commit("x", &[])]));
        assert!(history.finish(fourth, true));
        assert_eq!(history.selected, None);
    }

    #[test]
    fn filters_set_the_history_aside_and_bring_it_back() {
        let commits = vec![commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])];
        let mut history = History::default();
        let generation = history.begin(CancellationToken::new());
        history.page(generation, commits.clone(), layout(&commits));
        history.finish(generation, true);
        history.select(1);

        let search = history.enter_filter(
            FilterKind::Search {
                kind: SearchKind::Message,
                text: "x".into(),
            },
            CancellationToken::new(),
        );
        assert!(history.commits.is_empty() && history.selected.is_none());
        history.page(
            search,
            vec![commit("a", &[])],
            vec![flat_row(&commit("a", &[]))],
        );
        history.finish(search, true);
        assert_eq!(history.commits.len(), 1);

        // A second filter replaces the first but keeps the same full history aside.
        let file = history.enter_filter(
            FilterKind::File { path: "f".into() },
            CancellationToken::new(),
        );
        history.filter_rows(
            file,
            vec![commit("b", &["a"])],
            vec!["old".into()],
            vec!["g".into()],
        );
        history.filter_rows(search, vec![commit("zzz", &[])], Vec::new(), Vec::new());
        history.finish(file, true);
        history.select(0);
        assert_eq!(history.filter_path(), Some("g"));
        assert_eq!(history.commits.len(), 1);

        assert!(!history.exit_filter(), "the full history was complete");
        assert_eq!(history.commits.len(), 3);
        assert_eq!(history.selected_oid(), Some(Oid::fake("b")));
        assert!(history.filter.is_none());
        assert!(!history.exit_filter());

        // A filter that cut a read short asks for the history to be read again.
        history.begin(CancellationToken::new());
        history.enter_filter(FilterKind::Reflog, CancellationToken::new());
        assert!(history.exit_filter());
    }

    #[test]
    fn a_long_history_is_freed_during_a_filter_and_read_again() {
        let commits: Vec<LogCommit> = (0..=KEEP_ASIDE_MAX)
            .map(|n| commit(&n.to_string(), &[]))
            .collect();
        let rows = commits.iter().map(flat_row).collect();
        let mut history = History::default();
        let generation = history.begin(CancellationToken::new());
        history.page(generation, commits, rows);
        history.finish(generation, true);
        history.select(7);
        let selected = history.selected_oid().unwrap();

        history.enter_filter(FilterKind::Reflog, CancellationToken::new());
        assert!(history.exit_filter(), "a freed history is read again");
        assert!(history.commits.is_empty() && !history.loaded);

        // The re-read selects the same commit again.
        let again = history.begin(CancellationToken::new());
        let commits: Vec<LogCommit> = (0..20).map(|n| commit(&n.to_string(), &[])).collect();
        let rows = commits.iter().map(flat_row).collect();
        history.page(again, commits, rows);
        history.finish(again, true);
        assert_eq!(history.selected_oid(), Some(selected));
    }

    #[test]
    fn keyboard_selection_stays_in_range() {
        let mut history = History::default();
        assert!(!history.move_selection(1));
        let commits = vec![commit("b", &["a"]), commit("a", &[])];
        let generation = history.begin(CancellationToken::new());
        history.page(generation, commits.clone(), layout(&commits));
        history.finish(generation, true);
        assert!(history.move_selection(1));
        assert_eq!(history.selected, Some(0));
        assert!(history.move_selection(5));
        assert_eq!(history.selected, Some(1));
        assert!(!history.move_selection(1));
        assert!(history.move_selection(-9));
        assert_eq!(history.selected, Some(0));
        assert!(history.select_oid(&Oid::fake("a").to_string()));
        assert!(!history.select_oid(&Oid::fake("missing").to_string()));
    }

    #[test]
    fn badges_are_grouped_by_commit_head_first() {
        let refs = RefsSnapshot {
            refs: vec![
                git_ref("refs/tags/v1", "v1", RefKind::Tag, "aaa", false),
                git_ref(
                    "refs/remotes/origin/main",
                    "origin/main",
                    RefKind::Remote {
                        remote: "origin".into(),
                    },
                    "aaa",
                    false,
                ),
                git_ref("refs/heads/main", "main", RefKind::Local, "aaa", true),
                git_ref("refs/heads/topic", "topic", RefKind::Local, "bbb", false),
            ],
            ..Default::default()
        };
        let map = badges(&refs, None);
        let labels: Vec<(&str, BadgeKind)> = map[&Oid::fake("aaa")]
            .iter()
            .map(|b| (b.label.as_str(), b.kind))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("main", BadgeKind::Head),
                ("origin/main", BadgeKind::Remote),
                ("v1", BadgeKind::Tag)
            ]
        );
        assert_eq!(map[&Oid::fake("bbb")][0].kind, BadgeKind::Local);

        let detached = badges(&refs, Some(&Oid::fake("bbb").to_string()));
        assert_eq!(detached[&Oid::fake("bbb")][0].kind, BadgeKind::Detached);

        // Moving any tip or HEAD changes the tips; reordering refs does not.
        let before = tips(&refs, Some("aaa"));
        let mut moved = refs.clone();
        moved.refs.reverse();
        assert_eq!(tips(&moved, Some("aaa")), before);
        moved.refs[0].oid = "ccc".into();
        assert_ne!(tips(&moved, Some("aaa")), before);
        assert_ne!(tips(&refs, Some("bbb")), before);
    }
}
