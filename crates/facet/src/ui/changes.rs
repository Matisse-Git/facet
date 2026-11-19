//! The working copy view: a compact repository header, the list of changed files with the
//! commit box below it, and the diff of the current file beside it (`diff.rs`).

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId,
    Galley, Id, Layout, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit,
    TextFormat, Ui, Vec2,
};
use facet_git::parse::{Change, EntryKind, StatusEntry};

use super::power::BANNER;
use super::widgets::{
    self, icon_button_at, skeleton, text_button_at, text_button_width, text_line, Check,
};
use super::{region, splitter_feedback, SPLITTER_GRAB};
use crate::actions::{shortcut, Action};
use crate::app::FacetApp;
use crate::app_trees::LfsState;
use crate::history::View;
use crate::settings::{Settings, CHANGES_WIDTH, COMMIT_BOX_HEIGHT};
use crate::theme::{self, icon, radius, space, Palette};
use crate::workspace::{Area, Included, Workspace};

const MIN_DIFF_WIDTH: f32 = 320.0;
const MIN_LIST_HEIGHT: f32 = 96.0;

impl FacetApp {
    pub(super) fn work_view(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let mut actions = Vec::new();

        // Branch, sync and the Changes / History switch live in the title bar and sidebar.
        let mut body = rect;
        if self
            .active_tab()
            .is_some_and(|tab| tab.power.operation.is_some())
        {
            let banner = Rect::from_min_size(body.min, Vec2::new(body.width(), BANNER));
            region(ui, "operation-banner", banner, |ui| {
                self.operation_banner(ui, banner, &mut actions);
            });
            widgets::hairline_h(ui, rect.x_range().into(), banner.bottom());
            body.min.y = banner.bottom() + 1.0;
        }
        if self
            .active_tab()
            .is_some_and(|tab| tab.power.blame.is_some())
        {
            region(ui, "blame", body, |ui| {
                self.blame_view(ui, body, &mut actions)
            });
            self.pending.extend(actions);
            return;
        }
        match self.active_tab().map(|tab| tab.view) {
            Some(View::History) => {
                self.history_view(ui, body, &mut actions);
                self.pending.extend(actions);
                return;
            }
            Some(View::Stash) => {
                self.stash_view(ui, body, &mut actions);
                self.pending.extend(actions);
                return;
            }
            _ => {}
        }
        // A clean working tree: the whole view says so (unless an operation is under way).
        if let Some(work) = self.active_work().filter(|work| {
            work.loaded && work.row_count() == 0 && work.commit.error.is_none()
        }) {
            if self
                .active_tab()
                .is_some_and(|tab| tab.power.operation.is_none())
            {
                ui.painter().rect_filled(body, 0.0, palette.surface);
                region(ui, "clean", body, |ui| {
                    super::diff::clean_state(ui, body, work, &mut actions);
                });
                self.pending.extend(actions);
                return;
            }
        }
        let width = self
            .settings
            .changes_width
            .min((body.width() - MIN_DIFF_WIDTH).max(*CHANGES_WIDTH.start()))
            .round();
        let left = Rect::from_min_max(body.min, Pos2::new(body.left() + width, body.bottom()));
        let right = Rect::from_min_max(Pos2::new(left.right() + 1.0, body.top()), body.max);

        region(ui, "changes", left, |ui| {
            self.changes_panel(ui, left, &mut actions);
        });
        ui.painter().vline(
            left.right() + 0.5,
            body.y_range(),
            palette.hairline_stroke(),
        );
        region(ui, "diff", right, |ui| {
            self.diff_pane(ui, right, &mut actions)
        });

        // Splitter between the file list and the diff.
        let grab = Rect::from_min_max(
            Pos2::new(left.right() - SPLITTER_GRAB / 2.0, body.top()),
            Pos2::new(left.right() + SPLITTER_GRAB / 2.0, body.bottom()),
        );
        let response = ui.interact(
            grab,
            Id::unique("changes-splitter"),
            Sense::click_and_drag(),
        );
        if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            self.settings.changes_width = (pointer.x - body.left())
                .clamp(*CHANGES_WIDTH.start(), *CHANGES_WIDTH.end())
                .round();
        }
        if response.double_clicked() {
            self.settings.changes_width = Settings::default().changes_width;
        }
        splitter_feedback(ui, &response, CursorIcon::ResizeHorizontal, |stroke| {
            ui.painter().vline(left.right(), body.y_range(), stroke);
        });

