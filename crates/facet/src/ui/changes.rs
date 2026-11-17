//! The working copy view: a compact repository header, the list of changed files with the
//! commit box below it, and the diff of the current file beside it (`diff.rs`).

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId,
    Galley, Id, Layout, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit,
    TextFormat, Ui, UiBuilder, Vec2,
};
use facet_git::parse::{Change, EntryKind, StatusEntry};
use facet_git::Head;

use super::widgets::{
    self, button_enabled, checkbox, icon_button_at, skeleton, text_button_at, text_button_width,
    text_line, ButtonKind,
};
use super::{region, splitter_feedback, SPLITTER_GRAB};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, TabState};
use crate::settings::{Settings, CHANGES_WIDTH, COMMIT_BOX_HEIGHT};
use crate::theme::{self, icon, radius, space, Palette};
use crate::workspace::{Area, Row, Workspace};

const HEADER_HEIGHT: f32 = 44.0;
const FILE_ROW: f32 = 28.0;
const MIN_DIFF_WIDTH: f32 = 320.0;
const MIN_LIST_HEIGHT: f32 = 96.0;

impl FacetApp {
    pub(super) fn work_view(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let mut actions = Vec::new();

        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT));
        self.repo_header(ui, header, &mut actions);
        widgets::hairline_h(ui, rect.x_range().into(), header.bottom());

        let body = Rect::from_min_max(Pos2::new(rect.left(), header.bottom() + 1.0), rect.max);
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

    /// Branch, tracking state and the commit at HEAD, plus refresh / explorer buttons.
    fn repo_header(&self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let palette = Palette::of(ui.ctx());
        let Some(tab) = self.active_tab() else {
            return;
        };
        let TabState::Ready { head, .. } = &tab.state else {
            return;
        };
        let work = &tab.work;
        let center = rect.center().y;

        let size = 28.0;
        let refresh = Rect::from_center_size(
            Pos2::new(rect.right() - space::M - size / 2.0, center),
            Vec2::splat(size),
        );
        let tooltip = widgets::tooltip_with_shortcut(ui, "Refresh", &shortcut::REFRESH);
        if icon_button_at(
            ui,
            refresh,
            Id::unique("work-refresh"),
            icon::REFRESH,
            &tooltip,
        )
        .clicked()
        {
            actions.push(Action::Refresh);
        }
        let reveal = refresh.translate(Vec2::new(-(size + space::XS), 0.0));
        let tooltip =
            widgets::tooltip_with_shortcut(ui, "Open in File Explorer", &shortcut::REVEAL);
        if icon_button_at(
            ui,
            reveal,
            Id::unique("work-reveal"),
            icon::FOLDER,
            &tooltip,
        )
        .clicked()
        {
            actions.push(Action::RevealInExplorer);
        }
        let mut right = reveal.left() - space::M;
        if let Some(ms) = work.perf.last_ms {
            let painted = ui.painter().text(
                Pos2::new(right, center),
                Align2::RIGHT_CENTER,
                format!("Status {ms} ms"),
                theme::small(),
                palette.text_tertiary,
            );
            right = painted.left() - space::M;
        }
        let room = |x: f32| (right - x).max(0.0);

        let mut x = rect.left() + space::M;
        let (branch, color) = match head {
            Head::Branch { name, .. } => (name.clone(), palette.text),
            Head::Detached { commit } => {
                (format!("Detached at {}", commit.short_oid), palette.warning)
            }
        };
        x += text_line(
            ui,
            Pos2::new(x, center),
            &branch,
            theme::strong(),
            color,
            room(x),
        ) + space::S;

        let mut tracking = Vec::new();
        if let Some(ahead) = work.branch.ahead.filter(|n| *n > 0) {
            tracking.push(format!("↑{ahead}"));
        }
        if let Some(behind) = work.branch.behind.filter(|n| *n > 0) {
            tracking.push(format!("↓{behind}"));
        }
        if !tracking.is_empty() {
            x += text_line(
                ui,
                Pos2::new(x, center),
                &tracking.join(" "),
                theme::small_strong(),
                palette.text_secondary,
                room(x),
            ) + space::S;
        }
        x += space::XS;
        match head.commit() {
            Some(commit) => {
                x += text_line(
                    ui,
                    Pos2::new(x, center),
                    &commit.short_oid,
                    theme::mono(),
                    palette.text_tertiary,
                    room(x),
                ) + space::S;
                if room(x) > 24.0 {
                    text_line(
                        ui,
                        Pos2::new(x, center),
                        &commit.subject,
                        theme::body(),
                        palette.text_secondary,
                        room(x),
                    );
                }
            }
            None => {
                text_line(
                    ui,
                    Pos2::new(x, center),
                    "No commits yet",
                    theme::body(),
                    palette.text_secondary,
                    room(x),
                );
            }
        }
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
        let Some(work) = self.active_work_mut() else {
            return;
        };

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
            file_list(ui, list, work, actions);
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

/// The combined, virtualized list: "Staged" header and files, "Changes" header and files.
fn file_list(ui: &mut Ui, rect: Rect, work: &mut Workspace, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    if work.staged.is_empty() && work.unstaged.is_empty() {
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
            .map(|cursor| work.row_of(work.selection.area, cursor))
    } else {
        None
    };
    let rows = work.row_count();
    let work = &*work;
    region(ui, "file-list", rect, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt("file-list-scroll")
            .auto_shrink(false)
            .show_rows(ui, FILE_ROW, rows, |ui, range| {
                let left = ui.max_rect().left();
                let width = ui.max_rect().width();
                let top = ui.max_rect().top() - range.start as f32 * FILE_ROW;
                let row_rect = |row: usize| {
                    Rect::from_min_size(
                        Pos2::new(left, top + row as f32 * FILE_ROW),
                        Vec2::new(width, FILE_ROW),
                    )
                };
                if let Some(row) = reveal {
                    ui.scroll_to_rect(row_rect(row), None);
                }
                for row in range {
                    let rect = row_rect(row);
                    match work.row(row) {
                        Row::Header(area) => {
                            section_header(ui, rect, area, work.list(area).len(), actions);
                        }
                        Row::File(area, index) => file_row(ui, rect, work, area, index, actions),
                    }
                }
            });
    });
}

