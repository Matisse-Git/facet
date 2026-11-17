//! The diff pane: a header naming the file with its stage / discard actions, and the diff
//! itself — text with syntax colors, word-level highlights and line selection for partial
//! staging; a metadata card for binaries; an image comparison for pictures.

use std::borrow::Cow;
use std::ops::Range;

use eframe::egui::{
    text::LayoutJob, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Id, Pos2, Rect,
    ScrollArea, Sense, TextFormat, Ui, Vec2,
};
use facet_git::parse::{EntryKind, FileDiff, LineKind};
use facet_git::DiffSource;

use super::image_diff;
use super::region;
use super::widgets::{self, skeleton, text_button_at, text_button_width, text_line};
use crate::actions::{shortcut, Action};
use crate::app::FacetApp;
use crate::filekind::size_label;
use crate::highlight::{display_text, Decorations, Span};
use crate::settings::ImageMode;
use crate::theme::{self, icon, radius, space, Palette};
use crate::workspace::{
    Area, BinaryInfo, DiffBody, DiffKey, DiffRow, LineDrag, TextDiff, Workspace,
};

const HEADER_HEIGHT: f32 = 44.0;
const LINE_HEIGHT: f32 = 20.0;
/// Accent bar marking a selected line, at the gutter's left edge.
const SELECT_BAR: f32 = 3.0;

impl FacetApp {
    pub(super) fn diff_pane(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let palette = Palette::of(ui.ctx());
        ui.painter().rect_filled(rect, 0.0, palette.surface);
        let mode = self.settings.image_mode;
        let Some(work) = self.active_work_mut() else {
            return;
        };
        let Some(key) = work.diff.key.clone() else {
            empty_state(ui, rect, work);
            return;
        };

        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT));
        diff_header(ui, header, work, &key, mode, actions);
        widgets::hairline_h(ui, rect.x_range().into(), header.bottom());

        let body = Rect::from_min_max(Pos2::new(rect.left(), header.bottom() + 1.0), rect.max);
        let reset = std::mem::take(&mut work.diff.reset_scroll);
        let view = &mut work.diff;
        match &mut view.body {
            DiffBody::None | DiffBody::Loading => text_skeleton(ui, body),
            DiffBody::Failed(message) => {
                message_view(ui, body, "Couldn't show this diff", message);
            }
            DiffBody::Unchanged(diff) => {
                let (title, detail) = unchanged_text(diff);
                message_view(ui, body, title, &detail);
            }
            DiffBody::Binary(info) => binary_view(ui, body, info),
            DiffBody::Image(images) => region(ui, "image-diff", body, |ui| {
                image_diff::show(ui, body, images, mode, &mut view.swipe, &mut view.onion);
            }),
            DiffBody::Text(text) => region(ui, "text-diff", body, |ui| {
                text_view(ui, body, text, key.area, reset, actions);
            }),
        }
    }
}

fn empty_state(ui: &Ui, rect: Rect, work: &Workspace) {
    if !work.loaded {
        return;
    }
    let palette = Palette::of(ui.ctx());
    let (title, detail) = if work.staged.is_empty() && work.unstaged.is_empty() {
        (
            "Nothing to commit",
            "Changes to files in this repository appear here",
        )
    } else {
        (
            "Select a file to see its changes",
            "Use ↑ and ↓ to move through the list",
        )
    };
    let center = rect.center();
    ui.painter().text(
        center - Vec2::new(0.0, 10.0),
        Align2::CENTER_CENTER,
        title,
        theme::strong(),
        palette.text_secondary,
    );
    ui.painter().text(
        center + Vec2::new(0.0, 12.0),
        Align2::CENTER_CENTER,
        detail,
        theme::small(),
        palette.text_tertiary,
    );
}

fn file_name(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(slash) => (&path[..slash], &path[slash + 1..]),
        None => ("", path),
    }
}

