//! The working copy state of one repository tab: status lists, selection, the diff being
//! shown, the commit draft and the undo stack. Pure data and logic; the views in `ui/` read
//! it and `app_work.rs` drives it from background results.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use eframe::egui::TextureHandle;
use facet_git::parse::{BranchInfo, EntryKind, FileDiff, LineKind, LogCommit, Status, StatusEntry};
use facet_git::{CancellationToken, DiffRequest, DiffSource, PerfConfig, UndoEntry};

use crate::highlight::Decorations;

/// Which of the two file lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Area {
    Staged,
    Unstaged,
}

/// One row of the file list: a file, in the list its diff comes from. Every changed file has
/// exactly one row; its checkbox says whether it is staged.
pub type Row = (Area, usize);

/// Whether a file's changes are in the index: all, some (partly staged) or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Included {
    All,
    Some,
    None,
}

/// The selected files. A selection lives in one area at a time; `cursor` is the row the
/// keyboard is on and the file whose diff is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub area: Area,
    pub items: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub cursor: Option<usize>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            area: Area::Unstaged,
            items: BTreeSet::new(),
            anchor: None,
            cursor: None,
        }
    }
}

/// Identifies what the diff pane shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffKey {
    pub area: Area,
    pub request: DiffRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryInfo {
    /// "Unreal asset", "Binary file", …
    pub description: String,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
    /// The content is stored with Git LFS (the repository holds a pointer).
    pub lfs: bool,
}

/// A submodule pointer change, shown as the commits between the two recorded commits.
pub struct SubmoduleDiff {
    /// The commit recorded before and after (`None`: the submodule was added / removed).
    pub old: Option<String>,
    pub new: Option<String>,
    /// Commits the change brings in and drops, newest first.
    pub added: Vec<LogCommit>,
    pub removed: Vec<LogCommit>,
    /// Why the commits can't be listed (not checked out, commits not fetched).
    pub unavailable: Option<String>,
}

#[derive(Clone)]
pub struct ImageSide {
    pub texture: TextureHandle,
    /// Size of the original image in pixels (the texture may be scaled down).
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

#[derive(Clone)]
pub struct ImageDiff {
    pub old: Option<ImageSide>,
    pub new: Option<ImageSide>,
}

/// A drag across diff lines that selects or deselects them.
#[derive(Debug, Clone)]
pub struct LineDrag {
    pub anchor: u32,
    pub select: bool,
    pub base: BTreeSet<u32>,
}

pub struct TextDiff {
    pub diff: Arc<FileDiff>,
    pub decorations: Option<Arc<Decorations>>,
    /// Selected changed lines (indices into `diff.lines`) for line-level staging.
    pub selected: BTreeSet<u32>,
    pub drag: Option<LineDrag>,
    /// Longest line in bytes (capped), for the horizontal scroll extent.
    pub widest: usize,
    /// Hunks and lines can be staged / unstaged / discarded individually.
    pub partial: bool,
    /// Side-by-side rows: the old and the new line (indices into `diff.lines`, [`NO_LINE`]
    /// for a blank side).
    pub pairs: Vec<[u32; 2]>,
    /// Where each hunk's rows start in `pairs`.
    pub pair_starts: Vec<u32>,
}

/// A blank side of a side-by-side row.
pub const NO_LINE: u32 = u32::MAX;

/// Line up a diff's lines side by side: context on both sides, each run of removed lines
/// next to the added lines that replace it.
fn pair_lines(diff: &FileDiff) -> (Vec<[u32; 2]>, Vec<u32>) {
    let mut pairs = Vec::with_capacity(diff.lines.len());
    let mut starts = Vec::with_capacity(diff.hunks.len());
    let flush = |pairs: &mut Vec<[u32; 2]>, removed: &mut Vec<u32>, added: &mut Vec<u32>| {
        for row in 0..removed.len().max(added.len()) {
            pairs.push([
                removed.get(row).copied().unwrap_or(NO_LINE),
                added.get(row).copied().unwrap_or(NO_LINE),
            ]);
        }
        removed.clear();
        added.clear();
    };
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    for hunk in &diff.hunks {
        starts.push(pairs.len() as u32);
        for index in hunk.lines.clone() {
            match diff.lines[index as usize].kind {
                LineKind::Context => {
                    flush(&mut pairs, &mut removed, &mut added);
                    pairs.push([index, index]);
                }
                LineKind::Removed => {
                    if !added.is_empty() {
                        flush(&mut pairs, &mut removed, &mut added);
                    }
                    removed.push(index);
                }
                LineKind::Added => added.push(index),
            }
        }
        flush(&mut pairs, &mut removed, &mut added);
    }
    (pairs, starts)
}

impl TextDiff {
    pub fn new(diff: Arc<FileDiff>, partial: bool) -> Self {
        let widest = diff
            .lines
            .iter()
            .map(|line| (line.end - line.start) as usize)
            .max()
            .unwrap_or(0)
            .min(crate::highlight::MAX_LINE_BYTES + 1);
        let (pairs, pair_starts) = pair_lines(&diff);
        Self {
            diff,
            decorations: None,
            selected: BTreeSet::new(),
            drag: None,
            widest,
            partial,
            pairs,
            pair_starts,
        }
    }