        self.pending.extend(actions);
    }

    /// File list, the optional performance offer, and the commit box.
    fn changes_panel(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let can_commit = self.can_commit();
        let declined = self
            .active_tab()
            .is_some_and(|tab| self.perf_offer_declined(&tab.path));
        let commit_height = self
            .settings
            .commit_box_height
            .min((rect.height() - MIN_LIST_HEIGHT).max(*COMMIT_BOX_HEIGHT.start()))
            .round();
        let Some(index) = self.active.filter(|_| self.active_work().is_some()) else {
            return;
        };
        let tab = &mut self.tabs[index];
        let lfs = &tab.lfs;
        let work = &mut tab.work;

        let offer = (work.perf_offer() && !declined)
            .then(|| offer_text(ui, rect.width() - space::M * 2.0, work.perf.last_ms));
        let offer_height = offer.as_ref().map_or(0.0, |galley| {
            space::S + 18.0 + space::XS + galley.size().y + space::S + 24.0 + space::S
        });
        let commit = Rect::from_min_max(
            Pos2::new(rect.left(), rect.bottom() - commit_height),
            rect.max,
        );
        let offer_rect = Rect::from_min_max(
            Pos2::new(rect.left(), commit.top() - offer_height),
            Pos2::new(rect.right(), commit.top()),
        );
        let list = Rect::from_min_max(rect.min, Pos2::new(rect.right(), offer_rect.top()));

        if work.loaded {
            file_list(ui, list, work, lfs, actions);
        } else {
            list_skeleton(ui, list);
        }
        if let Some(galley) = offer {
            perf_offer(ui, offer_rect, galley, actions);
        }
        widgets::hairline_h(ui, rect.x_range().into(), commit.top());
        commit_box(ui, commit, work, can_commit, actions);

        // Splitter between the list and the commit box.
        let grab = Rect::from_min_max(
            Pos2::new(rect.left(), commit.top() - SPLITTER_GRAB / 2.0),
            Pos2::new(rect.right(), commit.top() + SPLITTER_GRAB / 2.0),
        );
        let response = ui.interact(grab, Id::unique("commit-splitter"), Sense::click_and_drag());
        if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            self.settings.commit_box_height = (rect.bottom() - pointer.y)
                .clamp(*COMMIT_BOX_HEIGHT.start(), *COMMIT_BOX_HEIGHT.end())
                .round();
        }
        if response.double_clicked() {
            self.settings.commit_box_height = Settings::default().commit_box_height;
        }
        splitter_feedback(ui, &response, CursorIcon::ResizeVertical, |stroke| {
            ui.painter().hline(rect.x_range(), commit.top(), stroke);
        });
    }
}

/// Height of the bar above the file list ("4 of 5 files").
const LIST_HEADER: f32 = 48.0;
/// Space around the rows.
const LIST_PAD: f32 = 6.0;

/// A file row (with the 1 pt gap below it).
fn file_row_height() -> f32 {
    theme::dense(40.0, 32.0) + 1.0
}

/// The bar above the list, then one row per changed file: its checkbox stages it.
fn file_list(
    ui: &mut Ui,
    rect: Rect,
    work: &mut Workspace,
    lfs: &LfsState,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), LIST_HEADER));
    list_header(ui, header, work, actions);
    widgets::hairline_h(ui, rect.x_range().into(), header.bottom() - 0.5);
    let rect = Rect::from_min_max(Pos2::new(rect.left(), header.bottom()), rect.max);
    if work.row_count() == 0 {
        let center = rect.center();
        ui.painter().text(
            center - Vec2::new(0.0, 10.0),
            Align2::CENTER_CENTER,
            "No changes",
            theme::strong(),
            palette.text_secondary,
        );
        ui.painter().text(
            center + Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            "The working tree matches the last commit",
            theme::small(),
            palette.text_tertiary,
        );
        return;
    }
    let reveal = if std::mem::take(&mut work.reveal_cursor) {
        work.selection
            .cursor
            .and_then(|cursor| work.row_of(work.selection.area, cursor))
    } else {
        None
    };
    let rows = work.row_count();
    let row_height = file_row_height();
    let work = &*work;
    let list = Rect::from_min_max(
        Pos2::new(rect.left() + LIST_PAD, rect.top() + LIST_PAD),
        Pos2::new(rect.right() - LIST_PAD, rect.bottom()),
    );
    region(ui, "file-list", list, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt("file-list-scroll")
            .auto_shrink(false)
            .show_rows(ui, row_height, rows, |ui, range| {
                let left = ui.max_rect().left();
                let width = ui.max_rect().width();
                let top = ui.max_rect().top() - range.start as f32 * row_height;
                let row_rect = |row: usize| {
                    Rect::from_min_size(
                        Pos2::new(left, top + row as f32 * row_height),
                        Vec2::new(width, row_height),
                    )
                };
                if let Some(row) = reveal {
                    ui.scroll_to_rect(row_rect(row), None);
                }
                for row in range {
                    let (area, index) = work.row(row);
                    file_row(ui, row_rect(row), work, lfs, area, index, actions);
                }
            });
    });
}