/// File name, line counts and where the diff comes from; the file's actions on the right.
fn diff_header(
    ui: &Ui,
    rect: Rect,
    work: &Workspace,
    key: &DiffKey,
    mode: ImageMode,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let center = rect.center().y;
    let path = &key.request.path;
    let entry = work
        .current()
        .filter(|(area, entry)| *area == key.area && entry.path == *path)
        .map(|(_, entry)| entry);
    let text = work.diff.text();
    let selected = text
        .filter(|text| text.partial)
        .map_or(0, |text| text.selected.len());

    let right = std::cell::Cell::new(rect.right() - space::S);
    let mut add = |label: &str, tooltip: String, color: Color32, active: bool, action: Action| {
        let width = text_button_width(ui, label);
        let button = Rect::from_min_max(
            Pos2::new(right.get() - width, center - 12.0),
            Pos2::new(right.get(), center + 12.0),
        );
        right.set(right.get() - width - space::XS);
        if text_button_at(
            ui,
            button,
            Id::unique(("diff-header-button", label)),
            label,
            &tooltip,
            color,
            active,
        )
        .clicked()
        {
            actions.push(action);
        }
    };

    let lines = |count: usize| {
        if count == 1 {
            "1 Line".to_owned()
        } else {
            format!("{count} Lines")
        }
    };
    let enter = widgets::shortcut_text(ui, &shortcut::TOGGLE_STAGE);
    if selected > 0 {
        match key.area {
            Area::Unstaged => {
                add(
                    &format!("Stage {}", lines(selected)),
                    format!("Stage the selected lines  ({enter})"),
                    palette.accent,
                    false,
                    Action::StageLines,
                );
                add(
                    &format!("Discard {}…", lines(selected)),
                    "Discard the selected lines (asks first; can be undone)".to_owned(),
                    palette.danger,
                    false,
                    Action::DiscardLines,
                );
            }
            Area::Staged => add(
                &format!("Unstage {}", lines(selected)),
                format!("Unstage the selected lines  ({enter})"),
                palette.text_secondary,
                false,
                Action::UnstageLines,
            ),
        }
    } else if let Some(entry) = entry {
        match key.area {
            Area::Unstaged => {
                let (label, tooltip) = if entry.kind == EntryKind::Unmerged {
                    (
                        "Mark Resolved",
                        "Stage the file as it is now, marking the conflict resolved",
                    )
                } else {
                    ("Stage File", "Stage this file")
                };
                add(
                    label,
                    format!("{tooltip}  ({enter})"),
                    palette.accent,
                    false,
                    Action::StagePaths(vec![path.clone()]),
                );
                if entry.kind != EntryKind::Unmerged && !entry.submodule {
                    add(
                        "Discard File…",
                        "Discard this file's changes (asks first; can be undone)".to_owned(),
                        palette.danger,
                        false,
                        Action::DiscardPath(path.clone()),
                    );
                }
            }
            Area::Staged => {
                let mut paths = vec![path.clone()];
                paths.extend(key.request.orig_path.clone());
                add(
                    "Unstage File",
                    format!("Unstage this file  ({enter})"),
                    palette.text_secondary,
                    false,
                    Action::UnstagePaths(paths),
                );
            }
        }
    }
    if let DiffBody::Image(images) = &work.diff.body {
        if images.old.is_some() && images.new.is_some() {
            right.set(right.get() - space::S);
            for option in ImageMode::ALL.into_iter().rev() {
                let active = option == mode;
                add(
                    option.label(),
                    format!("Compare the images: {}", option.label()),
                    if active {
                        palette.text
                    } else {
                        palette.text_secondary
                    },
                    active,
                    Action::SetImageMode(option),
                );
            }
        }
    }

    // Left side: name, +/− counts, source and folder.
    let room = |x: f32| (right.get() - space::S - x).max(0.0);
    let (dir, name) = file_name(path);
    let mut x = rect.left() + space::M;
    x += text_line(
        ui,
        Pos2::new(x, center),
        name,
        theme::strong(),
        palette.text,
        room(x),
    );
    x += space::S;
    if let Some(text) = text {
        for (count, sign, color) in [
            (text.diff.added, '+', palette.success),
            (text.diff.removed, '−', palette.danger),
        ] {
            if count > 0 && room(x) > 24.0 {
                x += text_line(
                    ui,
                    Pos2::new(x, center),
                    &format!("{sign}{count}"),
                    theme::small_strong(),
                    color,
                    room(x),
                ) + space::XS;
            }
        }
        x += space::XS;
    }
    let source = match key.request.source {
        DiffSource::Staged => "Staged",
        DiffSource::Unstaged => "Unstaged",
        DiffSource::Untracked => "Untracked",
        DiffSource::Conflict => "Conflict",
    };
    let mut detail = source.to_owned();
    if let Some(orig) = &key.request.orig_path {
        detail.push_str(&format!(" · renamed from {orig}"));
    } else if !dir.is_empty() {
        detail.push_str(&format!(" · {dir}"));
    }
    if room(x) > 24.0 {
        text_line(
            ui,
            Pos2::new(x, center),
            &detail,
            theme::small(),
            palette.text_tertiary,
            room(x),
        );
    }
}

