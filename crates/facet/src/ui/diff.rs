//! The diff pane: a header naming the file with its stage / discard actions, and the diff
//! itself — text with syntax colors, word-level highlights and line selection for partial
//! staging; a metadata card for binaries; an image comparison for pictures.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{
    self, text::LayoutJob, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Id, Pos2, Rect,
    ScrollArea, Sense, TextFormat, Ui, Vec2,
};
use facet_git::parse::{EntryKind, FileDiff, LineKind, LogCommit};

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
    Area, BinaryInfo, DiffBody, DiffKey, DiffView, LineDrag, SubmoduleDiff, TextDiff, Workspace,
    NO_LINE,
};

pub(super) const HEADER_HEIGHT: f32 = 48.0;
pub const LINE_HEIGHT: f32 = 22.0;

/// Text diffs are drawn side by side (the setting, copied here each frame so the history's
/// and stashes' diffs follow it too).
static SPLIT: AtomicBool = AtomicBool::new(false);

pub fn set_split(split: bool) {
    SPLIT.store(split, Ordering::Relaxed);
}
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
        let split = SPLIT.load(Ordering::Relaxed);
        diff_header(ui, header, work, &key, mode, split, actions);
        widgets::hairline_h(ui, rect.x_range().into(), header.bottom());

        let body = Rect::from_min_max(Pos2::new(rect.left(), header.bottom() + 1.0), rect.max);
        diff_body(ui, body, &mut work.diff, key.area, mode, actions);
    }
}

/// The diff itself (text, image comparison, binary card or a message), below a header.
/// Shared by the working copy and the history's commit detail.
pub(super) fn diff_body(
    ui: &mut Ui,
    body: Rect,
    view: &mut DiffView,
    area: Area,
    mode: ImageMode,
    actions: &mut Vec<Action>,
) {
    // A different file opened: back to the top. The benchmark scrolls to a given offset.
    let reset = std::mem::take(&mut view.reset_scroll);
    let scroll_to = view.scroll_to.take().or(reset.then_some(0.0));
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
        DiffBody::Submodule(range) => region(ui, "submodule-diff", body, |ui| {
            submodule_view(ui, range);
        }),
        DiffBody::Image(images) => region(ui, "image-diff", body, |ui| {
            image_diff::show(ui, body, images, mode, &mut view.swipe, &mut view.onion);
        }),
        DiffBody::Text(text) => region(ui, "text-diff", body, |ui| {
            let split = SPLIT.load(Ordering::Relaxed);
            text_view(ui, body, text, area, split, scroll_to, actions);
        }),
    }
}

