//! Every user-invokable action in Facet.
//!
//! Buttons, keyboard shortcuts and the command palette all go through [`Action`], so the
//! palette reaches everything by construction: a feature is added by adding a variant here,
//! listing it in [`commands`], and handling it in `FacetApp::run`.

use std::path::PathBuf;

use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use facet_git::{DiscardFile, PerfConfig};

use crate::settings::{ImageMode, ThemeMode};
use crate::workspace::Area;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    ShowPalette,
    OpenRepository,
    OpenPath(PathBuf),
    ForgetRecent(PathBuf),
    CloseTab(usize),
    CloseActiveTab,
    NextTab,
    PreviousTab,
    SwitchTab(usize),
    Refresh,
    RevealInExplorer,
    CopyRepositoryPath,
    ToggleSidebar,
    ToggleCommandLog,
    ClearCommandLog,
    CopyText {
        text: String,
        what: &'static str,
    },
    SetTheme(ThemeMode),
    Quit,

    // ----- working copy: file list ---------------------------------------------------------
    /// A click on a file row (`toggle`: Ctrl, `extend`: Shift).
    SelectFile {
        area: Area,
        index: usize,
        toggle: bool,
        extend: bool,
    },
    SelectNextFile,
    SelectPreviousFile,
    /// Shift+Arrow: grow the selection by one row up (-1) or down (1).
    ExtendFileSelection(isize),
    SelectAllFiles,
    StageSelected,
    UnstageSelected,
    /// Stage the selection if it is in "Changes", unstage it if it is in "Staged".
    ToggleStageSelected,
    StagePaths(Vec<String>),
    UnstagePaths(Vec<String>),
    StageAll,
    UnstageAll,
    /// Discards ask for confirmation first; the `Confirmed…` variants do the work.
    DiscardSelected,
    DiscardAll,
    DiscardPath(String),
    ConfirmedDiscardFiles {
        files: Vec<DiscardFile>,
        label: String,
    },
    ConfirmedDiscardPatch {
        patch: Vec<u8>,
        label: String,
    },
    /// Undo the latest discard (or redo it, right after an undo).
    UndoDiscard,

    // ----- working copy: diff --------------------------------------------------------------
    StageHunk(usize),
    UnstageHunk(usize),
    DiscardHunk(usize),
    StageLines,
    UnstageLines,
    DiscardLines,
    ClearLineSelection,
    CopySelectedLines,
    CopyFilePath,
    SetImageMode(ImageMode),

    // ----- working copy: commit ------------------------------------------------------------
    Commit,
    ToggleAmend,
    FocusCommitMessage,
    DismissCommitError,

    // ----- per-repository performance settings ---------------------------------------------
    EnablePerf,
    DisablePerf,
    DeclinePerfOffer,
}

pub mod shortcut {
    use super::{Key, KeyboardShortcut, Modifiers};

    const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);

    pub const PALETTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::P);
    pub const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::O);
    pub const CLOSE_TAB: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::W);
    pub const NEXT_TAB: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Tab);
    pub const PREVIOUS_TAB: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::Tab);
    pub const REFRESH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
    pub const REVEAL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::E);
    pub const COPY_PATH: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::C);
    pub const SIDEBAR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::B);
    pub const COMMAND_LOG: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Backtick);

    pub const COMMIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Enter);
    pub const FOCUS_MESSAGE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::M);
    pub const STAGE_ALL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::A);
    pub const UNSTAGE_ALL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::U);

    // The shortcuts below belong to the file list and the diff: they apply while no text
    // field or button has keyboard focus (see `FacetApp::handle_work_keys`).
    pub const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Z);
    pub const SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::A);
    pub const TOGGLE_STAGE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Enter);
    pub const DISCARD: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);
    pub const NEXT_FILE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowDown);
    pub const PREVIOUS_FILE: KeyboardShortcut =
        KeyboardShortcut::new(Modifiers::NONE, Key::ArrowUp);
    pub const CLEAR_LINES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Escape);
    pub const COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::C);
}

/// One entry in the command palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub title: String,
    pub shortcut: Option<KeyboardShortcut>,
    pub action: Action,
}