fn text_skeleton(ui: &Ui, rect: Rect) {
    let mut y = rect.top() + space::M;
    for width in [0.42, 0.66, 0.58, 0.8, 0.36, 0.7, 0.5] {
        let bar = Rect::from_min_size(
            Pos2::new(rect.left() + space::XL + space::L, y),
            Vec2::new((rect.width() - space::XL * 3.0).max(0.0) * width, 10.0),
        );
        skeleton(ui, bar);
        y += LINE_HEIGHT;
    }
}

fn message_view(ui: &Ui, rect: Rect, title: &str, detail: &str) {
    let palette = Palette::of(ui.ctx());
    let width = (rect.width() - space::XL * 2.0).max(0.0);
    let mut job = LayoutJob::single_section(
        detail.trim().to_owned(),
        TextFormat {
            font_id: theme::small(),
            color: palette.text_tertiary,
            ..Default::default()
        },
    );
    job.wrap.max_width = width;
    job.wrap.max_rows = 8;
    job.wrap.overflow_character = Some('…');
    job.halign = eframe::egui::Align::Center;
    let detail = ui.painter().layout_job(job);
    let top = rect.center().y - (20.0 + space::XS + detail.size().y) / 2.0;
    ui.painter().text(
        Pos2::new(rect.center().x, top + 10.0),
        Align2::CENTER_CENTER,
        title,
        theme::strong(),
        palette.text_secondary,
    );
    ui.painter().galley(
        Pos2::new(rect.center().x, top + 20.0 + space::XS),
        detail,
        palette.text_tertiary,
    );
}

/// What an empty text diff means.
fn unchanged_text(diff: &FileDiff) -> (&'static str, String) {
    match (diff.old_mode, diff.new_mode, diff.similarity) {
        (Some(old), Some(new), _) if old != new => (
            "File mode changed",
            format!("{old:o} → {new:o}; the content is the same"),
        ),
        (_, _, Some(similarity)) => (
            "Renamed without changes",
            format!("{similarity}% similar; the content is the same"),
        ),
        _ if diff.new_file => ("Empty file", "The new file has no content".to_owned()),
        _ if diff.deleted_file => ("Empty file deleted", "The file had no content".to_owned()),
        _ => (
            "No changes to show",
            "The two versions are identical".to_owned(),
        ),
    }
}

fn binary_view(ui: &Ui, rect: Rect, info: &BinaryInfo) {
    let palette = Palette::of(ui.ctx());
    let card = Rect::from_center_size(
        rect.center(),
        Vec2::new((rect.width() - space::XL * 2.0).clamp(0.0, 360.0), 168.0),
    );
    ui.painter().rect(
        card,
        CornerRadius::same(radius::L),
        palette.hover,
        palette.hairline_stroke(),
        eframe::egui::StrokeKind::Inside,
    );
    let x = card.center().x;
    let mut y = card.top() + space::L + 16.0;
    ui.painter().text(
        Pos2::new(x, y),
        Align2::CENTER_CENTER,
        icon::DOCUMENT,
        theme::icon_font(28.0),
        palette.text_secondary,
    );
    y += 16.0 + space::M + 10.0;
    ui.painter().text(
        Pos2::new(x, y),
        Align2::CENTER_CENTER,
        &info.description,
        theme::strong(),
        palette.text,
    );
    y += 22.0;
    let sizes = match (info.old_size, info.new_size) {
        (Some(old), Some(new)) => format!("{} → {}", size_label(old), size_label(new)),
        (None, Some(new)) => format!("New · {}", size_label(new)),
        (Some(old), None) => format!("Deleted · {}", size_label(old)),
        (None, None) => String::new(),
    };
    ui.painter().text(
        Pos2::new(x, y),
        Align2::CENTER_CENTER,
        sizes,
        theme::small(),
        palette.text_secondary,
    );
    y += 20.0;
    ui.painter().text(
        Pos2::new(x, y),
        Align2::CENTER_CENTER,
        "Binary content is not shown",
        theme::small(),
        palette.text_tertiary,
    );
    if info.lfs {
        y += 20.0;
        ui.painter().text(
            Pos2::new(x, y),
            Align2::CENTER_CENTER,
            "Stored with Git LFS",
            theme::small_strong(),
            palette.accent,
        );
    }
}

