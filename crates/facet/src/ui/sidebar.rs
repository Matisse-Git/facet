//! The sidebar: Changes, History, Branches (the branch flyout) and Stashes, then the
//! worktrees, submodules and LFS locks of the active repository when it has any, and at the
//! bottom when it was last fetched and Settings. Stashes and the other collections expand into
//! their rows; collapsing is remembered in the settings.
//!
//! The rows are one flat list drawn with `show_viewport`, so a repository with thousands of
//! stashes or submodules costs only the visible rows.
//!
//! It paints no background of its own: the Mica backdrop shows through.

use std::path::Path;
use std::time::SystemTime;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Pos2, Rect, ScrollArea, Sense, Ui, Vec2,
};
use facet_git::forge::ForgeKind;
use facet_git::parse::{GitRef, LfsLock, RefKind, StashEntry, Submodule, Worktree};
use facet_git::same_dir;

use super::history::{forge_name, menu_item};
use super::region;
use super::widgets::{self, interaction_fill, text_line};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, GitStatus, TabState};
use crate::app_trees::native_path;
use crate::app_work::ready_info;
use crate::format::{relative_time, unix_seconds};
use crate::history::View;
use crate::theme::{self, icon, radius, space, Palette};

/// A row under a section: a branch, a tag, a stash…
const ROW: f32 = 32.0;
/// A remote's name above its branches, and hint lines.
const SUB_ROW: f32 = 28.0;
/// Space under Settings.
const FOOTER_PAD: f32 = 12.0;
/// Settings, the "Fetched" line above it, and the gap between them and the list.
const FOOTER_HEIGHT: f32 = FOOTER_PAD + NAV_ROW + 8.0 + 16.0 + 8.0;
/// The Changes / History / Settings items, and the section items.
const NAV_ROW: f32 = 36.0;
/// Between navigation items.
const NAV_GAP: f32 = 2.0;
/// A section item plus the gap above it.
const HEADER_ROW: f32 = NAV_ROW + NAV_GAP;
/// Center of the icon column, from the row's left edge.
const ICON_X: f32 = 20.0;
/// Where labels start, from the row's left edge: rows line up under their section's name.
const TEXT_X: f32 = 38.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Stashes,
    Worktrees,
    Submodules,
    Locks,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::Stashes => "Stashes",
            Self::Worktrees => "Worktrees",
            Self::Submodules => "Submodules",
            Self::Locks => "Locks",
        }
    }

    fn glyph(self) -> char {
        match self {
            Self::Stashes => icon::STASH,
            Self::Worktrees => icon::FOLDER,
            Self::Submodules => icon::SUBMODULE,
            Self::Locks => icon::LOCK,
        }
    }

    /// The header button: glyph, tooltip, action.
    fn button(self) -> Option<(char, &'static str, Action)> {
        match self {
            Self::Stashes => Some((icon::ADD, "Stash changes…", Action::Stash)),
            Self::Worktrees => Some((
                icon::ADD,
                "Add worktree…",
                Action::AddWorktree { branch: None },
            )),
            Self::Submodules => Some((
                icon::REFRESH,
                "Update all submodules",
                Action::UpdateSubmodules(None),
            )),
            Self::Locks => Some((icon::REFRESH, "Refresh locks", Action::RefreshLocks)),
        }
    }
}

