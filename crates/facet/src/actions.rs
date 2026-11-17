//! Every user-invokable action in Facet.
//!
//! Buttons, keyboard shortcuts and the command palette all go through [`Action`], so the
//! palette reaches everything by construction: a feature is added by adding a variant here,
//! listing it in [`commands`], and handling it in `FacetApp::run`.

use std::path::PathBuf;

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

use crate::settings::ThemeMode;

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
    CopyText { text: String, what: &'static str },
    SetTheme(ThemeMode),
    Quit,
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
}

/// One entry in the command palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub title: String,
    pub shortcut: Option<KeyboardShortcut>,
    pub action: Action,
}

/// What the command list depends on.
pub struct CommandContext<'a> {
    /// Display names of the open tabs, in order.
    pub tabs: &'a [String],
    pub active_tab: Option<usize>,
    /// The active tab has finished loading.
    pub repo_ready: bool,
    pub recent: &'a [PathBuf],
    pub theme: ThemeMode,
    pub sidebar_visible: bool,
    pub command_log_visible: bool,
    pub command_log_empty: bool,
}

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

/// Global shortcuts, most specific first (egui matches shortcuts logically, so `Ctrl+Shift+Tab`
/// must be tested before `Ctrl+Tab`).
pub const GLOBAL_SHORTCUTS: &[(KeyboardShortcut, Action)] = &[
    (shortcut::PREVIOUS_TAB, Action::PreviousTab),
    (shortcut::NEXT_TAB, Action::NextTab),
    (shortcut::REVEAL, Action::RevealInExplorer),
    (shortcut::COPY_PATH, Action::CopyRepositoryPath),
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
            recent,
            theme: ThemeMode::System,
            sidebar_visible: true,
            command_log_visible: false,
            command_log_empty: true,
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
    fn every_global_shortcut_is_reachable_from_the_palette() {
        let tabs = ["a".to_owned(), "b".to_owned()];
        let list = commands(&context(&tabs, &[]));
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