/// The text diff: virtualized rows, a sticky gutter with line numbers, and (when the diff
/// supports partial staging) click / drag line selection and per-hunk actions.
fn text_view(
    ui: &mut Ui,
    rect: Rect,
    text: &mut TextDiff,
    area: Area,
    reset: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let font = theme::mono();
    let number_font = FontId::new(12.0, FontFamily::Monospace);
    let char_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
    let digits = text
        .diff
        .lines
        .iter()
        .map(|line| line.old_no.max(line.new_no))
        .max()
        .unwrap_or(0)
        .to_string()
        .len()
        .max(3);
    let column = digits as f32 * char_width + space::S;
    let gutter = SELECT_BAR + space::XS + column * 2.0 + char_width + space::S;
    let rows = text.row_count();
    let content = Vec2::new(
        gutter + space::S + text.widest as f32 * char_width * 1.05 + space::XL,
        rows as f32 * LINE_HEIGHT + space::S,
    );

    let mut scroll = ScrollArea::both()
        .id_salt("diff-text-scroll")
        .auto_shrink(false);
    if reset {
        scroll = scroll.scroll_offset(Vec2::ZERO);
    }
    let _ = rect;
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(Vec2::new(content.x.max(viewport.width()), content.y));
        let origin = ui.max_rect().min;
        let screen = viewport.translate(origin.to_vec2());
        let first = (viewport.min.y / LINE_HEIGHT).floor().max(0.0) as usize;
        let last = ((viewport.max.y / LINE_HEIGHT).ceil().max(0.0) as usize).min(rows);
        let row_rect = |row: usize| {
            Rect::from_min_size(
                Pos2::new(screen.left(), origin.y + row as f32 * LINE_HEIGHT),
                Vec2::new(screen.width(), LINE_HEIGHT),
            )
        };
        let row_at = |pos: Pos2| {
            (((pos.y - origin.y) / LINE_HEIGHT).floor().max(0.0) as usize)
                .min(rows.saturating_sub(1))
        };

        let hovered = if text.partial && rows > 0 {
            line_selection(ui, screen, text, row_at)
        } else {
            None
        };

        // Pass 1: line backgrounds and text (scrolls horizontally).
        let painter = ui.painter();
        let decorations = text.decorations.clone();
        let text_left = origin.x + gutter + space::S;
        for row in first..last {
            let DiffRow::Line(index) = text.row(row) else {
                continue;
            };
            let line = &text.diff.lines[index];
            let rect = row_rect(row);
            if let Some(tint) = line_tint(line.kind, palette) {
                painter.rect_filled(rect, 0.0, tint);
            }
            if text.selected.contains(&(index as u32)) {
                painter.rect_filled(rect, 0.0, palette.accent_soft);
            } else if hovered == Some(row) && line.kind != LineKind::Context {
                painter.rect_filled(rect, 0.0, palette.hover);
            }
            let job = line_job(
                text.diff.line_bytes(line),
                index,
                line.kind,
                line.no_newline,
                decorations.as_deref(),
                palette,
                &font,
            );
            let galley = painter.layout_job(job);
            painter.galley(
                Pos2::new(text_left, rect.center().y - galley.size().y / 2.0),
                galley,
                palette.text,
            );
        }

        // Pass 2: the sticky gutter and hunk headers, painted over the text.
        let gutter_rect = Rect::from_min_max(
            screen.min,
            Pos2::new(screen.left() + gutter, screen.bottom()),
        );
        painter.rect_filled(gutter_rect, 0.0, palette.surface);
        let old_right = screen.left() + SELECT_BAR + space::XS + column;
        let new_right = old_right + column;
        let sign_center = new_right + space::XS + char_width / 2.0;
        for row in first..last {
            let rect = row_rect(row);
            match text.row(row) {
                DiffRow::Line(index) => {
                    let line = &text.diff.lines[index];
                    let cell =
                        Rect::from_min_max(rect.min, Pos2::new(gutter_rect.right(), rect.bottom()));
                    if let Some(tint) = line_tint(line.kind, palette) {
                        painter.rect_filled(cell, 0.0, tint);
                    }
                    if text.selected.contains(&(index as u32)) {
                        painter.rect_filled(cell, 0.0, palette.accent_soft);
                        painter.rect_filled(
                            Rect::from_min_size(rect.min, Vec2::new(SELECT_BAR, LINE_HEIGHT)),
                            0.0,
                            palette.accent,
                        );
                    }
                    let y = rect.center().y;
                    let (old, new, sign, sign_color) = match line.kind {
                        LineKind::Context => (line.old_no, line.new_no, ' ', palette.text),
                        LineKind::Added => (0, line.new_no, '+', palette.success),
                        LineKind::Removed => (line.old_no, 0, '−', palette.danger),
                    };
                    for (number, right) in [(old, old_right), (new, new_right)] {
                        if number > 0 {
                            painter.text(
                                Pos2::new(right - space::XS, y),
                                Align2::RIGHT_CENTER,
                                number.to_string(),
                                number_font.clone(),
                                palette.text_tertiary,
                            );
                        }
                    }
                    if sign != ' ' {
                        painter.text(
                            Pos2::new(sign_center, y),
                            Align2::CENTER_CENTER,
                            sign,
                            font.clone(),
                            sign_color,
                        );
                    }
                }
                DiffRow::Hunk(hunk) => {
                    hunk_header(ui, rect, text, hunk, area, &font, actions);
                }
            }
        }
        painter.vline(
            gutter_rect.right() + 0.5,
            screen.y_range(),
            palette.hairline_stroke(),
        );
    });
}