    /// Rows of the diff view: one per hunk header plus one per line.
    pub fn row_count(&self) -> usize {
        self.diff.hunks.len() + self.diff.lines.len()
    }

    pub fn is_changed_line(&self, line: usize) -> bool {
        self.diff
            .lines
            .get(line)
            .is_some_and(|line| line.kind != LineKind::Context)
    }

    /// Select or deselect every changed line between `from` and `to` (inclusive).
    pub fn paint_selection(&mut self, from: u32, to: u32, select: bool, base: &BTreeSet<u32>) {
        let (low, high) = (from.min(to), from.max(to));
        let mut selected = base.clone();
        for line in low..=high {
            if self.is_changed_line(line as usize) {
                if select {
                    selected.insert(line);
                } else {
                    selected.remove(&line);
                }
            }
        }
        self.selected = selected;
    }
}

pub enum DiffBody {
    /// Nothing selected.
    None,
    Loading,
    Text(TextDiff),
    Binary(BinaryInfo),
    Image(ImageDiff),
    /// The two versions are identical (or differ only in file mode).
    Unchanged(Arc<FileDiff>),
    /// A submodule pointer change: the commit range.
    Submodule(SubmoduleDiff),
    Failed(String),
}

pub struct DiffView {
    pub key: Option<DiffKey>,
    pub body: DiffBody,
    /// Bumped for every load; results carrying an older value are dropped.
    pub generation: u64,
    pub cancel: Option<CancellationToken>,
    /// Scroll back to the top when the next frame is laid out (a different file was opened).
    pub reset_scroll: bool,
    /// Scroll the text diff to this offset on the next frame (the diff scroll benchmark).
    pub scroll_to: Option<f32>,
    /// Swipe position (0..=1) and onion-skin opacity (0..=1) of the image comparison.
    pub swipe: f32,
    pub onion: f32,
}

impl Default for DiffView {
    fn default() -> Self {
        Self {
            key: None,
            body: DiffBody::None,
            generation: 0,
            cancel: None,
            reset_scroll: false,
            scroll_to: None,
            swipe: 0.5,
            onion: 0.5,
        }
    }
}

impl DiffView {
    pub fn text(&self) -> Option<&TextDiff> {
        match &self.body {
            DiffBody::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextDiff> {
        match &mut self.body {
            DiffBody::Text(text) => Some(text),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct CommitDraft {
    pub message: String,
    pub amend: bool,
    /// The last commit's message as loaded when amend was switched on.
    pub amend_original: Option<String>,
    /// `git commit` is running.
    pub running: bool,
    /// What git and the hooks said when the last commit was rejected.
    pub error: Option<String>,
    /// Move keyboard focus into the message field on the next frame.
    pub focus_pending: bool,
}

#[derive(Default)]
pub struct RefreshState {
    pub running: bool,
    /// A refresh was requested while one was running: `Some(foreground)`.
    pub pending: Option<bool>,
    /// Bumped whenever Facet itself changed the repository; a status that started before
    /// the latest change is stale and is thrown away.
    pub epoch: u64,
}

#[derive(Default)]
pub struct PerfState {
    pub config: Option<PerfConfig>,
    /// Consecutive refreshes slower than the budget.
    pub slow_refreshes: u32,
    pub last_ms: Option<u32>,
}

#[derive(Default)]
pub struct Workspace {
    /// The first status has arrived.
    pub loaded: bool,
    pub branch: BranchInfo,
    pub entries: Vec<StatusEntry>,
    /// Indices into `entries`.
    pub staged: Vec<u32>,
    pub unstaged: Vec<u32>,
    /// The file list's rows: conflicts first, then by path.
    rows: Vec<Row>,
    /// Lines added / removed per path, `HEAD` to working tree.
    pub counts: HashMap<String, (u32, u32)>,
    pub selection: Selection,
    /// Scroll the file list to the cursor on the next frame.
    pub reveal_cursor: bool,
    pub diff: DiffView,
    /// The open diff may be out of date: reload it with the next status.
    pub diff_stale: bool,
    pub commit: CommitDraft,
    /// Discards that can be undone, oldest first.
    pub undo: Vec<UndoEntry>,
    pub refresh: RefreshState,
    pub perf: PerfState,
    /// Index-changing operations in flight.
    pub busy: u32,
}

impl Workspace {
    pub fn list(&self, area: Area) -> &[u32] {
        match area {
            Area::Staged => &self.staged,
            Area::Unstaged => &self.unstaged,
        }
    }

    pub fn entry(&self, area: Area, index: usize) -> Option<&StatusEntry> {
        self.list(area)
            .get(index)
            .map(|entry| &self.entries[*entry as usize])
    }

    /// Rows of the file list, one per changed file.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, row: usize) -> Row {
        self.rows[row]
    }

    /// The row showing this file (`None`: a partly staged file's staged side, which has no
    /// row of its own).
    pub fn row_of(&self, area: Area, index: usize) -> Option<usize> {
        self.rows.iter().position(|row| *row == (area, index))
    }

    /// Files with staged changes, of all changed files.
    pub fn included_count(&self) -> (usize, usize) {
        (self.staged.len(), self.rows.len())
    }

    /// How much of the file in this row is staged.
    pub fn included(&self, area: Area, index: usize) -> Included {
        let Some(entry) = self.entry(area, index) else {
            return Included::None;
        };
        match (entry.is_staged(), entry.is_unstaged()) {
            (true, false) => Included::All,
            (true, true) => Included::Some,
            _ => Included::None,
        }
    }

    /// Replace the status, keeping the selection on the same files where they still exist.
    pub fn apply_status(&mut self, status: Status) {
        let area = self.selection.area;
        let path_of = |this: &Self, index: usize| this.entry(area, index).map(|e| e.path.clone());
        let selected: Vec<String> = self
            .selection
            .items
            .iter()
            .filter_map(|index| path_of(self, *index))
            .collect();
        let cursor_path = self.selection.cursor.and_then(|index| path_of(self, index));
        let anchor_path = self.selection.anchor.and_then(|index| path_of(self, index));
        let old_cursor = self.selection.cursor;

        self.branch = status.branch;
        self.entries = status.entries;
        self.staged.clear();
        self.unstaged.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.is_staged() {
                self.staged.push(index as u32);
            }
            if entry.is_unstaged() {
                self.unstaged.push(index as u32);
            }
        }
        // Conflicts first, then everything by path (git lists untracked files separately).
        let entries = &self.entries;
        self.unstaged.sort_by(|a, b| {
            let (a, b) = (&entries[*a as usize], &entries[*b as usize]);
            (a.kind != EntryKind::Unmerged, &a.path).cmp(&(b.kind != EntryKind::Unmerged, &b.path))
        });
        // One row per file: its unstaged side when it has one, else its staged side.
        let staged_at: HashMap<u32, usize> = self
            .staged
            .iter()
            .enumerate()
            .map(|(position, entry)| (*entry, position))
            .collect();
        let unstaged_at: HashMap<u32, usize> = self
            .unstaged
            .iter()
            .enumerate()
            .map(|(position, entry)| (*entry, position))
            .collect();
        let mut order: Vec<u32> = (0..self.entries.len() as u32).collect();
        order.sort_by(|a, b| {
            let (a, b) = (&entries[*a as usize], &entries[*b as usize]);
            (a.kind != EntryKind::Unmerged, &a.path).cmp(&(b.kind != EntryKind::Unmerged, &b.path))
        });
        self.rows = order
            .into_iter()
            .filter_map(|entry| {
                unstaged_at
                    .get(&entry)
                    .map(|position| (Area::Unstaged, *position))
                    .or_else(|| staged_at.get(&entry).map(|position| (Area::Staged, *position)))
            })
            .collect();
        self.loaded = true;

        // Re-find the selected files.
        let mut selection = Selection {
            area,
            ..Selection::default()
        };
        if !selected.is_empty() || cursor_path.is_some() {
            let positions: HashMap<&str, usize> = self
                .list(area)
                .iter()
                .enumerate()
                .map(|(index, entry)| (self.entries[*entry as usize].path.as_str(), index))
                .collect();
            let find = |path: &Option<String>| {
                path.as_deref()
                    .and_then(|path| positions.get(path).copied())
            };
            selection.items = selected
                .iter()
                .filter_map(|path| positions.get(path.as_str()).copied())
                .collect();
            selection.cursor = find(&cursor_path);
            selection.anchor = find(&anchor_path);
        }
        // The file under the cursor was staged or unstaged: its row now shows the other list,
        // so follow it there.
        let other = match area {
            Area::Staged => Area::Unstaged,
            Area::Unstaged => Area::Staged,
        };
        if selection.cursor.is_none() && cursor_path.is_some() {
            let moved = self.list(other).iter().position(|entry| {
                Some(&self.entries[*entry as usize].path) == cursor_path.as_ref()
            });
            if let Some(index) = moved.filter(|index| self.row_of(other, *index).is_some()) {
                selection = Selection {
                    area: other,
                    items: BTreeSet::from([index]),
                    anchor: Some(index),
                    cursor: Some(index),
                };
            }
        }
        // The file under the cursor is gone (committed, discarded…): stay at the same
        // position so the next file is ready, else move to the other list.
        if selection.cursor.is_none() && old_cursor.is_some() {
            let len = self.list(area).len();
            if len > 0 {
                selection.cursor = Some(old_cursor.unwrap_or(0).min(len - 1));
            } else if !self.list(other).is_empty() {
                selection.area = other;
                selection.cursor = Some(0);
                selection.items.clear();
            }
            selection.anchor = selection.cursor;
            if selection.items.is_empty() {
                selection.items.extend(selection.cursor);
            }
        }
        self.selection = selection;
    }

    /// The file whose diff is shown.
    pub fn current(&self) -> Option<(Area, &StatusEntry)> {
        let area = self.selection.area;
        self.selection
            .cursor
            .and_then(|index| self.entry(area, index))
            .map(|entry| (area, entry))
    }

    /// The diff to show for the current file.
    pub fn current_key(&self) -> Option<DiffKey> {
        let (area, entry) = self.current()?;
        let source = match (area, entry.kind) {
            (Area::Staged, _) => DiffSource::Staged,
            (Area::Unstaged, EntryKind::Untracked) => DiffSource::Untracked,
            (Area::Unstaged, EntryKind::Unmerged) => DiffSource::Conflict,
            (Area::Unstaged, _) => DiffSource::Unstaged,
        };
        Some(DiffKey {
            area,
            request: DiffRequest {
                path: entry.path.clone(),
                orig_path: (area == Area::Staged)
                    .then(|| entry.orig_path.clone())
                    .flatten(),
                source,
            },
        })
    }

    pub fn selected_entries(&self) -> Vec<&StatusEntry> {
        self.selection
            .items
            .iter()
            .filter_map(|index| self.entry(self.selection.area, *index))
            .collect()
    }

    /// A click on a file row. `toggle` is Ctrl, `extend` is Shift.
    pub fn click(&mut self, area: Area, index: usize, toggle: bool, extend: bool) {
        if index >= self.list(area).len() {
            return;
        }
        let selection = &mut self.selection;
        if selection.area != area {
            *selection = Selection {
                area,
                ..Selection::default()
            };
        }
        if extend {
            let anchor = selection.anchor.unwrap_or(index);
            selection.items = (anchor.min(index)..=anchor.max(index)).collect();
            selection.anchor = Some(anchor);
        } else if toggle {
            if !selection.items.remove(&index) {
                selection.items.insert(index);
            }
            selection.anchor = Some(index);
        } else {
            selection.items = BTreeSet::from([index]);
            selection.anchor = Some(index);
        }
        selection.cursor = Some(index);
    }

    /// Move the cursor up or down the file list (keyboard navigation).
    pub fn move_cursor(&mut self, step: isize, extend: bool) {
        let files = self.rows.clone();
        if files.is_empty() {
            return;
        }
        let current = self.selection.cursor.and_then(|cursor| {
            files
                .iter()
                .position(|f| *f == (self.selection.area, cursor))
        });
        let target = match current {
            Some(current) => (current as isize + step).clamp(0, files.len() as isize - 1) as usize,
            None if step < 0 => files.len() - 1,
            None => 0,
        };
        let (area, index) = files[target];
        // A range cannot span the two lists; crossing over starts a new selection.
        self.click(area, index, false, extend && area == self.selection.area);
        self.reveal_cursor = true;
    }

    pub fn select_all(&mut self) {
        let area = self.selection.area;
        let len = self.list(area).len();
        if len == 0 {
            return;
        }
        self.selection.items = (0..len).collect();
        self.selection.anchor = Some(0);
        self.selection.cursor.get_or_insert(0);
    }

    /// Should the offer to enable fsmonitor / untracked cache be shown?
    pub fn perf_offer(&self) -> bool {
        self.perf.slow_refreshes >= 2 && self.perf.config.is_some_and(|c| !c.all_enabled())
    }

    /// The most recent discard that can be undone (or redone).
    pub fn last_undo(&self) -> Option<&UndoEntry> {
        self.undo.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_git::parse::Change;

    fn entry(path: &str, index: Change, worktree: Change) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            kind: EntryKind::Tracked,
            index,
            worktree,
            conflict: None,
            submodule: false,
            head_oid: None,
            index_oid: None,
        }
    }

    fn unstaged(path: &str) -> StatusEntry {
        entry(path, Change::Unmodified, Change::Modified)
    }

    fn staged(path: &str) -> StatusEntry {
        entry(path, Change::Modified, Change::Unmodified)
    }

    fn untracked(path: &str) -> StatusEntry {
        StatusEntry {
            kind: EntryKind::Untracked,
            ..entry(path, Change::Unmodified, Change::Unmodified)
        }
    }

    fn conflict(path: &str) -> StatusEntry {
        StatusEntry {
            kind: EntryKind::Unmerged,
            conflict: Some(*b"UU"),
            ..entry(path, Change::Unmerged, Change::Unmerged)
        }
    }

    fn workspace(entries: Vec<StatusEntry>) -> Workspace {
        let mut workspace = Workspace::default();
        workspace.apply_status(Status {
            branch: BranchInfo::default(),
            entries,
        });
        workspace
    }

    fn paths(workspace: &Workspace, area: Area) -> Vec<&str> {
        (0..workspace.list(area).len())
            .map(|index| workspace.entry(area, index).unwrap().path.as_str())
            .collect()
    }

    #[test]
    fn lists_and_rows() {
        let workspace = workspace(vec![
            entry("both.txt", Change::Modified, Change::Modified),
            staged("s.txt"),
            unstaged("z.txt"),
            untracked("a-new.txt"),
            conflict("m-conflict.txt"),
        ]);
        assert_eq!(paths(&workspace, Area::Staged), ["both.txt", "s.txt"]);
        // Conflicts first, then by path with untracked files mixed in.
        assert_eq!(
            paths(&workspace, Area::Unstaged),
            ["m-conflict.txt", "a-new.txt", "both.txt", "z.txt"]
        );
        // One row per file, conflicts first; a partly staged file shows its unstaged side.
        assert_eq!(workspace.row_count(), 5);
        assert_eq!(workspace.row(0), (Area::Unstaged, 0));
        assert_eq!(workspace.row(1), (Area::Unstaged, 1));
        assert_eq!(workspace.row(2), (Area::Unstaged, 2));
        assert_eq!(workspace.row(3), (Area::Staged, 1));
        assert_eq!(workspace.row(4), (Area::Unstaged, 3));
        assert_eq!(workspace.row_of(Area::Staged, 0), None);
        assert_eq!(workspace.row_of(Area::Unstaged, 3), Some(4));
        assert_eq!(workspace.included_count(), (2, 5));
        assert_eq!(workspace.included(Area::Unstaged, 2), Included::Some);
        assert_eq!(workspace.included(Area::Staged, 1), Included::All);
        assert_eq!(workspace.included(Area::Unstaged, 3), Included::None);
    }

    #[test]
    fn clicks_select_toggle_and_extend() {
        let mut w = workspace((0..6).map(|n| unstaged(&format!("f{n}.txt"))).collect());
        w.click(Area::Unstaged, 1, false, false);
        assert_eq!(w.selection.items, BTreeSet::from([1]));
        w.click(Area::Unstaged, 4, false, true);
        assert_eq!(w.selection.items, BTreeSet::from([1, 2, 3, 4]));
        assert_eq!(w.selection.cursor, Some(4));
        w.click(Area::Unstaged, 2, true, false);
        assert_eq!(w.selection.items, BTreeSet::from([1, 3, 4]));
        w.click(Area::Unstaged, 0, false, false);
        assert_eq!(w.selection.items, BTreeSet::from([0]));
        w.click(Area::Unstaged, 99, false, false);
        assert_eq!(w.selection.items, BTreeSet::from([0]));
        w.select_all();
        assert_eq!(w.selection.items.len(), 6);
        assert_eq!(w.selected_entries().len(), 6);
    }

    #[test]
    fn the_cursor_walks_through_both_lists() {
        let mut w = workspace(vec![
            staged("a1"),
            staged("a2"),
            unstaged("u1"),
            unstaged("u2"),
        ]);
        w.move_cursor(1, false);
        assert_eq!(
            (w.selection.area, w.selection.cursor),
            (Area::Staged, Some(0))
        );
        w.move_cursor(1, false);
        w.move_cursor(1, false);
        assert_eq!(
            (w.selection.area, w.selection.cursor),
            (Area::Unstaged, Some(0))
        );
        w.move_cursor(1, true);
        assert_eq!(w.selection.items, BTreeSet::from([0, 1]));
        w.move_cursor(5, false);
        assert_eq!(w.selection.cursor, Some(1));
        w.move_cursor(-3, false);
        assert_eq!(
            (w.selection.area, w.selection.cursor),
            (Area::Staged, Some(0))
        );
        assert!(w.reveal_cursor);
    }

    #[test]
    fn selection_follows_files_across_a_refresh() {
        let mut w = workspace(vec![
            unstaged("a"),
            unstaged("b"),
            unstaged("c"),
            unstaged("d"),
        ]);
        w.click(Area::Unstaged, 1, false, false);
        w.click(Area::Unstaged, 3, true, false);
        // `a` disappears, `bb` appears: b and d shift.
        w.apply_status(Status {
            branch: BranchInfo::default(),
            entries: vec![unstaged("b"), unstaged("bb"), unstaged("c"), unstaged("d")],
        });
        assert_eq!(w.selection.items, BTreeSet::from([0, 3]));
        assert_eq!(w.selection.cursor, Some(3));
        assert_eq!(w.current().unwrap().1.path, "d");
    }

    #[test]
    fn staging_the_current_file_keeps_it_selected() {
        let mut w = workspace(vec![unstaged("a"), unstaged("b"), unstaged("c")]);
        w.click(Area::Unstaged, 1, false, false);
        // `b` was staged: its row stays where it was, now checked.
        w.apply_status(Status {
            branch: BranchInfo::default(),
            entries: vec![unstaged("a"), staged("b"), unstaged("c")],
        });
        assert_eq!(w.selection.area, Area::Staged);
        assert_eq!(w.current().unwrap().1.path, "b");
        assert_eq!(w.row_of(Area::Staged, 0), Some(1));
        // Unstaged again.
        w.apply_status(Status {
            branch: BranchInfo::default(),
            entries: vec![unstaged("a"), unstaged("b"), unstaged("c")],
        });
        assert_eq!(w.selection.area, Area::Unstaged);
        assert_eq!(w.current().unwrap().1.path, "b");

        // A committed or discarded file: the cursor stays on the same position…
        w.apply_status(Status {
            branch: BranchInfo::default(),
            entries: vec![unstaged("a"), unstaged("c")],
        });
        assert_eq!(w.current().unwrap().1.path, "c");
        // …and when the list empties, the selection moves to the other list.
        w.apply_status(Status {
            branch: BranchInfo::default(),
            entries: vec![staged("a"), staged("x")],
        });
        assert_eq!(w.selection.area, Area::Staged);
        assert_eq!(w.current().unwrap().1.path, "a");
        // Nothing left at all.
        w.apply_status(Status::default());
        assert!(w.current().is_none());
        assert!(w.selection.items.is_empty());
    }

    #[test]
    fn diff_keys() {
        let mut renamed = staged("new.txt");
        renamed.orig_path = Some("old.txt".into());
        let mut w = workspace(vec![
            renamed,
            untracked("u.txt"),
            conflict("c.txt"),
            unstaged("m.txt"),
        ]);
        assert!(w.current_key().is_none());
        w.click(Area::Staged, 0, false, false);
        let key = w.current_key().unwrap();
        assert_eq!(key.request.source, DiffSource::Staged);
        assert_eq!(key.request.orig_path.as_deref(), Some("old.txt"));
        let sources: Vec<DiffSource> = (0..3)
            .map(|index| {
                w.click(Area::Unstaged, index, false, false);
                w.current_key().unwrap().request.source
            })
            .collect();
        assert_eq!(
            sources,
            [
                DiffSource::Conflict,
                DiffSource::Unstaged,
                DiffSource::Untracked
            ]
        );
    }

    #[test]
    fn diff_pairs_and_line_painting() {
        let diff = facet_git::parse::parse_file_diff(
            b"2\t2\tf\0\0diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -9,2 +9,2 @@\n-x\n+X\n y\n"
                .to_vec(),
        )
        .unwrap();
        let mut text = TextDiff::new(Arc::new(diff), true);
        assert_eq!(text.row_count(), 8);
        // Side by side: context on both sides, a removed line next to the line replacing it.
        assert_eq!(text.pairs, vec![[0, 0], [1, 2], [3, 4], [5, 5]]);
        assert_eq!(text.pair_starts, vec![0, 2]);
        // Painting over a range only picks changed lines.
        text.paint_selection(0, 4, true, &BTreeSet::new());
        assert_eq!(text.selected, BTreeSet::from([1, 2, 3, 4]));
        let base = text.selected.clone();
        text.paint_selection(3, 2, false, &base);
        assert_eq!(text.selected, BTreeSet::from([1, 4]));
        assert_eq!(text.widest, 1);
    }

    #[test]
    fn the_perf_offer_needs_two_slow_refreshes_and_disabled_settings() {
        let mut w = Workspace::default();
        assert!(!w.perf_offer());
        w.perf.slow_refreshes = 2;
        assert!(!w.perf_offer(), "config not known yet");
        w.perf.config = Some(PerfConfig::default());
        assert!(w.perf_offer());
        w.perf.config = Some(PerfConfig {
            fsmonitor: true,
            untracked_cache: true,
        });
        assert!(!w.perf_offer());
    }
}