enum Row<'a> {
    Header(Section, usize, bool),
    Stash(usize, &'a StashEntry),
    /// A worktree, and whether it is the one this tab shows.
    Worktree(&'a Worktree, bool),
    Submodule(&'a Submodule),
    Lock(&'a LfsLock),
    Hint(&'static str),
    Note(String),
}

impl Row<'_> {
    fn height(&self) -> f32 {
        match self {
            Row::Header(..) => HEADER_ROW,
            Row::Hint(_) | Row::Note(_) => SUB_ROW,
            _ => ROW,
        }
    }
}

impl FacetApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let mut actions = Vec::new();

        // Navigation: Changes, History and Branches of a working copy.
        let mut top = rect.top() + space::XS;
        let settings_open = self.settings_view.is_some();
        let work = self.active_tab().and_then(|tab| {
            ready_info(tab).map(|_| {
                let changed = tab.work.staged.len() + tab.work.unstaged.len();
                (tab.view, changed)
            })
        });
        if let Some((view, changed)) = work {
            for (target, glyph, label, tooltip, key, action) in [
                (
                    Some(View::Changes),
                    icon::CHANGES,
                    "Changes",
                    "Show the working copy",
                    Some(&shortcut::SHOW_CHANGES),
                    Action::ShowChanges,
                ),
                (
                    Some(View::History),
                    icon::HISTORY,
                    "History",
                    "Show the commit history",
                    Some(&shortcut::SHOW_HISTORY),
                    Action::ShowHistory,
                ),
                (
                    None,
                    icon::BRANCH,
                    "Branches",
                    "Switch, create or merge branches",
                    None,
                    Action::ShowBranches,
                ),
            ] {
                let row = Rect::from_min_size(
                    Pos2::new(rect.left() + space::S, top),
                    Vec2::new(rect.width() - space::S * 2.0, NAV_ROW),
                );
                let active = target == Some(view) && !settings_open;
                let count = (target == Some(View::Changes) && changed > 0).then(|| Count {
                    text: changed.to_string(),
                    pill: active,
                });
                let tooltip = match key {
                    Some(key) => widgets::tooltip_with_shortcut(ui, tooltip, key),
                    None => tooltip.to_owned(),
                };
                if nav_item(ui, row, glyph, label, count.as_ref(), active, palette)
                    .on_hover_text(tooltip)
                    .clicked()
                {
                    actions.push(action);
                }
                top = row.bottom() + NAV_GAP;
            }
        }

        let list = Rect::from_min_max(
            Pos2::new(rect.left() + space::S, top - NAV_GAP),
            Pos2::new(rect.right() - space::S, rect.bottom() - FOOTER_HEIGHT),
        );
        let rows = self.sidebar_rows();
        // The stash shown in the stash view is highlighted like the current page.
        let shown_stash = self
            .active_tab()
            .filter(|tab| tab.view == View::Stash && !settings_open)
            .and_then(|tab| tab.stash.entry.as_ref())
            .map(|entry| entry.oid.clone());

        region(ui, "sidebar-list", list, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ScrollArea::vertical()
                .id_salt("sidebar-scroll")
                .auto_shrink(false)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    let width = ui.max_rect().width();
                    let total: f32 = rows.iter().map(Row::height).sum();
                    ui.set_min_height(total);
                    let mut y = 0.0;
                    for (index, row) in rows.iter().enumerate() {
                        let height = row.height();
                        if y + height >= viewport.min.y && y <= viewport.max.y {
                            let rect = Rect::from_min_size(
                                origin + Vec2::new(0.0, y),
                                Vec2::new(width, height),
                            );
                            draw_row(
                                ui,
                                rect,
                                index,
                                row,
                                shown_stash.as_deref(),
                                palette,
                                &mut actions,
                            );
                        }
                        y += height;
                    }
                });
        });

        // Footer: when the repository was last fetched (or that git is missing), and Settings.
        let row = Rect::from_min_size(
            Pos2::new(rect.left() + space::S, rect.bottom() - FOOTER_PAD - NAV_ROW),
            Vec2::new(rect.width() - space::S * 2.0, NAV_ROW),
        );
        let note = Pos2::new(row.left() + 10.0, row.top() - 8.0);
        match &self.git_status {
            GitStatus::Missing(detail) => {
                let text_rect = ui.painter().text(
                    note,
                    Align2::LEFT_BOTTOM,
                    "Git not found",
                    theme::small_strong(),
                    palette.danger,
                );
                ui.interact(text_rect, Id::unique("git-missing"), Sense::hover())
                    .on_hover_text(detail);
            }
            GitStatus::Checking | GitStatus::Found(_) => {
                let fetched = self
                    .active_tab()
                    .filter(|_| work.is_some())
                    .and_then(|tab| tab.refs.last_fetch);
                if let Some(fetched) = fetched {
                    let now = unix_seconds(SystemTime::now());
                    let text = format!("Fetched {}", relative_time(now, unix_seconds(fetched)));
                    let text_rect = ui.painter().text(
                        note,
                        Align2::LEFT_BOTTOM,
                        text,
                        theme::small(),
                        palette.text_tertiary,
                    );
                    if let GitStatus::Found(version) = &self.git_status {
                        ui.interact(text_rect, Id::unique("sidebar-fetched"), Sense::hover())
                            .on_hover_text(format!("Git {version}"));
                    }
                }
            }
        }
        let tooltip = widgets::tooltip_with_shortcut(
            ui,
            if settings_open {
                "Close Settings"
            } else {
                "Settings"
            },
            &shortcut::SETTINGS,
        );
        if nav_item(
            ui,
            row,
            icon::SETTINGS,
            "Settings",
            None,
            settings_open,
            palette,
        )
        .on_hover_text(tooltip)
        .clicked()
        {
            actions.push(if settings_open {
                Action::CloseSettings
            } else {
                Action::ShowSettings
            });
        }
        self.pending.extend(actions);
    }

    /// The flat row list of the repository sections (for a working copy).
    fn sidebar_rows(&self) -> Vec<Row<'_>> {
        let collapsed = |section: Section| {
            self.settings
                .collapsed_sections
                .iter()
                .any(|s| s == section.title())
        };
        let mut rows = Vec::new();
        let repo = self
            .active_tab()
            .filter(|tab| matches!(&tab.state, TabState::Ready { info, .. } if !info.is_bare()));
        if let Some(tab) = repo.filter(|tab| tab.refs.loaded) {
            let snapshot = &tab.refs.snapshot;

            rows.push(Row::Header(
                Section::Stashes,
                snapshot.stashes.len(),
                collapsed(Section::Stashes),
            ));
            if !collapsed(Section::Stashes) {
                rows.extend(
                    snapshot
                        .stashes
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| Row::Stash(index, entry)),
                );
            }

            // Only this working folder: nothing to show.
            let worktrees: Vec<&Worktree> = snapshot.worktrees.iter().filter(|w| !w.bare).collect();
            if worktrees.len() > 1 {
                rows.push(Row::Header(
                    Section::Worktrees,
                    worktrees.len(),
                    collapsed(Section::Worktrees),
                ));
                if !collapsed(Section::Worktrees) {
                    rows.extend(worktrees.into_iter().map(|worktree| {
                        Row::Worktree(worktree, same_dir(Path::new(&worktree.path), &tab.path))
                    }));
                }
            }

            if !snapshot.submodules.is_empty() {
                rows.push(Row::Header(
                    Section::Submodules,
                    snapshot.submodules.len(),
                    collapsed(Section::Submodules),
                ));
                if !collapsed(Section::Submodules) {
                    rows.extend(snapshot.submodules.iter().map(Row::Submodule));
                }
            }

            let lfs = &tab.lfs;
            if lfs.usage.lfs {
                rows.push(Row::Header(
                    Section::Locks,
                    lfs.locks.len(),
                    collapsed(Section::Locks),
                ));
                if !collapsed(Section::Locks) {
                    if lfs.locks_running && !lfs.locks_loaded {
                        rows.push(Row::Hint("Asking the LFS server…"));
                    } else if let Some(error) = &lfs.locks_error {
                        rows.push(Row::Note(error.clone()));
                    } else if !lfs.locks_loaded {
                        rows.push(Row::Hint("Refresh to see who has files locked"));
                    } else if lfs.locks.is_empty() {
                        rows.push(Row::Hint("No files are locked"));
                    }
                    rows.extend(lfs.locks.iter().map(Row::Lock));
                }
            }
        }
        rows
    }
}