fn line_tint(kind: LineKind, palette: &Palette) -> Option<Color32> {
    match kind {
        LineKind::Context => None,
        LineKind::Added => Some(palette.diff_added),
        LineKind::Removed => Some(palette.diff_removed),
    }
}

/// Click a changed line to (de)select it, drag to paint the selection over a range, click a
/// hunk header to (de)select its changed lines. Returns the row under the pointer.
fn line_selection(
    ui: &Ui,
    screen: Rect,
    text: &mut TextDiff,
    row_at: impl Fn(Pos2) -> usize,
) -> Option<usize> {
    let response = ui.interact(
        screen,
        ui.scope_id().with("diff-lines"),
        Sense::click_and_drag(),
    );
    let line_at = |text: &TextDiff, row: usize| -> u32 {
        match text.row(row) {
            DiffRow::Line(line) => line as u32,
            DiffRow::Hunk(hunk) => text.diff.hunks[hunk].lines.start,
        }
    };

    if response.drag_started() {
        if let Some(pos) = ui.input(|input| input.pointer.press_origin()) {
            if let DiffRow::Line(line) = text.row(row_at(pos)) {
                if text.is_changed_line(line) {
                    let line = line as u32;
                    let select = !text.selected.contains(&line);
                    let base = text.selected.clone();
                    text.paint_selection(line, line, select, &base);
                    text.drag = Some(LineDrag {
                        anchor: line,
                        select,
                        base,
                    });
                }
            }
        }
    }
    if response.dragged() {
        if let (Some(drag), Some(pos)) = (text.drag.clone(), response.interact_pointer_pos()) {
            let line = line_at(text, row_at(pos));
            text.paint_selection(drag.anchor, line, drag.select, &drag.base);
            // Keep scrolling while the pointer is held near (or past) an edge.
            let edge = LINE_HEIGHT * 1.5;
            let delta = if pos.y < screen.top() + edge {
                (screen.top() + edge - pos.y).min(LINE_HEIGHT * 2.0)
            } else if pos.y > screen.bottom() - edge {
                -(pos.y - (screen.bottom() - edge)).min(LINE_HEIGHT * 2.0)
            } else {
                0.0
            };
            if delta != 0.0 {
                ui.scroll_with_delta(Vec2::new(0.0, delta * 0.5));
                ui.ctx().request_repaint();
            }
        }
    }
    if response.drag_stopped() {
        text.drag = None;
    }
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            match text.row(row_at(pos)) {
                DiffRow::Line(line) if text.is_changed_line(line) => {
                    let line = line as u32;
                    if !text.selected.remove(&line) {
                        text.selected.insert(line);
                    }
                }
                DiffRow::Hunk(hunk) => {
                    let lines: Range<u32> = text.diff.hunks[hunk].lines.clone();
                    let changed: Vec<u32> = lines
                        .filter(|line| text.is_changed_line(*line as usize))
                        .collect();
                    let all = changed.iter().all(|line| text.selected.contains(line));
                    for line in changed {
                        if all {
                            text.selected.remove(&line);
                        } else {
                            text.selected.insert(line);
                        }
                    }
                }
                DiffRow::Line(_) => {}
            }
        }
    }

    let row = response.hover_pos().map(&row_at)?;
    let pointable = match text.row(row) {
        DiffRow::Line(line) => text.is_changed_line(line),
        DiffRow::Hunk(_) => true,
    };
    if pointable {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    Some(row)
}