/// The state of the active repository's working copy, as far as commands depend on it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkContext {
    pub staged: usize,
    pub unstaged: usize,
    /// Where the file selection is, and how many files it holds.
    pub selection: Option<(Area, usize)>,
    /// A file is shown in the diff pane.
    pub has_current: bool,
    /// Which list the shown diff belongs to.
    pub diff_area: Option<Area>,
    /// Hunks of the shown diff that can be staged / unstaged / discarded on their own.
    pub hunks: usize,
    pub selected_lines: usize,
    pub can_commit: bool,
    pub can_amend: bool,
    pub amend: bool,
    /// The latest discard: its label, and whether invoking it redoes rather than undoes.
    pub undo: Option<(String, bool)>,
    pub perf: Option<PerfConfig>,
    /// An image comparison is on screen (and in which mode).
    pub image_mode: Option<ImageMode>,
}

/// What the command list depends on.
pub struct CommandContext<'a> {
    /// Display names of the open tabs, in order.
    pub tabs: &'a [String],
    pub active_tab: Option<usize>,
    /// The active tab has finished loading.
    pub repo_ready: bool,
    /// The active repository's working copy (none for bare repositories).
    pub work: Option<&'a WorkContext>,
    pub recent: &'a [PathBuf],
    pub theme: ThemeMode,
    pub sidebar_visible: bool,
    pub command_log_visible: bool,
    pub command_log_empty: bool,
}

/// Palette entries are listed for at most this many hunks of a diff.
const MAX_HUNK_COMMANDS: usize = 30;

/// All commands available right now, in palette order. Commands that cannot run in the current
/// state are left out rather than shown disabled.
pub fn commands(cx: &CommandContext<'_>) -> Vec<Command> {
    let mut list = Vec::new();
    let mut add = |title: String, shortcut: Option<KeyboardShortcut>, action: Action| {
        list.push(Command {
            title,
            shortcut,
            action,
        });
    };

    add(
        "Open Repository…".into(),
        Some(shortcut::OPEN),
        Action::OpenRepository,
    );

    if let Some(work) = cx.work {
        work_commands(work, &mut add);
    }

    if cx.repo_ready {
        add("Refresh".into(), Some(shortcut::REFRESH), Action::Refresh);
        add(
            "Open in File Explorer".into(),
            Some(shortcut::REVEAL),
            Action::RevealInExplorer,
        );
        add(
            "Copy Repository Path".into(),
            Some(shortcut::COPY_PATH),
            Action::CopyRepositoryPath,
        );
    }

    if cx.active_tab.is_some() {
        add(
            "Close Tab".into(),
            Some(shortcut::CLOSE_TAB),
            Action::CloseActiveTab,
        );
    }
    if cx.tabs.len() > 1 {
        add("Next Tab".into(), Some(shortcut::NEXT_TAB), Action::NextTab);
        add(
            "Previous Tab".into(),
            Some(shortcut::PREVIOUS_TAB),
            Action::PreviousTab,
        );
        for (index, name) in cx.tabs.iter().enumerate() {
            if Some(index) != cx.active_tab {
                add(
                    format!("Switch to Tab: {name}"),
                    None,
                    Action::SwitchTab(index),
                );
            }
        }
    }

    add(
        if cx.sidebar_visible {
            "Hide Sidebar"
        } else {
            "Show Sidebar"
        }
        .into(),
        Some(shortcut::SIDEBAR),
        Action::ToggleSidebar,
    );
    add(
        if cx.command_log_visible {
            "Hide Command Log"
        } else {
            "Show Command Log"
        }
        .into(),
        Some(shortcut::COMMAND_LOG),
        Action::ToggleCommandLog,
    );
    if !cx.command_log_empty {
        add("Clear Command Log".into(), None, Action::ClearCommandLog);
    }

    for mode in ThemeMode::ALL {
        if mode != cx.theme {
            add(
                format!("Theme: {}", mode.label()),
                None,
                Action::SetTheme(mode),
            );
        }
    }

    for path in cx.recent {
        // Name first so it survives truncation; the full path disambiguates same-named repos.
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        add(
            format!("Open Recent: {name}  —  {}", path.display()),
            None,
            Action::OpenPath(path.clone()),
        );
    }

    add("Quit Facet".into(), None, Action::Quit);
    list
}