/// The count on the right of a navigation item: plain, or an accent pill (the changes of
/// the page shown).
struct Count {
    text: String,
    pill: bool,
}

/// A navigation item: icon and label, an accent bar and a soft fill while it is the page
/// shown, and an optional count on the right.
fn nav_item(
    ui: &Ui,
    rect: Rect,
    glyph: char,
    label: &str,
    count: Option<&Count>,
    active: bool,
    palette: &Palette,
) -> egui::Response {
    let response = ui.interact(rect, Id::unique(("sidebar-nav", label)), Sense::click());
    let fill = if active {
        if response.is_pointer_button_down_on() {
            palette.pressed
        } else {
            palette.hover
        }
    } else {
        interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);
    if active {
        let bar = Rect::from_center_size(
            Pos2::new(rect.left() + 1.5, rect.center().y),
            Vec2::new(3.0, 16.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), palette.accent);
    }
    widgets::focus_ring(ui, &response, rect, radius::M, palette);
    ui.painter().text(
        Pos2::new(rect.left() + 20.0, rect.center().y),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(15.0),
        palette.text,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 38.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if active {
            theme::strong()
        } else {
            theme::body()
        },
        palette.text,
    );
    match count {
        Some(Count { text, pill: true }) => {
            let galley = ui.painter().layout_no_wrap(
                text.clone(),
                theme::caption_strong(),
                palette.on_accent,
            );
            let width = (galley.size().x + 12.0).max(20.0);
            let pill = Rect::from_center_size(
                Pos2::new(rect.right() - 10.0 - width / 2.0, rect.center().y),
                Vec2::new(width, 18.0),
            );
            ui.painter()
                .rect_filled(pill, CornerRadius::same(9), palette.accent);
            ui.painter()
                .galley(pill.center() - galley.size() / 2.0, galley, palette.on_accent);
        }
        Some(Count { text, pill: false }) => {
            ui.painter().text(
                Pos2::new(rect.right() - 10.0, rect.center().y),
                Align2::RIGHT_CENTER,
                text,
                theme::small(),
                palette.text_tertiary,
            );
        }
        None => {}
    }
    response
}