/// A hunk header row: the `@@` line and, when partial staging is possible, its actions.
fn hunk_header(
    ui: &Ui,
    rect: Rect,
    text: &TextDiff,
    hunk: usize,
    area: Area,
    font: &FontId,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, palette.surface);
    painter.rect_filled(rect, 0.0, palette.diff_hunk);

    let mut right = rect.right() - space::S;
    if text.partial {
        let buttons: &[(&str, &str, Color32, Action)] = match area {
            Area::Unstaged => &[
                (
                    "Stage Hunk",
                    "Stage this hunk",
                    palette.accent,
                    Action::StageHunk(hunk),
                ),
                (
                    "Discard…",
                    "Discard this hunk (asks first; can be undone)",
                    palette.danger,
                    Action::DiscardHunk(hunk),
                ),
            ],
            Area::Staged => &[(
                "Unstage Hunk",
                "Unstage this hunk",
                palette.text_secondary,
                Action::UnstageHunk(hunk),
            )],
        };
        for (label, tooltip, color, action) in buttons {
            let width = text_button_width(ui, label);
            let button = Rect::from_min_max(
                Pos2::new(right - width, rect.top() + 1.0),
                Pos2::new(right, rect.bottom() - 1.0),
            );
            right -= width + space::XS;
            if text_button_at(
                ui,
                button,
                Id::unique(("hunk-button", hunk, *label)),
                label,
                tooltip,
                *color,
                false,
            )
            .clicked()
            {
                actions.push(action.clone());
            }
        }
    }

    let header = String::from_utf8_lossy(text.diff.header_bytes(&text.diff.hunks[hunk]));
    let clip = Rect::from_min_max(rect.min, Pos2::new(right - space::S, rect.bottom()));
    painter
        .with_clip_rect(clip.intersect(painter.clip_rect()))
        .text(
            Pos2::new(rect.left() + SELECT_BAR + space::S, rect.center().y),
            Align2::LEFT_CENTER,
            header.trim_end(),
            font.clone(),
            palette.text_secondary,
        );
}