/// A plain header for a diff that has no actions (a file of a commit): name, counts, folder.
pub(super) fn plain_diff_header(ui: &Ui, rect: Rect, view: &DiffView) {
    let Some(key) = &view.key else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let center = rect.center().y;
    let room = |x: f32| (rect.right() - space::M - x).max(0.0);
    let (dir, name) = file_name(&key.request.path);
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
    if let Some(text) = view.text() {
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
    let detail = match &key.request.orig_path {
        Some(orig) => format!("renamed from {orig}"),
        None => dir.to_owned(),
    };
    if !detail.is_empty() && room(x) > 24.0 {
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

/// File name and folder, line counts, the layout switch and the file's actions (more of them
/// in the ⋯ menu). Selected lines get their own stage / discard buttons.
#[allow(clippy::too_many_lines)]
fn diff_header(
    ui: &Ui,
    rect: Rect,
    work: &Workspace,
    key: &DiffKey,
    mode: ImageMode,
    split: bool,
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

    // ⋯: everything you can do with this file.
    let more = Rect::from_center_size(
        Pos2::new(rect.right() - 12.0 - 16.0, center),
        Vec2::splat(32.0),
    );
    let response = widgets::icon_button_at(
        ui,
        more,
        Id::unique("diff-more"),
        icon::MORE,
        "More file actions",
    );
    egui::Popup::menu(&response).show(|ui| {
        let item = super::history::menu_item;
        if let Some(entry) = entry {
            match key.area {
                Area::Unstaged if entry.kind == EntryKind::Unmerged => {
                    item(ui, "Mark Resolved", Action::StagePaths(vec![path.clone()]), actions);
                }
                Area::Unstaged => {
                    item(ui, "Stage File", Action::StagePaths(vec![path.clone()]), actions);
                }
                Area::Staged => {
                    let mut paths = vec![path.clone()];
                    paths.extend(key.request.orig_path.clone());
                    item(ui, "Unstage File", Action::UnstagePaths(paths), actions);
                }
            }
            if key.area == Area::Unstaged && entry.kind != EntryKind::Unmerged && !entry.submodule
            {
                item(ui, "Discard File…", Action::DiscardPath(path.clone()), actions);
            }
            if entry.conflict.is_some() {
                ui.separator();
                item(ui, "Resolve Conflicts…", Action::ResolveConflicts(path.clone()), actions);
                item(
                    ui,
                    "Take Ours (Current Branch)",
                    Action::TakeSide {
                        path: path.clone(),
                        side: facet_git::conflict::Side::Ours,
                    },
                    actions,
                );
                item(
                    ui,
                    "Take Theirs (Incoming)",
                    Action::TakeSide {
                        path: path.clone(),
                        side: facet_git::conflict::Side::Theirs,
                    },
                    actions,
                );
            }
            ui.separator();
            if entry.kind != EntryKind::Untracked {
                item(ui, "File History", Action::ShowFileHistory(path.clone()), actions);
                item(
                    ui,
                    "Blame",
                    Action::Blame {
                        path: path.clone(),
                        rev: None,
                    },
                    actions,
                );
                item(ui, "Open in External Diff Tool", Action::OpenDiffTool, actions);
            }
        }
        item(
            ui,
            "Open in External Editor",
            Action::OpenInEditor(Some(path.clone())),
            actions,
        );
        item(ui, "Copy File Path", Action::CopyFilePath, actions);
    });
    let mut right = more.left() - 8.0;

    // Layout: Unified / Split for text, the comparison mode for images.
    let mut segmented = |labels: &[&str], current: usize, salt: &str| -> Option<usize> {
        let (width, clicked) = super::settings::segmented(
            ui,
            Id::unique(("diff-layout", salt)),
            right,
            center,
            labels,
            current,
        );
        right -= width + 12.0;
        clicked
    };
    match &work.diff.body {
        DiffBody::Text(_) => {
            if let Some(clicked) = segmented(&["Unified", "Split"], usize::from(split), "text") {
                actions.push(Action::SetSplitDiff(clicked == 1));
            }
        }
        DiffBody::Image(images) if images.old.is_some() && images.new.is_some() => {
            let labels: Vec<&str> = ImageMode::ALL.iter().map(|mode| mode.label()).collect();
            let current = ImageMode::ALL
                .iter()
                .position(|option| *option == mode)
                .unwrap_or(0);
            if let Some(clicked) = segmented(&labels, current, "image") {
                actions.push(Action::SetImageMode(ImageMode::ALL[clicked]));
            }
        }
        _ => {}
    }

    // +N −M
    if let Some(text) = text {
        let font = FontId::new(12.0, FontFamily::Monospace);
        for (count, sign, color) in [
            (text.diff.removed, '−', palette.danger),
            (text.diff.added, '+', palette.success),
        ] {
            if count > 0 {
                let painted = ui.painter().text(
                    Pos2::new(right, center),
                    Align2::RIGHT_CENTER,
                    format!("{sign}{count}"),
                    font.clone(),
                    color,
                );
                right = painted.left() - 6.0;
            }
        }
        right -= 6.0;
    }

    // Buttons for what needs doing now: selected lines, or a conflict.
    let mut add = |label: &str, tooltip: String, color: Color32, action: Action| {
        let width = text_button_width(ui, label);
        let button = Rect::from_min_max(
            Pos2::new(right - width, center - 13.0),
            Pos2::new(right, center + 13.0),
        );
        right -= width + space::XS;
        if text_button_at(
            ui,
            button,
            Id::unique(("diff-header-button", label)),
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
                    Action::StageLines,
                );
                add(
                    &format!("Discard {}…", lines(selected)),
                    "Discard the selected lines (asks first; can be undone)".to_owned(),
                    palette.danger,
                    Action::DiscardLines,
                );
            }
            Area::Staged => add(
                &format!("Unstage {}", lines(selected)),
                format!("Unstage the selected lines  ({enter})"),
                palette.text_secondary,
                Action::UnstageLines,
            ),
        }
    } else if entry.is_some_and(|entry| entry.conflict.is_some()) {
        add(
            "Resolve…",
            "Pick a side for each conflict, one by one".to_owned(),
            palette.accent,
            Action::ResolveConflicts(path.clone()),
        );
    }

    // Left side: name (14 semibold) and folder.
    let room = |x: f32| (right - space::S - x).max(0.0);
    let (dir, name) = file_name(path);
    let mut x = rect.left() + 18.0;
    x += text_line(
        ui,
        Pos2::new(x, center),
        name,
        theme::large_strong(),
        palette.text,
        room(x),
    );
    x += 8.0;
    let detail = match &key.request.orig_path {
        Some(orig) => format!("renamed from {orig}"),
        None => dir.to_owned(),
    };
    if !detail.is_empty() && room(x) > 24.0 {
        text_line(
            ui,
            Pos2::new(x, center + 1.0),
            &detail,
            theme::small(),
            palette.text_tertiary,
            room(x),
        );
    }
}

/// The working tree matches the last commit: say so, and offer what usually comes next.
pub(super) fn clean_state(ui: &Ui, rect: Rect, work: &Workspace, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let branch = &work.branch;
    let remote = branch
        .upstream
        .as_deref()
        .and_then(|upstream| upstream.split('/').next())
        .unwrap_or("the remote");
    let ahead = branch.ahead.unwrap_or(0);
    let mut detail = match &branch.head {
        Some(name) => format!("Your files match the last commit on {name}."),
        None => "Your files match the last commit.".to_owned(),
    };
    let push = if branch.oid.is_none() {
        None
    } else if branch.upstream.is_none() && branch.head.is_some() {
        detail.push_str(" This branch isn’t published yet.");
        Some("Publish Branch".to_owned())
    } else if ahead > 0 {
        let commits = if ahead == 1 {
            "1 commit that isn’t".to_owned()
        } else {
            format!("{ahead} commits that aren’t")
        };
        detail.push_str(&format!(" You have {commits} on {remote} yet."));
        Some(if ahead == 1 {
            "Push 1 commit".to_owned()
        } else {
            format!("Push {ahead} commits")
        })
    } else {
        None
    };

    let width = 380.0_f32.min(rect.width() - 48.0).max(120.0);
    let mut job = LayoutJob::single_section(
        detail,
        TextFormat {
            font_id: theme::large(),
            color: palette.text_secondary,
            line_height: Some(22.0),
            ..Default::default()
        },
    );
    job.wrap.max_width = width;
    job.halign = egui::Align::Center;
    let paragraph = ui.painter().layout_job(job);
    let height = 56.0 + 4.0 + 12.0 + 26.0 + 12.0 + paragraph.size().y + 12.0 + 8.0 + 36.0;
    let mut y = rect.center().y - height / 2.0;
    let x = rect.center().x;

    let badge = Pos2::new(x, y + 28.0);
    ui.painter()
        .circle_filled(badge, 28.0, palette.diff_added);
    widgets::check_mark(ui, Rect::from_center_size(badge, Vec2::splat(26.0)), palette.success, 2.0);
    y += 56.0 + 4.0 + 12.0;
    ui.painter().text(
        Pos2::new(x, y + 13.0),
        Align2::CENTER_CENTER,
        "Nothing to commit",
        FontId::new(20.0, theme::strong().family),
        palette.text,
    );
    y += 26.0 + 12.0;
    let paragraph_height = paragraph.size().y;
    ui.painter()
        .galley(Pos2::new(x, y), paragraph, palette.text_secondary);
    y += paragraph_height + 12.0 + 8.0;

    let explorer = "Show in Explorer";
    let explorer_width = widgets::outline_button_width(ui, None, explorer) + 4.0;
    let push_width = push.as_ref().map_or(0.0, |label| {
        ui.painter()
            .layout_no_wrap(label.clone(), theme::strong(), Color32::WHITE)
            .size()
            .x
            + 32.0
    });
    let total = explorer_width + push.as_ref().map_or(0.0, |_| push_width + 8.0);
    let mut left = x - total / 2.0;
    if let Some(label) = &push {
        let button = Rect::from_min_size(Pos2::new(left, y), Vec2::new(push_width, 36.0));
        left += push_width + 8.0;
        if widgets::primary_button_at(ui, button, Id::unique("clean-push"), None, label, true)
            .clicked()
        {
            actions.push(Action::Push);
        }
    }
    let button = Rect::from_min_size(Pos2::new(left, y), Vec2::new(explorer_width, 36.0));
    if widgets::outline_button_at(
        ui,
        button,
        Id::unique("clean-explorer"),
        None,
        explorer,
        "Open the repository's folder",
    )
    .clicked()
    {
        actions.push(Action::RevealInExplorer);
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

/// A submodule pointer change: where it moved, then the commits it brings in and drops.
fn submodule_view(ui: &mut Ui, range: &SubmoduleDiff) {
    const ROW: f32 = 26.0;
    enum Line<'a> {
        Title(String),
        Note(&'a str),
        Section(String),
        Commit(&'a LogCommit, bool),
    }
    let short = |oid: &str| oid.chars().take(7).collect::<String>();
    let mut lines = vec![Line::Title(match (&range.old, &range.new) {
        (Some(old), Some(new)) if old == new => format!("Submodule at {}", short(new)),
        (Some(old), Some(new)) => format!("Submodule moved from {} to {}", short(old), short(new)),
        (None, Some(new)) => format!("Submodule added at {}", short(new)),
        (Some(old), None) => format!("Submodule removed (was at {})", short(old)),
        (None, None) => "Submodule not checked out".to_owned(),
    })];
    if let Some(note) = &range.unavailable {
        lines.push(Line::Note(note));
    }
    for (commits, added) in [(&range.added, true), (&range.removed, false)] {
        if commits.is_empty() {
            continue;
        }
        let count = commits.len();
        let noun = if count == 1 { "commit" } else { "commits" };
        let more = if count >= 500 { " (first 500)" } else { "" };
        lines.push(Line::Section(if added {
            format!("{count} {noun} added{more}")
        } else {
            format!("{count} {noun} removed{more}")
        }));
        lines.extend(commits.iter().map(|commit| Line::Commit(commit, added)));
    }

    let palette = Palette::of(ui.ctx());
    let now = crate::format::unix_seconds(std::time::SystemTime::now());
    ScrollArea::vertical()
        .id_salt("submodule-scroll")
        .auto_shrink(false)
        .show_rows(ui, ROW, lines.len(), |ui, rows| {
            let left = ui.max_rect().left() + space::M;
            let right = ui.max_rect().right() - space::M;
            let top = ui.max_rect().top() - rows.start as f32 * ROW + space::S;
            for index in rows {
                let center = top + index as f32 * ROW + ROW / 2.0;
                let room = |x: f32| (right - x).max(0.0);
                match &lines[index] {
                    Line::Title(text) => {
                        text_line(
                            ui,
                            Pos2::new(left, center),
                            text,
                            theme::strong(),
                            palette.text,
                            room(left),
                        );
                    }
                    Line::Note(text) => {
                        text_line(
                            ui,
                            Pos2::new(left, center),
                            text,
                            theme::small(),
                            palette.warning,
                            room(left),
                        );
                    }
                    Line::Section(text) => {
                        text_line(
                            ui,
                            Pos2::new(left, center + 4.0),
                            text,
                            theme::small_strong(),
                            palette.text_tertiary,
                            room(left),
                        );
                    }
                    Line::Commit(commit, added) => {
                        let (sign, color) = if *added {
                            ('+', palette.success)
                        } else {
                            ('−', palette.danger)
                        };
                        ui.painter().text(
                            Pos2::new(left + 4.0, center),
                            Align2::CENTER_CENTER,
                            sign,
                            theme::small_strong(),
                            color,
                        );
                        let mut x = left + space::M;
                        x += text_line(
                            ui,
                            Pos2::new(x, center),
                            &commit.short_oid(),
                            theme::mono(),
                            palette.text_secondary,
                            room(x),
                        ) + space::S;
                        let meta = format!(
                            "{} · {}",
                            commit.author,
                            crate::format::relative_time(now, commit.time)
                        );
                        let meta_width = widgets::elided(
                            ui,
                            &meta,
                            theme::small(),
                            palette.text_tertiary,
                            f32::INFINITY,
                        )
                        .size()
                        .x;
                        let subject_room = (room(x) - meta_width - space::M).max(room(x) * 0.5);
                        let width = text_line(
                            ui,
                            Pos2::new(x, center),
                            &commit.subject,
                            theme::body(),
                            palette.text,
                            subject_room,
                        );
                        x += width + space::M;
                        text_line(
                            ui,
                            Pos2::new(x, center),
                            &meta,
                            theme::small(),
                            palette.text_tertiary,
                            room(x),
                        );
                    }
                }
            }
        });
}

/// The text diff: virtualized rows, a sticky gutter with line numbers, and (when the diff
/// supports partial staging) click / drag line selection and per-hunk actions.
/// Where things sit in a text diff drawn as hunk cards: a 36 pt header per hunk, 22 pt lines,
/// 16 pt between cards. Rows are the hunk's lines (unified) or line pairs (split).
struct Cards<'a> {
    text: &'a TextDiff,
    split: bool,
}

impl Cards<'_> {
    fn hunks(&self) -> usize {
        self.text.diff.hunks.len()
    }

    /// The first row of hunk `hunk` among all hunks' rows.
    fn start(&self, hunk: usize) -> usize {
        if hunk >= self.hunks() {
            return if self.split {
                self.text.pairs.len()
            } else {
                self.text.diff.lines.len()
            };
        }
        if self.split {
            self.text.pair_starts[hunk] as usize
        } else {
            self.text.diff.hunks[hunk].lines.start as usize
        }
    }

    fn rows(&self, hunk: usize) -> usize {
        self.start(hunk + 1) - self.start(hunk)
    }

    /// Top of hunk `hunk`'s card (relative to the content's top).
    fn top(&self, hunk: usize) -> f32 {
        CARD_TOP + hunk as f32 * (HUNK_HEADER + CARD_GAP) + self.start(hunk) as f32 * LINE_HEIGHT
    }

    fn bottom(&self, hunk: usize) -> f32 {
        self.top(hunk) + HUNK_HEADER + self.rows(hunk) as f32 * LINE_HEIGHT
    }

    fn height(&self) -> f32 {
        match self.hunks() {
            0 => 0.0,
            hunks => self.bottom(hunks - 1) + CARD_BOTTOM,
        }
    }

    /// The hunk whose card is at or above `y`.
    fn hunk_at(&self, y: f32) -> usize {
        let (mut low, mut high) = (0, self.hunks());
        while low < high {
            let mid = (low + high) / 2;
            if self.top(mid) <= y {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low.saturating_sub(1)
    }

    /// The line drawn in row `row` of a hunk, on one side (`0` old, `1` new; unified diffs
    /// have one side).
    fn line(&self, hunk: usize, row: usize, side: usize) -> Option<usize> {
        let index = self.start(hunk) + row;
        if self.split {
            let line = self.text.pairs.get(index)?[side];
            (line != NO_LINE).then_some(line as usize)
        } else {
            Some(index)
        }
    }
}

/// What the pointer is over in a text diff.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Header(usize),
    /// A line, and the row it is drawn in.
    Line(usize, usize, usize),
}

/// Top padding above the first card, the space below the last, between cards, and a card's
/// header height.
const CARD_TOP: f32 = 12.0;
const CARD_BOTTOM: f32 = 24.0;
const CARD_GAP: f32 = 16.0;
const CARD_MARGIN: f32 = 16.0;
const HUNK_HEADER: f32 = 36.0;
/// Width of the `+` / `−` column.
const SIGN_COLUMN: f32 = 24.0;

#[allow(clippy::too_many_lines)]
fn text_view(
    ui: &mut Ui,
    rect: Rect,
    text: &mut TextDiff,
    area: Area,
    split: bool,
    scroll_to: Option<f32>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let font = theme::mono();
    let number_font = FontId::new(11.5, FontFamily::Monospace);
    let char_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
    let digits = text
        .diff
        .lines
        .iter()
        .map(|line| line.old_no.max(line.new_no))
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    let number = (digits as f32 * char_width + 20.0).max(52.0);
    // Unified: old number, new number, sign. Split: number and sign on each side.
    let gutter = if split {
        number + SIGN_COLUMN
    } else {
        number * 2.0 + SIGN_COLUMN
    };
    let code_width = text.widest as f32 * char_width * 1.05 + space::XL;
    let cards = Cards {
        text: &*text,
        split,
    };
    let content_height = cards.height();
    let content_width = CARD_MARGIN * 2.0
        + if split {
            (gutter + code_width) * 2.0
        } else {
            gutter + code_width
        };

    let mut scroll = ScrollArea::both()
        .id_salt(("diff-text-scroll", split))
        .auto_shrink(false);
    if let Some(y) = scroll_to {
        scroll = scroll.scroll_offset(Vec2::new(0.0, y));
    }
    let _ = rect;
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(Vec2::new(
            content_width.max(viewport.width()),
            content_height.max(viewport.height()),
        ));
        let origin = ui.max_rect().min;
        let screen = viewport.translate(origin.to_vec2());
        // Cards stay put horizontally; only the code inside them scrolls.
        let scroll_x = viewport.min.x;
        let card_x = Rect::from_min_max(
            Pos2::new(screen.left() + CARD_MARGIN, 0.0),
            Pos2::new(
                (screen.right() - CARD_MARGIN).max(screen.left() + CARD_MARGIN + 160.0),
                0.0,
            ),
        )
        .x_range();
        let half = card_x.span() / 2.0;
        let hit = |text: &TextDiff, pos: Pos2| -> Option<Hit> {
            let cards = Cards { text, split };
            let y = pos.y - origin.y;
            let hunk = cards.hunk_at(y);
            if hunk >= cards.hunks() || y > cards.bottom(hunk) || !card_x.contains(pos.x) {
                return None;
            }
            let top = cards.top(hunk);
            if y < top {
                return None;
            }
            if y < top + HUNK_HEADER {
                return Some(Hit::Header(hunk));
            }
            let row = (((y - top - HUNK_HEADER) / LINE_HEIGHT) as usize)
                .min(cards.rows(hunk).saturating_sub(1));
            let side = usize::from(split && pos.x >= card_x.min + half);
            let line = cards.line(hunk, row, side).or_else(|| {
                // The blank side of a pair: the other side's line, if it is context.
                cards.line(hunk, row, 1 - side)
            })?;
            Some(Hit::Line(line, hunk, row))
        };

        let hovered = if text.partial && !text.diff.hunks.is_empty() {
            line_selection(ui, screen, text, hit)
        } else {
            None
        };
        let cards = Cards {
            text: &*text,
            split,
        };

        let painter = ui.painter();
        let decorations = text.decorations.clone();
        let corner = CornerRadius::same(radius::LAYER);
        let first = cards.hunk_at(viewport.min.y);
        let mut hunk = first;
        while hunk < cards.hunks() && cards.top(hunk) < viewport.max.y {
            let card = Rect::from_min_max(
                Pos2::new(card_x.min, origin.y + cards.top(hunk)),
                Pos2::new(card_x.max, origin.y + cards.bottom(hunk)),
            );
            painter.rect_filled(card, corner, palette.surface);
            let rows = cards.rows(hunk);
            let first_row = (((viewport.min.y - cards.top(hunk) - HUNK_HEADER) / LINE_HEIGHT)
                .floor()
                .max(0.0) as usize)
                .min(rows);
            let last_row = (((viewport.max.y - cards.top(hunk) - HUNK_HEADER) / LINE_HEIGHT)
                .ceil()
                .max(0.0) as usize)
                .min(rows);
            let sides: &[(usize, f32, f32)] = if split {
                &[(0, card_x.min, card_x.min + half), (1, card_x.min + half, card_x.max)]
            } else {
                &[(0, card_x.min, card_x.max)]
            };
            for row in first_row..last_row {
                let y = card.top() + HUNK_HEADER + row as f32 * LINE_HEIGHT;
                let last = row + 1 == rows;
                for &(side, left, right) in sides {
                    let cell = Rect::from_min_max(
                        Pos2::new(left, y),
                        Pos2::new(right, y + LINE_HEIGHT),
                    );
                    // Round the bottom corners of the card's last row.
                    let rounding = |left_edge: bool, right_edge: bool| CornerRadius {
                        sw: if last && left_edge { radius::LAYER - 1 } else { 0 },
                        se: if last && right_edge { radius::LAYER - 1 } else { 0 },
                        ..CornerRadius::ZERO
                    };
                    let left_edge = side == 0;
                    let right_edge = !split || side == 1;
                    let Some(index) = cards.line(hunk, row, side) else {
                        // The blank side of a pair.
                        painter.rect_filled(
                            cell,
                            rounding(left_edge, right_edge),
                            palette.surface_alt,
                        );
                        continue;
                    };
                    let line = &text.diff.lines[index];
                    let gutter_cell =
                        Rect::from_min_max(cell.min, Pos2::new(left + gutter, cell.bottom()));
                    if let Some(tint) = line_tint(line.kind, palette) {
                        painter.rect_filled(cell, rounding(left_edge, right_edge), tint);
                        // The gutter is a shade stronger.
                        painter.rect_filled(gutter_cell, rounding(left_edge, false), tint);
                    }
                    let selected = text.selected.contains(&(index as u32));
                    if selected {
                        painter.rect_filled(cell, rounding(left_edge, right_edge), palette.accent_soft);
                        painter.rect_filled(
                            Rect::from_min_size(cell.min, Vec2::new(SELECT_BAR, LINE_HEIGHT)),
                            0.0,
                            palette.accent,
                        );
                    } else if hovered == Some(index) && line.kind != LineKind::Context {
                        painter.rect_filled(cell, rounding(left_edge, right_edge), palette.hover);
                    }

                    // Numbers and sign.
                    let center = cell.center().y;
                    let (sign, sign_color) = match line.kind {
                        LineKind::Context => (' ', palette.text),
                        LineKind::Added => ('+', palette.success),
                        LineKind::Removed => ('−', palette.danger),
                    };
                    let numbers: &[(u32, f32)] = if split {
                        &[(
                            if side == 0 { line.old_no } else { line.new_no },
                            left + number,
                        )]
                    } else {
                        &[
                            (
                                if line.kind == LineKind::Added { 0 } else { line.old_no },
                                left + number,
                            ),
                            (
                                if line.kind == LineKind::Removed { 0 } else { line.new_no },
                                left + number * 2.0,
                            ),
                        ]
                    };
                    for &(no, right) in numbers {
                        if no > 0 {
                            painter.text(
                                Pos2::new(right - 10.0, center),
                                Align2::RIGHT_CENTER,
                                no.to_string(),
                                number_font.clone(),
                                palette.text_tertiary,
                            );
                        }
                    }
                    if sign != ' ' {
                        painter.text(
                            Pos2::new(left + gutter - SIGN_COLUMN / 2.0, center),
                            Align2::CENTER_CENTER,
                            sign,
                            font.clone(),
                            sign_color,
                        );
                    }

                    // The code, scrolled horizontally inside its column.
                    let code = Rect::from_min_max(
                        Pos2::new(left + gutter, cell.top()),
                        Pos2::new(right - 1.0, cell.bottom()),
                    );
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
                    painter
                        .with_clip_rect(code.intersect(painter.clip_rect()))
                        .galley(
                            Pos2::new(code.left() - scroll_x, center - galley.size().y / 2.0),
                            galley,
                            palette.text,
                        );
                }
            }
            if split {
                painter.vline(
                    card_x.min + half,
                    (card.top() + HUNK_HEADER)..=card.bottom(),
                    palette.hairline_stroke(),
                );
            }
            let header = Rect::from_min_size(card.min, Vec2::new(card.width(), HUNK_HEADER));
            hunk_header(ui, header, text, hunk, area, actions);
            painter.rect_stroke(
                card,
                corner,
                palette.hairline_stroke(),
                eframe::egui::StrokeKind::Inside,
            );
            hunk += 1;
        }
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
/// hunk header to (de)select its changed lines. Returns the line under the pointer.
fn line_selection(
    ui: &Ui,
    screen: Rect,
    text: &mut TextDiff,
    hit: impl Fn(&TextDiff, Pos2) -> Option<Hit>,
) -> Option<usize> {
    let response = ui.interact(
        screen,
        ui.scope_id().with("diff-lines"),
        Sense::click_and_drag(),
    );
    let line_at = |text: &TextDiff, hit: Hit| -> u32 {
        match hit {
            Hit::Line(line, ..) => line as u32,
            Hit::Header(hunk) => text.diff.hunks[hunk].lines.start,
        }
    };

    if response.drag_started() {
        if let Some(Hit::Line(line, ..)) = ui
            .input(|input| input.pointer.press_origin())
            .and_then(|pos| hit(text, pos))
        {
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
    if response.dragged() {
        if let (Some(drag), Some(pos)) = (text.drag.clone(), response.interact_pointer_pos()) {
            if let Some(target) = hit(text, pos) {
                let line = line_at(text, target);
                text.paint_selection(drag.anchor, line, drag.select, &drag.base);
            }
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
        match response
            .interact_pointer_pos()
            .and_then(|pos| hit(text, pos))
        {
            Some(Hit::Line(line, ..)) if text.is_changed_line(line) => {
                let line = line as u32;
                if !text.selected.remove(&line) {
                    text.selected.insert(line);
                }
            }
            Some(Hit::Header(hunk)) => {
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
            _ => {}
        }
    }

    let target = response.hover_pos().and_then(|pos| hit(text, pos))?;
    let pointable = match target {
        Hit::Line(line, ..) => text.is_changed_line(line),
        Hit::Header(_) => true,
    };
    if pointable {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    match target {
        Hit::Line(line, ..) => Some(line),
        Hit::Header(_) => None,
    }
}

/// A hunk card's header: a checkbox that stages or unstages the hunk, the `@@` line, and
/// Discard hunk.
fn hunk_header(
    ui: &Ui,
    rect: Rect,
    text: &TextDiff,
    hunk: usize,
    area: Area,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius {
            nw: radius::LAYER,
            ne: radius::LAYER,
            ..CornerRadius::ZERO
        },
        palette.surface_alt,
    );
    widgets::hairline_h(ui, rect.x_range().into(), rect.bottom() - 0.5);
    let center = rect.center().y;
    let mut left = rect.left() + 12.0;
    if text.partial {
        let staged = area == Area::Staged;
        let box_rect = Rect::from_center_size(Pos2::new(left + 8.0, center), Vec2::splat(16.0));
        let (tooltip, action) = if staged {
            ("Unstage this hunk", Action::UnstageHunk(hunk))
        } else {
            ("Stage this hunk", Action::StageHunk(hunk))
        };
        if widgets::check_box_at(
            ui,
            box_rect,
            Id::unique(("hunk-check", hunk)),
            if staged {
                widgets::Check::On
            } else {
                widgets::Check::Off
            },
            tooltip,
        )
        .clicked()
        {
            actions.push(action);
        }
        left = box_rect.right() + 10.0;
    }

    let mut right = rect.right() - 8.0;
    if text.partial && area == Area::Unstaged {
        let label = "Discard hunk";
        let width = text_button_width(ui, label);
        let button = Rect::from_min_max(
            Pos2::new(right - width, center - 13.0),
            Pos2::new(right, center + 13.0),
        );
        right -= width + space::XS;
        if text_button_at(
            ui,
            button,
            Id::unique(("hunk-discard", hunk)),
            label,
            "Discard this hunk (asks first; can be undone)",
            palette.text_secondary,
            false,
        )
        .clicked()
        {
            actions.push(Action::DiscardHunk(hunk));
        }
    }

    // `@@ −42,8 +42,13 @@ section`: the range dim, the section a little stronger.
    let header = String::from_utf8_lossy(text.diff.header_bytes(&text.diff.hunks[hunk]));
    let header = header.trim_end();
    let (range, section) = match header.get(2..).and_then(|rest| rest.find("@@")) {
        Some(end) => header.split_at(end + 4),
        None => (header, ""),
    };
    let range = range.replacen(" -", " −", 1);
    let font = FontId::new(12.0, FontFamily::Monospace);
    let clip = Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(right - space::S, rect.bottom()));
    let clipped = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
    let drawn = clipped.text(
        Pos2::new(left, center),
        Align2::LEFT_CENTER,
        range.trim_end(),
        font.clone(),
        palette.text_tertiary,
    );
    if !section.trim().is_empty() {
        clipped.text(
            Pos2::new(drawn.right() + char_gap(&font), center),
            Align2::LEFT_CENTER,
            section.trim(),
            font,
            palette.text_secondary,
        );
    }
}

fn char_gap(font: &FontId) -> f32 {
    font.size * 0.6
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