/// `☑ 4 of 5 files ⋯`: include everything (or nothing), and the list's other actions.
fn list_header(ui: &Ui, rect: Rect, work: &Workspace, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let center = rect.center().y;
    let (staged, total) = work.included_count();
    let state = if total == 0 || staged == 0 {
        Check::Off
    } else if work.unstaged.is_empty() {
        Check::On
    } else {
        Check::Mixed
    };
    let check = Rect::from_center_size(Pos2::new(rect.left() + 14.0 + 9.0, center), Vec2::splat(18.0));
    if total > 0 {
        let (tooltip, action) = if state == Check::On {
            (
                widgets::tooltip_with_shortcut(ui, "Unstage all changes", &shortcut::UNSTAGE_ALL),
                Action::UnstageAll,
            )
        } else {
            (
                widgets::tooltip_with_shortcut(ui, "Stage all changes", &shortcut::STAGE_ALL),
                Action::StageAll,
            )
        };
        if widgets::check_box_at(ui, check, Id::unique("include-all"), state, &tooltip).clicked() {
            actions.push(action);
        }
    }
    let label = match total {
        0 => "No changed files".to_owned(),
        1 => format!("{staged} of 1 file"),
        _ => format!("{staged} of {total} files"),
    };
    let more = Rect::from_center_size(Pos2::new(rect.right() - 12.0 - 16.0, center), Vec2::splat(32.0));
    let left = if total > 0 { check.right() + 10.0 } else { rect.left() + 14.0 };
    text_line(
        ui,
        Pos2::new(left, center),
        &label,
        theme::strong(),
        palette.text,
        more.left() - space::S - left,
    );
    let response = icon_button_at(ui, more, Id::unique("file-list-more"), icon::MORE, "More actions");
    egui::Popup::menu(&response).show(|ui| {
        let item = |ui: &mut Ui, label: &str, enabled: bool, action: Action, actions: &mut Vec<Action>| {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                actions.push(action);
                ui.close();
            }
        };
        item(ui, "Stage All", !work.unstaged.is_empty(), Action::StageAll, actions);
        item(ui, "Unstage All", !work.staged.is_empty(), Action::UnstageAll, actions);
        ui.separator();
        item(ui, "Stash Staged Changes", !work.staged.is_empty(), Action::StashStaged, actions);
        item(ui, "Discard All Changes…", !work.unstaged.is_empty(), Action::DiscardAll, actions);
        ui.separator();
        item(ui, "Show in Explorer", true, Action::RevealInExplorer, actions);
    });
}

/// Status letter, its color and what it means, for an entry in one of the lists.
fn badge(entry: &StatusEntry, area: Area, palette: &Palette) -> (char, Color32, &'static str) {
    match entry.kind {
        EntryKind::Unmerged => return ('!', palette.danger, "Conflict"),
        EntryKind::Untracked => return ('A', palette.success, "New file (untracked)"),
        _ => {}
    }
    let change = match area {
        Area::Staged => entry.index,
        Area::Unstaged => entry.worktree,
    };
    match change {
        Change::Modified => ('M', palette.warning, "Modified"),
        Change::Added => ('A', palette.success, "Added"),
        Change::Deleted => ('D', palette.danger, "Deleted"),
        Change::Renamed => ('R', palette.accent, "Renamed"),
        Change::Copied => ('C', palette.accent, "Copied"),
        Change::TypeChanged => ('T', palette.warning, "Type changed"),
        _ => ('•', palette.text_secondary, "Changed"),
    }
}

fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(slash) => (&path[..slash], &path[slash + 1..]),
        None => ("", path),
    }
}

/// The paths that stage or unstage `entry` (a staged rename also needs its old path).
fn entry_paths(entry: &StatusEntry, area: Area) -> Vec<String> {
    let mut paths = vec![entry.path.clone()];
    if area == Area::Staged {
        paths.extend(entry.orig_path.clone());
    }
    paths
}