fn row_base(
    ui: &Ui,
    rect: Rect,
    id: Id,
    active: bool,
    palette: &Palette,
) -> eframe::egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let inner = rect.shrink2(Vec2::new(0.0, 1.0));
    let fill = if active {
        palette.accent_soft
    } else {
        interaction_fill(ui, &response, palette)
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(inner, CornerRadius::same(radius::M), fill);
    }
    widgets::focus_ring(ui, &response, inner, radius::M, palette);
    response
}

/// Small text right-aligned at `right`; returns where the row's main text has to end.
fn right_text(ui: &Ui, right: f32, center: f32, text: &str, color: Color32) -> f32 {
    if text.is_empty() {
        return right;
    }
    let galley = widgets::elided(ui, text, theme::small(), color, 120.0);
    let left = right - galley.size().x;
    ui.painter().galley(
        Pos2::new(left, center - galley.size().y / 2.0),
        galley,
        color,
    );
    left - space::S
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn draw_row(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    row: &Row<'_>,
    shown_stash: Option<&str>,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let center = rect.center().y;
    let text_left = rect.left() + TEXT_X;
    let icon_x = rect.left() + ICON_X;
    let mut right = rect.right() - space::S - 2.0;
    match row {
        Row::Header(section, count, collapsed) => {
            // Drawn like Changes / History: icon, name, then count and chevron on the right.
            let item =
                Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - NAV_ROW), rect.max);
            let center = item.center().y;
            let response = ui.interact(item, Id::unique(("sidebar-header", index)), Sense::click());
            let hovered = ui.rect_contains_pointer(item);
            ui.painter().rect_filled(
                item,
                CornerRadius::same(radius::M),
                interaction_fill(ui, &response, palette),
            );
            widgets::focus_ring(ui, &response, item, radius::M, palette);
            ui.painter().text(
                Pos2::new(icon_x, center),
                Align2::CENTER_CENTER,
                section.glyph(),
                theme::icon_font(15.0),
                palette.text,
            );
            // The count on the right; while hovered, the chevron that opens or closes the
            // list (an empty section has nothing to open) and the section's button.
            if *count > 0 && hovered {
                ui.painter().text(
                    Pos2::new(item.right() - 14.0, center),
                    Align2::CENTER_CENTER,
                    if *collapsed {
                        icon::CHEVRON_RIGHT
                    } else {
                        icon::CHEVRON_DOWN
                    },
                    theme::icon_font(9.0),
                    palette.text_tertiary,
                );
            }
            right = if hovered {
                item.right() - 28.0
            } else {
                item.right() - 10.0
            };
            // "+" for sections that create something, ⟳ for those that refresh: on hover.
            let mut add_clicked = false;
            match section.button().filter(|_| hovered) {
                Some((glyph, tooltip, action)) => {
                    let button =
                        Rect::from_center_size(Pos2::new(right - 12.0, center), Vec2::splat(26.0));
                    if widgets::icon_button_at(
                        ui,
                        button,
                        Id::unique(("sidebar-add", index)),
                        glyph,
                        tooltip,
                    )
                    .clicked()
                    {
                        actions.push(action);
                        add_clicked = true;
                    }
                    right = button.left() - space::XS;
                }
                None if *count > 0 => {
                    let painted = ui.painter().text(
                        Pos2::new(right, center),
                        Align2::RIGHT_CENTER,
                        count.to_string(),
                        theme::small(),
                        palette.text_tertiary,
                    );
                    right = painted.left() - space::S;
                }
                None => {}
            }
            text_line(
                ui,
                Pos2::new(text_left, center),
                section.title(),
                theme::body(),
                palette.text,
                right - text_left,
            );
            if response.clicked() && !add_clicked {
                actions.push(Action::ToggleSidebarSection(section.title().to_owned()));
            }
            response.on_hover_text(if *collapsed {
                format!("Show {}", section.title().to_lowercase())
            } else {
                format!("Hide {}", section.title().to_lowercase())
            });
        }
        Row::Hint(text) => {
            text_line(
                ui,
                Pos2::new(text_left, center),
                text,
                theme::small(),
                palette.text_tertiary,
                right - text_left,
            );
        }
        Row::Stash(position, entry) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-stash", index)),
                shown_stash == Some(entry.oid.as_str()),
                palette,
            );
            if response.clicked() {
                actions.push(Action::ShowStash(*position));
            }
            // Apply / pop on hover.
            let hovered = ui.rect_contains_pointer(rect);
            if hovered {
                for (label, tooltip, action) in [
                    (
                        "Pop",
                        "Apply and drop this stash",
                        Action::PopStash(*position),
                    ),
                    (
                        "Apply",
                        "Apply this stash and keep it",
                        Action::ApplyStash(*position),
                    ),
                ] {
                    let width = widgets::text_button_width(ui, label);
                    let button = Rect::from_min_max(
                        Pos2::new(right - width, center - 11.0),
                        Pos2::new(right, center + 11.0),
                    );
                    right -= width + 2.0;
                    if widgets::text_button_at(
                        ui,
                        button,
                        Id::unique(("stash-button", index, label)),
                        label,
                        tooltip,
                        palette.text,
                        false,
                    )
                    .clicked()
                    {
                        actions.push(action);
                    }
                }
            }
            let message = entry
                .message
                .split_once(": ")
                .map_or(entry.message.as_str(), |(_, rest)| rest);
            text_line(
                ui,
                Pos2::new(text_left, center),
                message,
                theme::body(),
                palette.text,
                (right - text_left - space::XS).max(0.0),
            );
            let response = response.on_hover_text(format!(
                "{}\n{}{}",
                entry.selector,
                entry.message,
                if entry.untracked {
                    "\nIncludes untracked files"
                } else {
                    ""
                }
            ));
            response.context_menu(|ui| {
                menu_item(ui, "Apply", Action::ApplyStash(*position), actions);
                menu_item(
                    ui,
                    "Pop (Apply and Drop)",
                    Action::PopStash(*position),
                    actions,
                );
                ui.separator();
                menu_item(ui, "Drop…", Action::DropStash(*position), actions);
            });
        }
        Row::Note(text) => {
            let painted = text_line(
                ui,
                Pos2::new(text_left, center),
                text,
                theme::small(),
                palette.warning,
                right - text_left,
            );
            let hit =
                Rect::from_min_size(Pos2::new(text_left, rect.top()), Vec2::new(painted, ROW));
            ui.interact(hit, Id::unique(("sidebar-note", index)), Sense::hover())
                .on_hover_text(text.as_str());
        }
        Row::Worktree(worktree, current) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-worktree", index)),
                false,
                palette,
            );
            let path = native_path(&worktree.path);
            if *current {
                ui.painter().text(
                    Pos2::new(icon_x, center),
                    Align2::CENTER_CENTER,
                    icon::CHECK,
                    theme::icon_font(12.0),
                    palette.accent,
                );
            }
            let (detail, detail_color) = if worktree.prunable.is_some() {
                ("folder gone".to_owned(), palette.warning)
            } else if let Some(branch) = worktree.branch_name() {
                (branch.to_owned(), palette.text_secondary)
            } else {
                ("detached".to_owned(), palette.text_tertiary)
            };
            right = right_text(ui, right, center, &detail, detail_color);
            text_line(
                ui,
                Pos2::new(text_left, center),
                worktree.name(),
                if *current {
                    theme::strong()
                } else {
                    theme::body()
                },
                palette.text,
                right - text_left,
            );
            let openable = !*current && worktree.prunable.is_none();
            if response.clicked() && openable {
                actions.push(Action::OpenPath(path.clone()));
            }
            let mut hover = path.display().to_string();
            if worktree.main {
                hover.push_str("\nThe main worktree");
            }
            if let Some(reason) = &worktree.locked {
                hover.push_str("\nLocked against pruning");
                if !reason.is_empty() {
                    hover.push_str(&format!(": {reason}"));
                }
            }
            if openable {
                hover.push_str("\nClick to open it in a tab");
            }
            let response = response.on_hover_text(hover);
            response.context_menu(|ui| {
                if openable {
                    menu_item(ui, "Open in Tab", Action::OpenPath(path.clone()), actions);
                }
                menu_item(
                    ui,
                    "Copy Folder Path",
                    Action::CopyText {
                        text: path.display().to_string(),
                        what: "folder path",
                    },
                    actions,
                );
                if worktree.prunable.is_some() {
                    ui.separator();
                    menu_item(
                        ui,
                        "Forget Worktrees Whose Folders Are Gone",
                        Action::PruneWorktrees,
                        actions,
                    );
                } else if !worktree.main && !*current {
                    ui.separator();
                    menu_item(
                        ui,
                        "Remove Worktree…",
                        Action::RemoveWorktree(path.clone()),
                        actions,
                    );
                }
            });
        }
        Row::Submodule(module) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-submodule", index)),
                false,
                palette,
            );
            let (detail, color) = match (&module.recorded, module.populated) {
                (_, false) => ("not checked out".to_owned(), palette.warning),
                (Some(oid), true) => (oid.chars().take(7).collect(), palette.text_tertiary),
                (None, true) => (String::new(), palette.text_tertiary),
            };
            right = right_text(ui, right, center, &detail, color);
            text_line(
                ui,
                Pos2::new(text_left, center),
                module.short_name(),
                theme::body(),
                palette.text,
                right - text_left,
            );
            if response.clicked() {
                actions.push(if module.populated {
                    Action::OpenSubmodule(module.path.clone())
                } else {
                    Action::UpdateSubmodules(Some(module.path.clone()))
                });
            }
            let mut hover = module.path.clone();
            if let Some(url) = &module.url {
                hover.push_str(&format!("\n{url}"));
            }
            hover.push_str(if module.populated {
                "\nClick to open it in a tab"
            } else {
                "\nClick to check it out (update)"
            });
            let response = response.on_hover_text(hover);
            response.context_menu(|ui| {
                if module.populated {
                    menu_item(
                        ui,
                        "Open in Tab",
                        Action::OpenSubmodule(module.path.clone()),
                        actions,
                    );
                }
                menu_item(
                    ui,
                    "Update (Check Out Recorded Commit)",
                    Action::UpdateSubmodules(Some(module.path.clone())),
                    actions,
                );
                menu_item(
                    ui,
                    "Sync URL from .gitmodules",
                    Action::SyncSubmodules(Some(module.path.clone())),
                    actions,
                );
                if let Some(url) = &module.url {
                    ui.separator();
                    menu_item(
                        ui,
                        "Copy URL",
                        Action::CopyText {
                            text: url.clone(),
                            what: "submodule URL",
                        },
                        actions,
                    );
                }
            });
        }
        Row::Lock(lock) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-lock", index)),
                false,
                palette,
            );
            let ours = lock.ours == Some(true);
            ui.painter().text(
                Pos2::new(icon_x, center),
                Align2::CENTER_CENTER,
                icon::LOCK,
                theme::icon_font(12.0),
                if ours {
                    palette.accent
                } else {
                    palette.warning
                },
            );
            let unlock = if lock.ours == Some(false) {
                Action::ForceUnlock {
                    id: lock.id.clone(),
                    path: lock.path.clone(),
                    owner: lock.owner.clone(),
                }
            } else {
                Action::UnlockFile {
                    id: lock.id.clone(),
                    path: lock.path.clone(),
                }
            };
            if ui.rect_contains_pointer(rect) {
                let button =
                    Rect::from_center_size(Pos2::new(right - 12.0, center), Vec2::splat(24.0));
                right = button.left() - space::XS;
                let tooltip = if lock.ours == Some(false) {
                    "Force unlock…"
                } else {
                    "Unlock"
                };
                if widgets::icon_button_at(
                    ui,
                    button,
                    Id::unique(("lock-button", index)),
                    icon::UNLOCK,
                    tooltip,
                )
                .clicked()
                {
                    actions.push(unlock.clone());
                }
            } else {
                let owner = if ours { "you" } else { lock.owner.as_str() };
                right = right_text(ui, right, center, owner, palette.text_tertiary);
            }
            let name = lock.path.rsplit('/').next().unwrap_or(&lock.path);
            text_line(
                ui,
                Pos2::new(text_left, center),
                name,
                theme::body(),
                palette.text,
                right - text_left,
            );
            let owner = match lock.ours {
                Some(true) => "you".to_owned(),
                _ if lock.owner.is_empty() => "someone".to_owned(),
                _ => lock.owner.clone(),
            };
            let response = response.on_hover_text(format!(
                "{}\nLocked by {owner}{}",
                lock.path,
                if lock.locked_at.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", lock.locked_at)
                }
            ));
            response.context_menu(|ui| {
                let label = if lock.ours == Some(false) {
                    "Force Unlock…"
                } else {
                    "Unlock"
                };
                menu_item(ui, label, unlock.clone(), actions);
                if lock.ours.is_none() {
                    menu_item(
                        ui,
                        "Force Unlock…",
                        Action::ForceUnlock {
                            id: lock.id.clone(),
                            path: lock.path.clone(),
                            owner: lock.owner.clone(),
                        },
                        actions,
                    );
                }
                ui.separator();
                menu_item(
                    ui,
                    "Copy File Path",
                    Action::CopyText {
                        text: lock.path.clone(),
                        what: "file path",
                    },
                    actions,
                );
            });
        }
    }
}