fn section_header(ui: &Ui, rect: Rect, area: Area, count: usize, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let center = rect.center().y;
    let title = match area {
        Area::Staged => "Staged",
        Area::Unstaged => "Changes",
    };
    let x = rect.left() + space::M;
    let width = text_line(
        ui,
        Pos2::new(x, center),
        title,
        theme::small_strong(),
        palette.text_secondary,
        rect.width(),
    );
    ui.painter().text(
        Pos2::new(x + width + space::S, center),
        Align2::LEFT_CENTER,
        count.to_string(),
        theme::small(),
        palette.text_tertiary,
    );
    if count == 0 {
        return;
    }

    let mut right = rect.right() - space::S;
    let mut add = |label: &str, tooltip: String, color: Color32, action: Action| {
        let width = text_button_width(ui, label);
        let button = Rect::from_min_max(
            Pos2::new(right - width, center - 11.0),
            Pos2::new(right, center + 11.0),
        );
        right -= width + space::XS;
        if text_button_at(
            ui,
            button,
            Id::unique(("section-button", label)),
            label,
            &tooltip,
            color,
            false,
        )
        .clicked()
        {
            actions.push(action);
        }
    };
    match area {
        Area::Staged => add(
            "Unstage All",
            widgets::tooltip_with_shortcut(ui, "Unstage all changes", &shortcut::UNSTAGE_ALL),
            palette.text_secondary,
            Action::UnstageAll,
        ),
        Area::Unstaged => {
            add(
                "Stage All",
                widgets::tooltip_with_shortcut(ui, "Stage all changes", &shortcut::STAGE_ALL),
                palette.accent,
                Action::StageAll,
            );
            add(
                "Discard All…",
                "Discard all changes (asks first; can be undone)".to_owned(),
                palette.danger,
                Action::DiscardAll,
            );
        }
    }
}