#[allow(clippy::too_many_lines)]
fn file_row(
    ui: &Ui,
    rect: Rect,
    work: &Workspace,
    lfs: &LfsState,
    area: Area,
    index: usize,
    actions: &mut Vec<Action>,
) {
    let Some(entry) = work.entry(area, index) else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let selection = &work.selection;
    let selected = selection.area == area && selection.items.contains(&index);
    let cursor = selection.area == area && selection.cursor == Some(index);

    let inner = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.bottom() - 1.0));
    let response = ui.interact(inner, Id::unique(("file-row", area, index)), Sense::click());
    let fill = if selected {
        palette.accent_soft
    } else {
        widgets::interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(inner, CornerRadius::same(radius::M), fill);
    if cursor && selection.items.len() > 1 {
        ui.painter().rect_stroke(
            inner,
            CornerRadius::same(radius::M),
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    }
    let center = inner.center().y;

    // Include: the checkbox stages or unstages the whole file.
    let included = work.included(area, index);
    let check = Rect::from_center_size(Pos2::new(inner.left() + 8.0 + 9.0, center), Vec2::splat(18.0));
    let (state, tooltip, action) = match (entry.kind, included) {
        (EntryKind::Unmerged, _) => (
            Check::Off,
            "Resolve the conflicts first".to_owned(),
            Action::ResolveConflicts(entry.path.clone()),
        ),
        (_, Included::All) => (
            Check::On,
            widgets::tooltip_with_shortcut(ui, "Unstage this file", &shortcut::TOGGLE_STAGE),
            Action::UnstagePaths(entry_paths(entry, area)),
        ),
        (_, Included::Some) => (
            Check::Mixed,
            "Partly staged: stage the rest of this file".to_owned(),
            Action::StagePaths(vec![entry.path.clone()]),
        ),
        (_, Included::None) => (
            Check::Off,
            widgets::tooltip_with_shortcut(ui, "Stage this file", &shortcut::TOGGLE_STAGE),
            Action::StagePaths(vec![entry.path.clone()]),
        ),
    };
    if widgets::check_box_at(ui, check, Id::unique(("file-check", area, index)), state, &tooltip)
        .clicked()
    {
        actions.push(action);
    }

    // Status as a tinted letter tile.
    let (letter, color, meaning) = badge(entry, area, palette);
    let tile = Rect::from_center_size(Pos2::new(check.right() + 10.0 + 9.0, center), Vec2::splat(18.0));
    ui.painter().rect_filled(
        tile,
        CornerRadius::same(radius::S),
        theme::fade(color, if palette.dark { 0.18 } else { 0.14 }),
    );
    ui.painter().text(
        tile.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId::new(10.5, theme::strong().family),
        color,
    );

    // Right side: discard while hovered, else the line counts; LFS marks before them.
    let mut right = inner.right() - 8.0;
    let hovered = ui.rect_contains_pointer(inner);
    if hovered && area == Area::Unstaged && entry.kind != EntryKind::Unmerged && !entry.submodule {
        let button = Rect::from_center_size(Pos2::new(right - 12.0, center), Vec2::splat(24.0));
        right -= 24.0 + space::XS;
        if icon_button_at(
            ui,
            button,
            Id::unique(("file-discard", area, index)),
            icon::UNDO,
            "Discard changes…  (Delete)",
        )
        .clicked()
        {
            actions.push(Action::DiscardPath(entry.path.clone()));
        }
    } else if let Some((added, removed)) = work.counts.get(&entry.path) {
        let counts = match (added, removed) {
            (0, 0) => String::new(),
            (added, 0) => format!("+{added}"),
            (0, removed) => format!("−{removed}"),
            (added, removed) => format!("+{added} −{removed}"),
        };
        if !counts.is_empty() {
            let painted = ui.painter().text(
                Pos2::new(right, center),
                Align2::RIGHT_CENTER,
                counts,
                FontId::new(11.0, FontFamily::Monospace),
                palette.text_tertiary,
            );
            right = painted.left() - space::S;
        }
    }

    // Git LFS: who holds the lock, and whether the file is stored with LFS.
    let attributes = lfs.attributes(&entry.path);
    let lock = lfs.lock(&entry.path);
    let mut lfs_note = String::new();
    let lock_mark = match lock {
        Some(lock) if lock.ours == Some(true) => {
            lfs_note.push_str("\nLocked by you");
            Some(palette.accent)
        }
        Some(lock) => {
            let owner = if lock.owner.is_empty() {
                "someone else"
            } else {
                lock.owner.as_str()
            };
            lfs_note.push_str(&format!("\nLocked by {owner}"));
            Some(palette.danger)
        }
        // Edited without a lock: the team expects one.
        None if attributes.lockable && lfs.locks_loaded => {
            lfs_note.push_str("\nLockable, but not locked by you: lock it before you edit it");
            Some(palette.warning)
        }
        None => None,
    };
    if let Some(color) = lock_mark {
        ui.painter().text(
            Pos2::new(right - 8.0, center),
            Align2::CENTER_CENTER,
            icon::LOCK,
            theme::icon_font(11.0),
            color,
        );
        right -= 16.0 + space::XS;
    }
    if attributes.lfs {
        lfs_note.push_str("\nStored with Git LFS");
        let painted = ui.painter().text(
            Pos2::new(right, center),
            Align2::RIGHT_CENTER,
            "LFS",
            theme::small_strong(),
            palette.text_tertiary,
        );
        right = painted.left() - space::XS;
    }

    // Name (semibold) and folder.
    let (dir, name) = split_path(&entry.path);
    let text_left = tile.right() + 10.0;
    let available = (right - space::XS - text_left).max(0.0);
    let deleted = letter == 'D';
    let width = text_line(
        ui,
        Pos2::new(text_left, center),
        name,
        theme::strong(),
        if deleted {
            palette.text_secondary
        } else {
            palette.text
        },
        available,
    );
    let detail = match (area, &entry.orig_path) {
        (Area::Staged, Some(orig)) => format!("← {orig}"),
        _ => dir.to_owned(),
    };
    let detail_room = available - width - 5.0;
    if !detail.is_empty() && detail_room > 24.0 {
        text_line(
            ui,
            Pos2::new(text_left + width + 5.0, center),
            &detail,
            theme::small(),
            palette.text_tertiary,
            detail_room,
        );
    }

    if response.clicked() {
        let modifiers = ui.input(|input| input.modifiers);
        actions.push(Action::SelectFile {
            area,
            index,
            toggle: modifiers.command,
            extend: modifiers.shift,
        });
    }
    if response.double_clicked() && entry.kind != EntryKind::Unmerged {
        actions.push(match included {
            Included::All => Action::UnstagePaths(entry_paths(entry, area)),
            _ => Action::StagePaths(vec![entry.path.clone()]),
        });
    }
    // Right-clicking outside the selection selects that file first, like Explorer.
    if response.secondary_clicked() && !selected {
        actions.push(Action::SelectFile {
            area,
            index,
            toggle: false,
            extend: false,
        });
    }
    let mut tooltip = format!("{}\n{meaning}", entry.path);
    if included == Included::Some {
        tooltip.push_str(", partly staged");
    }
    if entry.submodule {
        tooltip.push_str(" (submodule)");
    }
    tooltip.push_str(&lfs_note);
    let response = response.on_hover_text(tooltip);
    response.context_menu(|ui| {
        let stage = match area {
            Area::Unstaged => ("Stage", Action::StageSelected),
            Area::Staged => ("Unstage", Action::UnstageSelected),
        };
        super::history::menu_item(ui, stage.0, stage.1, actions);
        if included == Included::Some {
            super::history::menu_item(
                ui,
                "Unstage",
                Action::UnstagePaths(vec![entry.path.clone()]),
                actions,
            );
        }
        super::history::menu_item(ui, "Stash Selected Files…", Action::StashSelected, actions);
        if area == Area::Unstaged && entry.kind != EntryKind::Unmerged && !entry.submodule {
            super::history::menu_item(
                ui,
                "Discard Changes…",
                Action::DiscardPath(entry.path.clone()),
                actions,
            );
        }
        if entry.conflict.is_some() {
            ui.separator();
            let path = entry.path.clone();
            super::history::menu_item(
                ui,
                "Resolve Conflicts…",
                Action::ResolveConflicts(path.clone()),
                actions,
            );
            super::history::menu_item(
                ui,
                "Take Ours (Current Branch)",
                Action::TakeSide {
                    path: path.clone(),
                    side: facet_git::conflict::Side::Ours,
                },
                actions,
            );
            super::history::menu_item(
                ui,
                "Take Theirs (Incoming)",
                Action::TakeSide {
                    path: path.clone(),
                    side: facet_git::conflict::Side::Theirs,
                },
                actions,
            );
            super::history::menu_item(
                ui,
                "Mark Resolved",
                Action::StagePaths(vec![path.clone()]),
                actions,
            );
            super::history::menu_item(
                ui,
                "Open in External Merge Tool",
                Action::OpenMergeTool(path),
                actions,
            );
        }
        if lfs.usage.lfs && (attributes.lfs || attributes.lockable || lock.is_some()) {
            ui.separator();
            match lock {
                None => super::history::menu_item(
                    ui,
                    "Lock File (LFS)",
                    Action::LockFile(entry.path.clone()),
                    actions,
                ),
                Some(lock) if lock.ours == Some(false) => super::history::menu_item(
                    ui,
                    "Force Unlock (LFS)…",
                    Action::ForceUnlock {
                        id: lock.id.clone(),
                        path: lock.path.clone(),
                        owner: lock.owner.clone(),
                    },
                    actions,
                ),
                Some(lock) => super::history::menu_item(
                    ui,
                    "Unlock File (LFS)",
                    Action::UnlockFile {
                        id: lock.id.clone(),
                        path: lock.path.clone(),
                    },
                    actions,
                ),
            }
        }
        if entry.submodule {
            ui.separator();
            super::history::menu_item(
                ui,
                "Open Submodule in Tab",
                Action::OpenSubmodule(entry.path.clone()),
                actions,
            );
            super::history::menu_item(
                ui,
                "Update Submodule",
                Action::UpdateSubmodules(Some(entry.path.clone())),
                actions,
            );
        }
        ui.separator();
        if entry.kind != EntryKind::Untracked {
            super::history::menu_item(
                ui,
                "File History",
                Action::ShowFileHistory(entry.path.clone()),
                actions,
            );
            super::history::menu_item(
                ui,
                "Blame",
                Action::Blame {
                    path: entry.path.clone(),
                    rev: None,
                },
                actions,
            );
            super::history::menu_item(
                ui,
                "Open in External Diff Tool",
                Action::OpenDiffTool,
                actions,
            );
            ui.separator();
        }
        super::history::menu_item(
            ui,
            "Open in External Editor",
            Action::OpenInEditor(Some(entry.path.clone())),
            actions,
        );
        super::history::menu_item(ui, "Copy File Path", Action::CopyFilePath, actions);
    });
}

fn list_skeleton(ui: &Ui, rect: Rect) {
    let row = file_row_height();
    let mut y = rect.top() + LIST_HEADER + LIST_PAD;
    for (index, width) in [72.0_f32, 180.0, 140.0, 210.0, 96.0, 160.0]
        .into_iter()
        .enumerate()
    {
        let left = rect.left() + LIST_PAD + 8.0;
        let height = if index == 0 { 10.0 } else { 12.0 };
        skeleton(
            ui,
            Rect::from_min_size(
                Pos2::new(left, y + (row - height) / 2.0),
                Vec2::new(width.min(rect.width() - space::XL), height),
            ),
        );
        y += row;
    }
}

fn offer_text(ui: &Ui, width: f32, last_ms: Option<u32>) -> Arc<Galley> {
    let palette = Palette::of(ui.ctx());
    let timing = last_ms.map_or(String::new(), |ms| format!(" (the last took {ms} ms)"));
    let mut job = LayoutJob::single_section(
        format!(
            "Refreshing the status is slow here{timing}. Git's file system monitor and \
             untracked cache usually make it much faster. Only this repository's settings change."
        ),
        TextFormat {
            font_id: theme::small(),
            color: palette.text_secondary,
            ..Default::default()
        },
    );
    job.wrap.max_width = width.max(0.0);
    ui.painter().layout_job(job)
}

/// The opt-in offer to enable `core.fsmonitor` / `core.untrackedCache` for this repository.
fn perf_offer(ui: &Ui, rect: Rect, text: Arc<Galley>, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let card = rect.shrink2(Vec2::new(space::S, space::XS));
    ui.painter().rect(
        card,
        CornerRadius::same(radius::M),
        palette.accent_soft,
        Stroke::NONE,
        StrokeKind::Inside,
    );
    let left = card.left() + space::S;
    let mut y = card.top() + space::S;
    ui.painter().text(
        Pos2::new(left, y + 9.0),
        Align2::LEFT_CENTER,
        "Speed up status?",
        theme::small_strong(),
        palette.text,
    );
    y += 18.0 + space::XS;
    let text_height = text.size().y;
    ui.painter()
        .galley(Pos2::new(left, y), text, palette.text_secondary);
    y += text_height + space::S;

    let mut x = left - space::XS;
    for (label, tooltip, color, action) in [
        (
            "Enable",
            "Run git config --local core.fsmonitor true and core.untrackedCache true",
            palette.accent,
            Action::EnablePerf,
        ),
        (
            "Don't Ask Again",
            "Keep this repository's settings as they are",
            palette.text_secondary,
            Action::DeclinePerfOffer,
        ),
    ] {
        let width = text_button_width(ui, label);
        let button = Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, 24.0));
        x += width + space::XS;
        if text_button_at(
            ui,
            button,
            Id::unique(("perf-offer", label)),
            label,
            tooltip,
            color,
            false,
        )
        .clicked()
        {
            actions.push(action);
        }
    }
}