/// The menu of a local branch: the rows of the branch flyout.
pub(super) fn branch_menu(
    ui: &mut Ui,
    branch: &GitRef,
    forge: Option<ForgeKind>,
    actions: &mut Vec<Action>,
) {
    if !branch.head {
        menu_item(
            ui,
            "Switch to Branch",
            Action::SwitchBranch(branch.name.clone()),
            actions,
        );
        menu_item(
            ui,
            "Merge into Current Branch",
            Action::MergeIntoCurrent(branch.name.clone()),
            actions,
        );
    }
    menu_item(
        ui,
        "Show in History",
        Action::RevealCommit(branch.oid.clone()),
        actions,
    );
    menu_item(
        ui,
        "New Branch from Here…",
        Action::NewBranch {
            at: Some(branch.oid.clone()),
        },
        actions,
    );
    ui.separator();
    menu_item(
        ui,
        "Rename…",
        Action::RenameBranch(branch.name.clone()),
        actions,
    );
    if !branch.head {
        menu_item(
            ui,
            "Delete…",
            Action::DeleteBranch(branch.name.clone()),
            actions,
        );
    }
    ui.separator();
    menu_item(
        ui,
        "Copy Branch Name",
        Action::CopyText {
            text: branch.name.clone(),
            what: "branch name",
        },
        actions,
    );
    if let Some(kind) = forge {
        if branch.upstream.is_some() {
            menu_item(
                ui,
                &format!("Open on {}", forge_name(kind)),
                Action::OpenBranchOnForge(branch.name.clone()),
                actions,
            );
            let request = match kind {
                ForgeKind::GitLab => "Create Merge Request",
                _ => "Create Pull Request",
            };
            menu_item(
                ui,
                request,
                Action::CreatePullRequest(branch.name.clone()),
                actions,
            );
        }
    }
}

