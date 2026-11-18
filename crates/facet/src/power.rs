//! M3 state of a repository tab: the operation in progress (rebase, merge, cherry-pick…),
//! the undo stack of history-rewriting operations, the search bar, the blame view, the
//! interactive rebase editor and the conflict resolver. Pure data and logic; `app_power.rs`
//! drives it and `ui/power.rs` draws it.

use facet_git::conflict::{Choice, Region};
use facet_git::parse::{BlameCommit, BlameRange, LogCommit};
use facet_git::rebase::{TodoAction, TodoItem};
use facet_git::{CancellationToken, Operation, SearchKind, UndoPoint};

/// Undo entries kept per repository.
const UNDO_DEPTH: usize = 20;

#[derive(Default)]
pub struct PowerState {
    /// What `.git` says is in progress, read with the refs.
    pub operation: Option<Operation>,
    /// History-rewriting operations that can be undone, oldest first.
    pub undo: Vec<UndoItem>,
    pub search: SearchBar,
    pub blame: Option<BlameView>,
    pub rebase: Option<RebaseEditor>,
    pub conflict: Option<ConflictEditor>,
}

impl PowerState {
    pub fn push_undo(&mut self, item: UndoItem) {
        self.undo.push(item);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    /// Forget `item` (it was just undone), wherever it is in the stack.
    pub fn forget_undo(&mut self, point: &UndoPoint) {
        self.undo.retain(|item| item.point != *point);
    }
}

/// An operation that moved HEAD, and how to put HEAD back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoItem {
    /// "Cherry-picked 4659738", "Reset main to 1a2b3c4"…
    pub title: String,
    pub point: UndoPoint,
}

/// The search field above the commit list.
pub struct SearchBar {
    pub open: bool,
    pub kind: SearchKind,
    pub text: String,
    /// Give the text field keyboard focus on the next frame.
    pub focus: bool,
}

impl Default for SearchBar {
    fn default() -> Self {
        Self {
            open: false,
            kind: SearchKind::Message,
            text: String::new(),
            focus: false,
        }
    }
}

/// Who last changed each line of a file.
pub struct BlameView {
    pub path: String,
    /// `None`: the working copy; otherwise as of this commit.
    pub rev: Option<String>,
    /// The file's lines (read separately from the blame itself).
    pub lines: Vec<String>,
    /// Per line: index into `commits` + 1, or 0 while not known yet.
    pub owners: Vec<u32>,
    pub commits: Vec<BlameCommit>,
    pub loading: bool,
    pub error: Option<String>,
    pub generation: u64,
    pub cancel: Option<CancellationToken>,
    pub selected: Option<usize>,
}

impl BlameView {
    pub fn new(path: String, rev: Option<String>, generation: u64) -> Self {
        Self {
            path,
            rev,
            lines: Vec::new(),
            owners: Vec::new(),
            commits: Vec::new(),
            loading: true,
            error: None,
            generation,
            cancel: None,
            selected: None,
        }
    }

    /// The file's text arrived.
    pub fn set_text(&mut self, text: &[u8]) {
        let text = String::from_utf8_lossy(text);
        self.lines = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
            .collect();
        // A final newline does not start another line.
        if self.lines.last().is_some_and(String::is_empty) {
            self.lines.pop();
        }
        self.owners.resize(self.lines.len(), 0);
    }

    /// More of the blame arrived: new commits (appended in order) and ranges.
    pub fn add(&mut self, commits: Vec<BlameCommit>, ranges: &[BlameRange]) {
        self.commits.extend(commits);
        for range in ranges {
            let start = range.line.saturating_sub(1) as usize;
            let end = start + range.count as usize;
            if self.owners.len() < end {
                self.owners.resize(end, 0);
            }
            let owner = u32::try_from(range.commit + 1).unwrap_or(0);
            self.owners[start..end].fill(owner);
        }
    }

    pub fn owner(&self, line: usize) -> Option<&BlameCommit> {
        let owner = *self.owners.get(line)?;
        owner
            .checked_sub(1)
            .and_then(|index| self.commits.get(index as usize))
    }