/// Status letter, its color and what it means, for an entry in one of the lists.
fn badge(entry: &StatusEntry, area: Area, palette: &Palette) -> (char, Color32, &'static str) {
    match entry.kind {
        EntryKind::Unmerged => return ('!', palette.danger, "Conflict"),
        EntryKind::Untracked => return ('U', palette.success, "New file (untracked)"),
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

fn file_row(
    ui: &Ui,
    rect: Rect,
    work: &Workspace,
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

    let response = ui.interact(rect, Id::unique(("file-row", area, index)), Sense::click());
    let inner = rect.shrink2(Vec2::new(space::XS, 1.0));
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

    let center = rect.center().y;
    let (letter, color, meaning) = badge(entry, area, palette);
    ui.painter().text(
        Pos2::new(inner.left() + space::S + 6.0, center),
        Align2::CENTER_CENTER,
        letter,
        theme::small_strong(),
        color,
    );

    // Row buttons appear while the pointer is over the row.
    let mut right = inner.right() - space::XS;
    if ui.rect_contains_pointer(rect) {
        let size = 24.0;
        let mut button = |glyph: char, tooltip: &str, salt: &str, action: Action| {
            let rect =
                Rect::from_center_size(Pos2::new(right - size / 2.0, center), Vec2::splat(size));
            right -= size + 2.0;
            if icon_button_at(
                ui,
                rect,
                Id::unique(("file-button", area, index, salt)),
                glyph,
                tooltip,
            )
            .clicked()
            {
                actions.push(action);
            }
        };
        match area {
            Area::Unstaged => {
                button(
                    icon::ADD,
                    "Stage  (Enter)",
                    "stage",
                    Action::StagePaths(vec![entry.path.clone()]),
                );
                if entry.kind != EntryKind::Unmerged && !entry.submodule {
                    button(
                        icon::UNDO,
                        "Discard changes…  (Delete)",
                        "discard",
                        Action::DiscardPath(entry.path.clone()),
                    );
                }
            }
            Area::Staged => {
                let mut paths = vec![entry.path.clone()];
                paths.extend(entry.orig_path.clone());
                button(
                    icon::REMOVE,
                    "Unstage  (Enter)",
                    "unstage",
                    Action::UnstagePaths(paths),
                );
            }
        }
    }

    let (dir, name) = split_path(&entry.path);
    let text_left = inner.left() + space::S + 12.0 + space::S;
    let available = (right - space::XS - text_left).max(0.0);
    let width = text_line(
        ui,
        Pos2::new(text_left, center),
        name,
        theme::body(),
        palette.text,
        available,
    );
    let detail = match (area, &entry.orig_path) {
        (Area::Staged, Some(orig)) => format!("← {orig}"),
        _ => dir.to_owned(),
    };
    let detail_room = available - width - space::S;
    if !detail.is_empty() && detail_room > 24.0 {
        text_line(
            ui,
            Pos2::new(text_left + width + space::S, center),
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
    if response.double_clicked() {
        actions.push(match area {
            Area::Unstaged => Action::StagePaths(vec![entry.path.clone()]),
            Area::Staged => {
                let mut paths = vec![entry.path.clone()];
                paths.extend(entry.orig_path.clone());
                Action::UnstagePaths(paths)
            }
        });
    }
    let mut tooltip = format!("{}\n{meaning}", entry.path);
    if entry.submodule {
        tooltip.push_str(" (submodule)");
    }
    response.on_hover_text(tooltip);
}

fn list_skeleton(ui: &Ui, rect: Rect) {
    let mut y = rect.top() + space::S;
    for (index, width) in [72.0_f32, 180.0, 140.0, 210.0, 96.0, 160.0]
        .into_iter()
        .enumerate()
    {
        let left = rect.left() + space::M;
        let height = if index == 0 { 10.0 } else { 12.0 };
        skeleton(
            ui,
            Rect::from_min_size(
                Pos2::new(left, y + (FILE_ROW - height) / 2.0),
                Vec2::new(width.min(rect.width() - space::XL), height),
            ),
        );
        y += FILE_ROW;
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

/// Commit message, amend toggle and the commit button; a rejected commit shows git's and the
/// hooks' output above the message.
fn commit_box(
    ui: &mut Ui,
    rect: Rect,
    work: &mut Workspace,
    can_commit: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let inner = rect.shrink(space::S);
    let has_commit = work.branch.oid.is_some();
    let staged = work.staged.len();
    let draft = &mut work.commit;
    let bottom_row = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - theme::ROW_HEIGHT),
        inner.max,
    );
    let mut top = inner.top();

    if let Some(error) = draft.error.clone() {
        let height = ((bottom_row.top() - top) * 0.5).clamp(56.0, 160.0);
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
        top = area.bottom() + space::S;
    }

    let edit = Rect::from_min_max(
        Pos2::new(inner.left(), top),
        Pos2::new(inner.right(), (bottom_row.top() - space::S).max(top + 24.0)),
    );
    let edit_id = Id::unique("commit-message");
    let focused = ui.ctx().memory(|memory| memory.has_focus(edit_id));
    ui.painter().rect(
        edit,
        CornerRadius::same(radius::M),
        palette.hover,
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
    let focus_now = std::mem::take(&mut draft.focus_pending);
    let running = draft.running;
    let hint = format!(
        "{}  ({})",
        if draft.amend {
            "Message of the amended commit"
        } else {
            "Commit message"
        },
        widgets::shortcut_text(ui, &shortcut::FOCUS_MESSAGE)
    );
    let edit_height = edit.height() - 2.0;
    region(ui, "commit-edit", edit.shrink(1.0), |ui| {
        ScrollArea::vertical()
            .id_salt("commit-edit-scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                let response = ui.add(
                    TextEdit::multiline(&mut draft.message)
                        .id(edit_id)
                        .hint_text(hint)
                        .font(theme::body())
                        // egui 0.37: a custom frame replaces `.margin()`; pad via the frame.
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(8)))
                        .desired_width(f32::INFINITY)
                        .min_size(Vec2::new(0.0, edit_height))
                        .interactive(!running),
                );
                if focus_now {
                    response.request_focus();
                }
            });
    });

    let amend = draft.amend;
    let message_empty = draft.message.trim().is_empty();
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("commit-row")
            .max_rect(bottom_row)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            let tooltip = if has_commit {
                "Replace the last commit with the staged changes and this message"
            } else {
                "There is no commit to amend yet"
            };
            if checkbox(ui, amend, "Amend", tooltip, has_commit && !running).clicked() {
                actions.push(Action::ToggleAmend);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = match (running, amend) {
                    (true, _) => "Committing…",
                    (false, true) => "Amend",
                    (false, false) => "Commit",
                };
                let tooltip = if running {
                    "git commit is running".to_owned()
                } else if can_commit {
                    let what = if amend {
                        "Amend the last commit".to_owned()
                    } else if staged == 1 {
                        "Commit 1 staged file".to_owned()
                    } else {
                        format!("Commit {staged} staged files")
                    };
                    widgets::tooltip_with_shortcut(ui, &what, &shortcut::COMMIT)
                } else if message_empty {
                    "Write a commit message first".to_owned()
                } else {
                    "Stage some changes first, or amend the last commit".to_owned()
                };
                if button_enabled(
                    ui,
                    ButtonKind::Primary,
                    Some(icon::CHECK),
                    label,
                    &tooltip,
                    can_commit,
                )
                .clicked()
                {
                    actions.push(Action::Commit);
                }
            });
        },
    );
}