/// The menu of a remote-tracking branch: the flyout's remote rows.
pub(super) fn remote_branch_menu(
    ui: &mut Ui,
    branch: &GitRef,
    forge: Option<ForgeKind>,
    actions: &mut Vec<Action>,
) {
    menu_item(
        ui,
        "Check Out",
        Action::CheckoutRemoteBranch(branch.name.clone()),
        actions,
    );
    menu_item(
        ui,
        "Merge into Current Branch",
        Action::MergeIntoCurrent(branch.name.clone()),
        actions,
    );
    menu_item(
        ui,
        "Show in History",
        Action::RevealCommit(branch.oid.clone()),
        actions,
    );
    menu_item(
        ui,
        "New Branch from Here…",
        Action::NewBranch {
            at: Some(branch.oid.clone()),
        },
        actions,
    );
    ui.separator();
    menu_item(
        ui,
        "Copy Branch Name",
        Action::CopyText {
            text: branch.name.clone(),
            what: "branch name",
        },
        actions,
    );
    if let (Some(kind), RefKind::Remote { .. }) = (forge, &branch.kind) {
        menu_item(
            ui,
            &format!("Open on {}", forge_name(kind)),
            Action::OpenBranchOnForge(branch.short_name().to_owned()),
            actions,
        );
    }
}