/// Height of the summary field and the commit button.
const FIELD_HEIGHT: f32 = 36.0;
/// Summary lines longer than this get the counter in the warning color.
const SUMMARY_LIMIT: usize = 72;

/// The commit message as the composer shows it: the summary line and the description
/// (everything after the blank line).
fn split_message(message: &str) -> (String, String) {
    match message.split_once('\n') {
        Some((summary, rest)) => (
            summary.trim_end_matches('\r').to_owned(),
            rest.trim_start_matches(['\r', '\n']).to_owned(),
        ),
        None => (message.to_owned(), String::new()),
    }
}

fn join_message(summary: &str, description: &str) -> String {
    if description.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}\n\n{description}")
    }
}

/// A text field of the composer: layer fill, hairline outline (accent while focused).
fn field_frame(ui: &Ui, rect: Rect, id: Id) {
    let palette = Palette::of(ui.ctx());
    let focused = ui.ctx().memory(|memory| memory.has_focus(id));
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::LAYER),
        palette.surface,
        Stroke::new(
            1.0,
            if focused {
                palette.accent
            } else {
                palette.hairline
            },
        ),
        StrokeKind::Inside,
    );
}

/// The composer: summary, description, the amend switch with the summary's length, and the
/// commit button. A rejected commit shows git's and the hooks' output above it.
#[allow(clippy::too_many_lines)]
fn commit_box(
    ui: &mut Ui,
    rect: Rect,
    work: &mut Workspace,
    can_commit: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let inner = rect.shrink(12.0);
    let has_commit = work.branch.oid.is_some();
    let branch = work.branch.head.clone();
    let staged = work.staged.len();
    let draft = &mut work.commit;
    let gap = 8.0;
    let button = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - FIELD_HEIGHT),
        inner.max,
    );
    let options = Rect::from_min_max(
        Pos2::new(inner.left() + 2.0, button.top() - gap - 18.0),
        Pos2::new(inner.right() - 2.0, button.top() - gap),
    );
    let mut top = inner.top();

    if let Some(error) = draft.error.clone() {
        let room = options.top() - gap - FIELD_HEIGHT - gap - 40.0 - gap - top;
        let height = room.clamp(56.0, 160.0);
        let area = Rect::from_min_size(
            Pos2::new(inner.left(), top),
            Vec2::new(inner.width(), height),
        );
        ui.painter().rect(
            area,
            CornerRadius::same(radius::M),
            theme::fade(palette.danger, 0.10),
            Stroke::new(1.0, theme::fade(palette.danger, 0.45)),
            StrokeKind::Inside,
        );
        let title_y = area.top() + 14.0;
        ui.painter().text(
            Pos2::new(area.left() + space::S, title_y),
            Align2::LEFT_CENTER,
            "The commit was rejected",
            theme::small_strong(),
            palette.danger,
        );
        let close =
            Rect::from_center_size(Pos2::new(area.right() - 14.0, title_y), Vec2::splat(22.0));
        if icon_button_at(
            ui,
            close,
            Id::unique("commit-error-close"),
            icon::CLOSE,
            "Dismiss",
        )
        .clicked()
        {
            actions.push(Action::DismissCommitError);
        }
        let text_rect = Rect::from_min_max(
            Pos2::new(area.left() + space::S, area.top() + 28.0),
            area.max - Vec2::splat(space::XS),
        );
        region(ui, "commit-error", text_rect, |ui| {
            ScrollArea::vertical()
                .id_salt("commit-error-scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(error)
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(palette.text),
                    );
                });
        });
        top = area.bottom() + gap;
    }

    let (mut summary, mut description) = split_message(&draft.message);
    let (old_summary, old_description) = (summary.clone(), description.clone());
    let focus_now = std::mem::take(&mut draft.focus_pending);
    let running = draft.running;

    // Summary: one line, semibold.
    let summary_rect = Rect::from_min_size(
        Pos2::new(inner.left(), top),
        Vec2::new(inner.width(), FIELD_HEIGHT),
    );
    let summary_id = Id::unique("commit-message");
    field_frame(ui, summary_rect, summary_id);
    let hint = format!(
        "{}  ({})",
        if draft.amend {
            "Summary of the amended commit"
        } else {
            "Summary"
        },
        widgets::shortcut_text(ui, &shortcut::FOCUS_MESSAGE)
    );
    let mut next_line = false;
    region(ui, "commit-summary", summary_rect.shrink(1.0), |ui| {
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let response = ui.add(
                TextEdit::singleline(&mut summary)
                    .id(summary_id)
                    .hint_text(hint)
                    .font(theme::strong())
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(11, 8)))
                    .desired_width(f32::INFINITY)
                    .interactive(!running),
            );
            if focus_now {
                response.request_focus();
            }
            // Enter moves on to the description, like a mail subject line.
            next_line = response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter) && !input.modifiers.ctrl);
        });
    });

    // Description: fills the rest.
    let description_rect = Rect::from_min_max(
        Pos2::new(inner.left(), summary_rect.bottom() + gap),
        Pos2::new(
            inner.right(),
            (options.top() - gap).max(summary_rect.bottom() + gap + 32.0),
        ),
    );
    let description_id = Id::unique("commit-description");
    field_frame(ui, description_rect, description_id);
    let edit_height = description_rect.height() - 2.0;
    region(ui, "commit-edit", description_rect.shrink(1.0), |ui| {
        ScrollArea::vertical()
            .id_salt("commit-edit-scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                let response = ui.add(
                    TextEdit::multiline(&mut description)
                        .id(description_id)
                        .hint_text("Description (optional)")
                        .font(theme::body())
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(11, 8)))
                        .desired_width(f32::INFINITY)
                        .min_size(Vec2::new(0.0, edit_height))
                        .interactive(!running),
                );
                if next_line {
                    response.request_focus();
                }
            });
    });
    if summary != old_summary || description != old_description {
        draft.message = join_message(summary.trim_end_matches(['\r', '\n']), &description);
    }

    // Amend switch and the summary's length.
    let amend = draft.amend;
    let toggle = Rect::from_min_size(
        Pos2::new(options.left(), options.center().y - 8.0),
        Vec2::new(30.0, 16.0),
    );
    let label_width = ui
        .painter()
        .layout_no_wrap("Amend last commit".to_owned(), theme::small(), palette.text)
        .size()
        .x;
    let amend_rect = Rect::from_min_max(
        toggle.min,
        Pos2::new(toggle.right() + 8.0 + label_width, toggle.bottom()),
    );
    let enabled = has_commit && !running;
    let response = ui.interact(
        amend_rect,
        Id::unique("amend"),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    super::settings::switch_visual(ui, toggle, response.id, amend, response.hovered(), enabled);
    ui.painter().text(
        Pos2::new(toggle.right() + 8.0, options.center().y),
        Align2::LEFT_CENTER,
        "Amend last commit",
        theme::small(),
        if enabled {
            palette.text_secondary
        } else {
            palette.text_tertiary
        },
    );
    let response = response.on_hover_text(if has_commit {
        "Replace the last commit with the staged changes and this message"
    } else {
        "There is no commit to amend yet"
    });
    if response.clicked() {
        actions.push(Action::ToggleAmend);
    }
    let length = summary.chars().count();
    ui.painter().text(
        Pos2::new(options.right(), options.center().y),
        Align2::RIGHT_CENTER,
        format!("{length} / {SUMMARY_LIMIT}"),
        theme::small(),
        if length > SUMMARY_LIMIT {
            palette.warning
        } else {
            palette.text_tertiary
        },
    );

    // The commit button.
    let label = if running {
        "Committing…".to_owned()
    } else if amend {
        "Amend last commit".to_owned()
    } else {
        let files = match staged {
            0 => "Commit".to_owned(),
            1 => "Commit 1 file".to_owned(),
            count => format!("Commit {count} files"),
        };
        match &branch {
            Some(branch) => format!("{files} to {branch}"),
            None => files,
        }
    };
    let tooltip = if running {
        "git commit is running".to_owned()
    } else if can_commit {
        widgets::tooltip_with_shortcut(ui, &label, &shortcut::COMMIT)
    } else if draft.message.trim().is_empty() {
        "Write a commit message first".to_owned()
    } else {
        "Stage some changes first, or amend the last commit".to_owned()
    };
    let keys = widgets::shortcut_text(ui, &shortcut::COMMIT);
    let keys = keys.strip_suffix("+Enter").unwrap_or(&keys);
    if commit_button(ui, button, &label, keys, can_commit)
        .on_hover_text(tooltip)
        .clicked()
    {
        actions.push(Action::Commit);
    }
}