    /// Whether `line` starts a run of lines from one commit (its gutter shows the commit).
    pub fn starts_run(&self, line: usize) -> bool {
        line == 0 || self.owners.get(line) != self.owners.get(line - 1)
    }
}

/// The interactive rebase editor: the todo list being prepared.
pub struct RebaseEditor {
    /// Rebase onto this commit (exclusive); `None` rewrites from the root commit.
    pub base: Option<String>,
    /// How the dialog names where the rebase starts.
    pub base_label: String,
    /// Oldest first, as git applies them.
    pub items: Vec<TodoItem>,
    pub loading: bool,
    pub error: Option<String>,
    /// The row whose controls have keyboard focus.
    pub selected: usize,
    /// The commits in plan order, to tell whether rows were moved.
    original: Vec<String>,
}

impl RebaseEditor {
    pub fn new(base: Option<String>, base_label: String) -> Self {
        Self {
            base,
            base_label,
            items: Vec::new(),
            loading: true,
            error: None,
            selected: 0,
            original: Vec::new(),
        }
    }

    pub fn set_plan(&mut self, commits: Vec<LogCommit>) {
        self.loading = false;
        self.items = commits
            .into_iter()
            .map(|commit| TodoItem {
                action: TodoAction::Pick,
                oid: commit.oid.to_string(),
                subject: commit.subject.into(),
                message: None,
            })
            .collect();
        self.original = self.items.iter().map(|item| item.oid.clone()).collect();
        self.selected = self.items.len().saturating_sub(1);
    }

    pub fn set_action(&mut self, row: usize, action: TodoAction) {
        let Some(item) = self.items.get_mut(row) else {
            return;
        };
        item.action = action;
        if action == TodoAction::Reword && item.message.is_none() {
            item.message = Some(item.subject.clone());
        }
        self.selected = row;
    }

    /// Move a row `step` places: `-1` up, `1` down, more when it is dragged. The rows in
    /// between shift by one.
    pub fn move_row(&mut self, row: usize, step: isize) {
        let Some(target) = row.checked_add_signed(step) else {
            return;
        };
        if row < self.items.len() && target < self.items.len() && target != row {
            let item = self.items.remove(row);
            self.items.insert(target, item);
            self.selected = target;
        }
    }

    /// Whether the plan changes anything at all (an unchanged list is a no-op rebase).
    pub fn changes_something(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.action != TodoAction::Pick)
            || !self
                .items
                .iter()
                .map(|item| &item.oid)
                .eq(self.original.iter())
    }
}

/// Resolving one conflicted file chunk by chunk.
pub struct ConflictEditor {
    pub path: String,
    /// `None` while the file is read, or when it has no conflict markers.
    pub regions: Option<Vec<Region>>,
    /// One per conflict chunk, in order.
    pub choices: Vec<Option<Choice>>,
    pub loading: bool,
    pub error: Option<String>,
}

impl ConflictEditor {
    pub fn new(path: String) -> Self {
        Self {
            path,
            regions: None,
            choices: Vec::new(),
            loading: true,
            error: None,
        }
    }

    pub fn set_regions(&mut self, regions: Vec<Region>) {
        let chunks = regions
            .iter()
            .filter(|region| matches!(region, Region::Conflict(_)))
            .count();
        self.choices = vec![None; chunks];
        self.regions = Some(regions);
        self.loading = false;
    }

    pub fn chunks(&self) -> usize {
        self.choices.len()
    }

    pub fn resolved(&self) -> usize {
        self.choices
            .iter()
            .filter(|choice| choice.is_some())
            .count()
    }

    pub fn choose(&mut self, chunk: usize, choice: Option<Choice>) {
        if let Some(slot) = self.choices.get_mut(chunk) {
            *slot = choice;
        }
    }