fn work_commands(
    work: &WorkContext,
    add: &mut impl FnMut(String, Option<KeyboardShortcut>, Action),
) {
    if work.can_commit {
        add(
            if work.amend {
                "Amend Last Commit"
            } else {
                "Commit"
            }
            .into(),
            Some(shortcut::COMMIT),
            Action::Commit,
        );
    }
    add(
        "Write Commit Message".into(),
        Some(shortcut::FOCUS_MESSAGE),
        Action::FocusCommitMessage,
    );
    if work.can_amend {
        add(
            if work.amend {
                "Commit: Stop Amending"
            } else {
                "Commit: Amend Last Commit"
            }
            .into(),
            None,
            Action::ToggleAmend,
        );
    }

    if work.unstaged > 0 {
        add(
            "Stage All Changes".into(),
            Some(shortcut::STAGE_ALL),
            Action::StageAll,
        );
    }
    if work.staged > 0 {
        add(
            "Unstage All Changes".into(),
            Some(shortcut::UNSTAGE_ALL),
            Action::UnstageAll,
        );
    }
    match work.selection {
        Some((Area::Unstaged, count)) if count > 0 => {
            add(
                format!("Stage {}", selected_files(count)),
                Some(shortcut::TOGGLE_STAGE),
                Action::StageSelected,
            );
            add(
                format!("Discard {}…", selected_files(count)),
                Some(shortcut::DISCARD),
                Action::DiscardSelected,
            );
        }
        Some((Area::Staged, count)) if count > 0 => add(
            format!("Unstage {}", selected_files(count)),
            Some(shortcut::TOGGLE_STAGE),
            Action::UnstageSelected,
        ),
        _ => {}
    }
    if work.unstaged > 0 {
        add("Discard All Changes…".into(), None, Action::DiscardAll);
    }
    if let Some((label, redo)) = &work.undo {
        add(
            if *redo {
                format!("Redo Discard: {label}")
            } else {
                format!("Undo Discard: {label}")
            },
            Some(shortcut::UNDO),
            Action::UndoDiscard,
        );
    }

    // Lines and hunks of the diff on screen.
    if work.selected_lines > 0 {
        let lines = if work.selected_lines == 1 {
            "Selected Line".to_owned()
        } else {
            format!("{} Selected Lines", work.selected_lines)
        };
        match work.diff_area {
            Some(Area::Unstaged) => {
                add(
                    format!("Stage {lines}"),
                    Some(shortcut::TOGGLE_STAGE),
                    Action::StageLines,
                );
                add(
                    format!("Discard {lines}…"),
                    Some(shortcut::DISCARD),
                    Action::DiscardLines,
                );
            }
            Some(Area::Staged) => add(
                format!("Unstage {lines}"),
                Some(shortcut::TOGGLE_STAGE),
                Action::UnstageLines,
            ),
            None => {}
        }
        add(
            format!("Copy {lines}"),
            Some(shortcut::COPY),
            Action::CopySelectedLines,
        );
        add(
            "Clear Line Selection".into(),
            Some(shortcut::CLEAR_LINES),
            Action::ClearLineSelection,
        );
    }
    for hunk in 0..work.hunks.min(MAX_HUNK_COMMANDS) {
        let number = hunk + 1;
        match work.diff_area {
            Some(Area::Unstaged) => {
                add(
                    format!("Stage Hunk {number}"),
                    None,
                    Action::StageHunk(hunk),
                );
                add(
                    format!("Discard Hunk {number}…"),
                    None,
                    Action::DiscardHunk(hunk),
                );
            }
            Some(Area::Staged) => add(
                format!("Unstage Hunk {number}"),
                None,
                Action::UnstageHunk(hunk),
            ),
            None => {}
        }
    }

    if work.staged + work.unstaged > 0 {
        add(
            "Select Next File".into(),
            Some(shortcut::NEXT_FILE),
            Action::SelectNextFile,
        );
        add(
            "Select Previous File".into(),
            Some(shortcut::PREVIOUS_FILE),
            Action::SelectPreviousFile,
        );
        add(
            "Select All Files in List".into(),
            Some(shortcut::SELECT_ALL),
            Action::SelectAllFiles,
        );
    }
    if work.has_current {
        add("Copy File Path".into(), None, Action::CopyFilePath);
    }
    if let Some(current) = work.image_mode {
        for mode in ImageMode::ALL {
            if mode != current {
                add(
                    format!("Image Diff: {}", mode.label()),
                    None,
                    Action::SetImageMode(mode),
                );
            }
        }
    }

    match work.perf {
        Some(config) if config.all_enabled() => add(
            "Repository: Disable File System Monitor and Untracked Cache".into(),
            None,
            Action::DisablePerf,
        ),
        Some(_) => add(
            "Repository: Enable File System Monitor and Untracked Cache".into(),
            None,
            Action::EnablePerf,
        ),
        None => {}
    }
}