/// The Return key glyph (Segoe Fluent Icons), drawn after the shortcut's modifiers.
const RETURN_KEY: char = '\u{E751}';

/// Full-width accent button: the label, then the shortcut (`Ctrl ↵`) dimmed.
fn commit_button(ui: &Ui, rect: Rect, label: &str, keys: &str, enabled: bool) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let response = widgets::primary_button_at(ui, rect, Id::unique("commit-button"), None, "", enabled);
    let color = if enabled {
        palette.on_accent
    } else {
        palette.text_tertiary
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::strong(), color);
    let keys = ui.painter().layout_no_wrap(
        keys.to_owned(),
        theme::small(),
        theme::fade(color, 0.75),
    );
    let max_label = rect.width() - 24.0 - keys.size().x - 16.0 - 10.0;
    let galley = if galley.size().x > max_label {
        widgets::elided(ui, label, theme::strong(), color, max_label)
    } else {
        galley
    };
    let keys_width = keys.size().x + 4.0 + 12.0;
    let total = galley.size().x + 10.0 + keys_width;
    let left = rect.center().x - total / 2.0;
    let center = rect.center().y;
    let keys_left = left + galley.size().x + 10.0;
    let keys_height = keys.size().y;
    let return_left = keys_left + keys.size().x + 4.0;
    ui.painter().galley(
        Pos2::new(left, center - galley.size().y / 2.0),
        galley,
        color,
    );
    ui.painter().galley(
        Pos2::new(keys_left, center - keys_height / 2.0),
        keys,
        color,
    );
    ui.painter().text(
        Pos2::new(return_left + 6.0, center + 1.0),
        Align2::CENTER_CENTER,
        RETURN_KEY,
        theme::icon_font(11.0),
        theme::fade(color, 0.75),
    );
    response
}