    /// Every chunk the same way.
    pub fn choose_all(&mut self, choice: Choice) {
        self.choices.fill(Some(choice));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facet_git::conflict::parse_markers;

    fn commit(oid: &str) -> LogCommit {
        LogCommit {
            oid: facet_git::Oid::from_hex(&oid.repeat(40)).unwrap(),
            parents: facet_git::Parents::None,
            author: "A".into(),
            time: 0,
            subject: format!("subject {oid}").into(),
        }
    }

    #[test]
    fn rebase_editor_tracks_changes() {
        let mut editor = RebaseEditor::new(Some("base".into()), "base".into());
        editor.set_plan(vec![commit("a"), commit("b"), commit("c")]);
        assert!(!editor.loading);
        assert_eq!(editor.selected, 2);
        assert!(!editor.changes_something());

        editor.move_row(2, -1);
        assert_eq!(&editor.items[1].oid[..1], "c");
        assert_eq!(editor.selected, 1);
        assert!(editor.changes_something());
        editor.move_row(1, 1);
        assert!(!editor.changes_something());
        // Out of range moves do nothing.
        editor.move_row(0, -1);
        editor.move_row(2, 1);
        assert_eq!(&editor.items[0].oid[..1], "a");
        // A drag moves a row several places; the rows in between shift.
        editor.move_row(0, 2);
        let order: Vec<&str> = editor.items.iter().map(|item| &item.oid[..1]).collect();
        assert_eq!(order, ["b", "c", "a"]);
        assert_eq!(editor.selected, 2);
        editor.move_row(2, -2);
        assert!(!editor.changes_something());

        editor.set_action(1, TodoAction::Reword);
        assert_eq!(editor.items[1].message.as_deref(), Some("subject b"));
        assert!(editor.changes_something());
    }

    #[test]
    fn blame_owners_follow_ranges() {
        let mut view = BlameView::new("f".into(), None, 1);
        view.set_text(
            b"one
two
three
",
        );
        assert_eq!(view.lines, vec!["one", "two", "three"]);
        let commit = |oid: &str| BlameCommit {
            oid: oid.into(),
            ..BlameCommit::default()
        };
        view.add(
            vec![commit("x")],
            &[BlameRange {
                commit: 0,
                line: 2,
                count: 2,
                orig_line: 1,
                filename: "f".into(),
            }],
        );
        assert!(view.owner(0).is_none());
        assert_eq!(view.owner(1).map(|c| c.oid.as_str()), Some("x"));
        assert!(view.starts_run(1));
        assert!(!view.starts_run(2));
        view.add(
            vec![commit("y")],
            &[BlameRange {
                commit: 1,
                line: 1,
                count: 1,
                orig_line: 1,
                filename: "f".into(),
            }],
        );
        assert_eq!(view.owner(0).map(|c| c.oid.as_str()), Some("y"));
    }

    #[test]
    fn conflict_editor_counts_chunks() {
        let text = "a
<<<<<<< HEAD
ours
=======
theirs
>>>>>>> topic
b
";
        let mut editor = ConflictEditor::new("f".into());
        editor.set_regions(parse_markers(text).unwrap());
        assert_eq!((editor.chunks(), editor.resolved()), (1, 0));
        editor.choose(0, Some(Choice::Theirs));
        assert_eq!(editor.resolved(), 1);
        editor.choose(5, Some(Choice::Ours));
        editor.choose_all(Choice::Ours);
        assert_eq!(editor.choices, vec![Some(Choice::Ours)]);
    }

    #[test]
    fn undo_stack_is_bounded_and_forgets_undone_entries() {
        let mut power = PowerState::default();
        for n in 0..25 {
            power.push_undo(UndoItem {
                title: format!("op {n}"),
                point: UndoPoint {
                    head: format!("{n}"),
                    mode: facet_git::ResetMode::Keep,
                    stash: None,
                },
            });
        }
        assert_eq!(power.undo.len(), UNDO_DEPTH);
        assert_eq!(power.undo[0].title, "op 5");
        let point = power.undo[3].point.clone();
        power.forget_undo(&point);
        assert_eq!(power.undo.len(), UNDO_DEPTH - 1);
    }
}