fn selected_files(count: usize) -> String {
    if count == 1 {
        "Selected File".to_owned()
    } else {
        format!("{count} Selected Files")
    }
}

/// Global shortcuts, most specific first (egui matches shortcuts logically, so `Ctrl+Shift+Tab`
/// must be tested before `Ctrl+Tab`).
pub const GLOBAL_SHORTCUTS: &[(KeyboardShortcut, Action)] = &[
    (shortcut::PREVIOUS_TAB, Action::PreviousTab),
    (shortcut::NEXT_TAB, Action::NextTab),
    (shortcut::REVEAL, Action::RevealInExplorer),
    (shortcut::COPY_PATH, Action::CopyRepositoryPath),
    (shortcut::STAGE_ALL, Action::StageAll),
    (shortcut::UNSTAGE_ALL, Action::UnstageAll),
    (shortcut::COMMIT, Action::Commit),
    (shortcut::FOCUS_MESSAGE, Action::FocusCommitMessage),
    (shortcut::PALETTE, Action::ShowPalette),
    (shortcut::OPEN, Action::OpenRepository),
    (shortcut::CLOSE_TAB, Action::CloseActiveTab),
    (shortcut::REFRESH, Action::Refresh),
    (shortcut::SIDEBAR, Action::ToggleSidebar),
    (shortcut::COMMAND_LOG, Action::ToggleCommandLog),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>(tabs: &'a [String], recent: &'a [PathBuf]) -> CommandContext<'a> {
        CommandContext {
            tabs,
            active_tab: if tabs.is_empty() { None } else { Some(0) },
            repo_ready: !tabs.is_empty(),
            work: None,
            recent,
            theme: ThemeMode::System,
            sidebar_visible: true,
            command_log_visible: false,
            command_log_empty: true,
        }
    }

    /// A working copy with something to do for every command.
    fn busy_work() -> WorkContext {
        WorkContext {
            staged: 2,
            unstaged: 3,
            selection: Some((Area::Unstaged, 2)),
            has_current: true,
            diff_area: Some(Area::Unstaged),
            hunks: 2,
            selected_lines: 3,
            can_commit: true,
            can_amend: true,
            amend: false,
            undo: Some(("notes.txt".into(), false)),
            perf: Some(PerfConfig::default()),
            image_mode: Some(ImageMode::SideBySide),
        }
    }

    fn titles(commands: &[Command]) -> Vec<&str> {
        commands.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn without_a_repository_only_global_commands_are_offered() {
        let list = commands(&context(&[], &[]));
        let titles = titles(&list);
        assert!(titles.contains(&"Open Repository…"));
        assert!(titles.contains(&"Quit Facet"));
        assert!(!titles.contains(&"Refresh"));
        assert!(!titles.contains(&"Close Tab"));
        assert!(!titles.contains(&"Next Tab"));
        assert!(!titles.contains(&"Clear Command Log"));
        assert!(!titles.contains(&"Commit"));
        assert!(!titles.contains(&"Write Commit Message"));
    }

    #[test]
    fn repository_commands_appear_when_a_repo_is_open() {
        let tabs = ["game".to_owned(), "tools".to_owned()];
        let recent = [PathBuf::from(r"D:\old")];
        let list = commands(&context(&tabs, &recent));
        let titles = titles(&list);
        for expected in [
            "Refresh",
            "Open in File Explorer",
            "Copy Repository Path",
            "Close Tab",
            "Next Tab",
            "Previous Tab",
            "Switch to Tab: tools",
            r"Open Recent: old  —  D:\old",
        ] {
            assert!(titles.contains(&expected), "missing {expected}");
        }
        assert!(
            !titles.contains(&"Switch to Tab: game"),
            "active tab is skipped"
        );
    }

    #[test]
    fn toggles_and_theme_reflect_current_state() {
        let mut cx = context(&[], &[]);
        cx.sidebar_visible = false;
        cx.command_log_visible = true;
        cx.command_log_empty = false;
        cx.theme = ThemeMode::Dark;
        let list = commands(&cx);
        let titles = titles(&list);
        assert!(titles.contains(&"Show Sidebar"));
        assert!(titles.contains(&"Hide Command Log"));
        assert!(titles.contains(&"Clear Command Log"));
        assert!(titles.contains(&"Theme: System"));
        assert!(titles.contains(&"Theme: Light"));
        assert!(!titles.contains(&"Theme: Dark"));
    }

    #[test]
    fn working_copy_commands_follow_the_state() {
        let tabs = ["game".to_owned()];
        let work = busy_work();
        let mut cx = context(&tabs, &[]);
        cx.work = Some(&work);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Commit",
            "Write Commit Message",
            "Commit: Amend Last Commit",
            "Stage All Changes",
            "Unstage All Changes",
            "Stage 2 Selected Files",
            "Discard 2 Selected Files…",
            "Discard All Changes…",
            "Undo Discard: notes.txt",
            "Stage 3 Selected Lines",
            "Discard 3 Selected Lines…",
            "Copy 3 Selected Lines",
            "Clear Line Selection",
            "Stage Hunk 1",
            "Discard Hunk 2…",
            "Select Next File",
            "Select All Files in List",
            "Copy File Path",
            "Image Diff: Swipe",
            "Image Diff: Onion Skin",
            "Repository: Enable File System Monitor and Untracked Cache",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.contains(&"Image Diff: Side by Side"));
        assert!(!all.contains(&"Unstage Hunk 1"));

        // The staged side offers the opposite operations and never a discard.
        let staged = WorkContext {
            selection: Some((Area::Staged, 1)),
            diff_area: Some(Area::Staged),
            selected_lines: 1,
            amend: true,
            undo: Some(("notes.txt".into(), true)),
            perf: Some(PerfConfig {
                fsmonitor: true,
                untracked_cache: true,
            }),
            image_mode: None,
            ..busy_work()
        };
        cx.work = Some(&staged);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Amend Last Commit",
            "Commit: Stop Amending",
            "Unstage Selected File",
            "Unstage Selected Line",
            "Unstage Hunk 2",
            "Redo Discard: notes.txt",
            "Repository: Disable File System Monitor and Untracked Cache",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.iter().any(|title| title.starts_with("Discard Hunk")));
        assert!(!all.iter().any(|title| title.starts_with("Image Diff")));

        // A clean working copy offers none of it.
        let clean = WorkContext::default();
        cx.work = Some(&clean);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"Write Commit Message"));
        for absent in [
            "Commit",
            "Stage All Changes",
            "Select Next File",
            "Copy File Path",
        ] {
            assert!(!all.contains(&absent), "unexpected {absent}");
        }
    }

    #[test]
    fn every_global_shortcut_is_reachable_from_the_palette() {
        let tabs = ["a".to_owned(), "b".to_owned()];
        let work = busy_work();
        let mut cx = context(&tabs, &[]);
        cx.work = Some(&work);
        let list = commands(&cx);
        for (shortcut, action) in GLOBAL_SHORTCUTS {
            if *action == Action::ShowPalette {
                continue; // the palette does not list itself
            }
            let entry = list
                .iter()
                .find(|c| c.action == *action)
                .unwrap_or_else(|| panic!("{action:?} has a shortcut but no palette entry"));
            assert_eq!(entry.shortcut, Some(*shortcut), "{action:?}");
        }
    }

    #[test]
    fn shortcuts_are_unique() {
        for (i, (a, _)) in GLOBAL_SHORTCUTS.iter().enumerate() {
            for (b, _) in &GLOBAL_SHORTCUTS[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