/// One diff line as a layout job: syntax colors (when made for the current appearance),
/// stronger backgrounds behind the words that changed, tabs as four spaces.
fn line_job(
    bytes: &[u8],
    index: usize,
    kind: LineKind,
    no_newline: bool,
    decorations: Option<&Decorations>,
    palette: &Palette,
    font: &FontId,
) -> LayoutJob {
    let (display, cut) = display_text(bytes);
    let display: &str = &display;
    let syntax: &[Span] = decorations
        .filter(|decorations| decorations.dark == palette.dark)
        .and_then(|decorations| decorations.syntax.get(index))
        .map_or(&[], |spans| &spans[..]);
    let emphasis: &[Range<u32>] = decorations
        .and_then(|decorations| decorations.emphasis.get(index))
        .map_or(&[], |ranges| &ranges[..]);
    let strong = match kind {
        LineKind::Added => palette.diff_added_strong,
        LineKind::Removed => palette.diff_removed_strong,
        LineKind::Context => Color32::TRANSPARENT,
    };

    let len = display.len();
    let mut cuts = vec![0, len];
    cuts.extend(syntax.iter().map(|span| span.end as usize));
    cuts.extend(
        emphasis
            .iter()
            .flat_map(|range| [range.start as usize, range.end as usize]),
    );
    cuts.retain(|cut| *cut <= len && display.is_char_boundary(*cut));
    cuts.sort_unstable();
    cuts.dedup();

    let mut job = LayoutJob::default();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let color = syntax
            .iter()
            .find(|span| span.end as usize > start)
            .and_then(|span| span.color)
            .unwrap_or(palette.text);
        let emphasized = emphasis
            .iter()
            .any(|range| range.start as usize <= start && start < range.end as usize);
        let segment = &display[start..end];
        let segment = if segment.contains('\t') {
            Cow::Owned(segment.replace('\t', "    "))
        } else {
            Cow::Borrowed(segment)
        };
        job.append(
            &segment,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                background: if emphasized {
                    strong
                } else {
                    Color32::TRANSPARENT
                },
                ..Default::default()
            },
        );
    }
    let note = |job: &mut LayoutJob, text: &str| {
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: theme::small(),
                color: palette.text_tertiary,
                valign: eframe::egui::Align::Center,
                ..Default::default()
            },
        );
    };
    if cut {
        note(&mut job, "  … (line shortened)");
    }
    if no_newline {
        note(&mut job, "  (no newline at end of file)");
    }
    job
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    fn plain(job: &LayoutJob) -> Vec<(String, bool)> {
        job.sections
            .iter()
            .map(|section| {
                (
                    job.text[section.byte_range.start.0..section.byte_range.end.0].to_owned(),
                    section.format.background != Color32::TRANSPARENT,
                )
            })
            .collect()
    }

    #[test]
    fn line_jobs_split_at_emphasis_and_expand_tabs() {
        let decorations = Decorations {
            dark: true,
            syntax: vec![Box::new([])],
            emphasis: vec![Box::new([4..7])],
        };
        let job = line_job(
            b"let\tfoo = 1;",
            0,
            LineKind::Added,
            false,
            Some(&decorations),
            &DARK,
            &theme::mono(),
        );
        assert_eq!(
            plain(&job),
            [
                ("let    ".to_owned(), false),
                ("foo".to_owned(), true),
                (" = 1;".to_owned(), false)
            ]
        );
    }

    #[test]
    fn syntax_colors_for_the_other_appearance_are_ignored() {
        let red = Color32::from_rgb(255, 0, 0);
        let decorations = Decorations {
            dark: false,
            syntax: vec![Box::new([Span {
                end: 3,
                color: Some(red),
            }])],
            emphasis: vec![Box::new([])],
        };
        let job = line_job(
            b"abc",
            0,
            LineKind::Context,
            true,
            Some(&decorations),
            &DARK,
            &theme::mono(),
        );
        assert_eq!(job.sections[0].format.color, DARK.text);
        assert!(job.text.ends_with("(no newline at end of file)"));
    }

    #[test]
    fn empty_diffs_explain_themselves() {
        let mut diff = facet_git::parse::parse_file_diff(Vec::new()).unwrap();
        diff.old_mode = Some(0o100644);
        diff.new_mode = Some(0o100755);
        assert_eq!(
            unchanged_text(&diff).1,
            "100644 → 100755; the content is the same"
        );
        diff.new_mode = diff.old_mode;
        diff.similarity = Some(100);
        assert_eq!(unchanged_text(&diff).0, "Renamed without changes");
    }
}
