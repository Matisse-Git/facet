//! The stash view: what a stash holds, laid out like the working copy — the stash and its
//! files on the left, the diff of the selected file beside it. Read-only; the stash is
//! applied, popped or dropped from its header.

use std::time::SystemTime;

use eframe::egui::{Align, CursorIcon, Id, Layout, Pos2, Rect, Sense, Ui, UiBuilder, Vec2};
use facet_git::parse::StashEntry;

use super::diff::{diff_body, plain_diff_header, HEADER_HEIGHT};
use super::history::{centered_message, file_list, FileItem, FileListKind};
use super::widgets::{self, button, skeleton, text_line, ButtonKind};
use super::{region, splitter_feedback, SPLITTER_GRAB};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::format::{relative_time, unix_seconds};
use crate::history::{StashBody, StashView};
use crate::settings::{ImageMode, Settings, CHANGES_WIDTH};
use crate::theme::{self, icon, space, Palette};
use crate::workspace::Area;

const MIN_DIFF_WIDTH: f32 = 320.0;
/// Message, selector line and the Pop / Apply / Drop buttons.
const STASH_HEADER: f32 = 108.0;
const SECTION_ROW: f32 = 28.0;

impl FacetApp {
    pub(super) fn stash_view(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let palette = Palette::of(ui.ctx());
        let Some(index) = self.active else {
            return;
        };
        // The same split as the working copy, so switching between them doesn't jump.
        let width = self
            .settings
            .changes_width
            .min((rect.width() - MIN_DIFF_WIDTH).max(*CHANGES_WIDTH.start()))
            .round();
        let left = Rect::from_min_max(rect.min, Pos2::new(rect.left() + width, rect.bottom()));
        let right = Rect::from_min_max(Pos2::new(left.right() + 1.0, rect.top()), rect.max);
        let mode = self.settings.image_mode;
        let view = &mut self.tabs[index].stash;

        region(ui, "stash", left, |ui| stash_panel(ui, left, view, actions));
        ui.painter().vline(
            left.right() + 0.5,
            rect.y_range(),
            palette.hairline_stroke(),
        );
        region(ui, "stash-diff", right, |ui| {
            stash_diff(ui, right, view, mode, actions);
        });

        // Splitter between the file list and the diff (shared with the working copy).
        let grab = Rect::from_min_max(
            Pos2::new(left.right() - SPLITTER_GRAB / 2.0, rect.top()),
            Pos2::new(left.right() + SPLITTER_GRAB / 2.0, rect.bottom()),
        );
        let response = ui.interact(grab, Id::unique("stash-splitter"), Sense::click_and_drag());
        if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            self.settings.changes_width = (pointer.x - rect.left())
                .clamp(*CHANGES_WIDTH.start(), *CHANGES_WIDTH.end())
                .round();
        }
        if response.double_clicked() {
            self.settings.changes_width = Settings::default().changes_width;
        }
        splitter_feedback(ui, &response, CursorIcon::ResizeHorizontal, |stroke| {
            ui.painter().vline(left.right(), rect.y_range(), stroke);
        });
    }
}

/// "On main: message" → ("main", "message"); "WIP on main: abc123 subject" likewise.
fn split_message(message: &str) -> (Option<&str>, &str) {
    match message.split_once(": ") {
        Some((prefix, rest)) => {
            let branch = prefix
                .strip_prefix("WIP on ")
                .or_else(|| prefix.strip_prefix("On "));
            (branch, rest)
        }
        None => (None, message),
    }
}

fn stash_panel(ui: &mut Ui, rect: Rect, view: &StashView, actions: &mut Vec<Action>) {
    let Some(entry) = &view.entry else {
        centered_message(ui, rect, "No stash selected", "Pick a stash in the sidebar");
        return;
    };
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), STASH_HEADER));
    stash_header(ui, header, entry, actions);
    widgets::hairline_h(ui, rect.x_range().into(), header.bottom());

    let palette = Palette::of(ui.ctx());
    let section = Rect::from_min_size(
        Pos2::new(rect.left(), header.bottom() + 1.0),
        Vec2::new(rect.width(), SECTION_ROW),
    );
    let list = Rect::from_min_max(Pos2::new(rect.left(), section.bottom()), rect.max);
    match &view.body {
        StashBody::Loading => {
            let mut y = list.top() + space::S;
            for width in [0.6, 0.45, 0.7, 0.35] {
                skeleton(
                    ui,
                    Rect::from_min_size(
                        Pos2::new(list.left() + space::M, y),
                        Vec2::new((list.width() - space::XL) * width, 12.0),
                    ),
                );
                y += 26.0;
            }
        }
        StashBody::Failed(message) => {
            centered_message(ui, list, "Couldn't read this stash", message);
        }
        StashBody::Loaded(files) => {
            let x = section.left() + space::M;
            let center = section.center().y;
            let width = text_line(
                ui,
                Pos2::new(x, center),
                "Files",
                theme::small_strong(),
                palette.text_secondary,
                section.width(),
            );
            text_line(
                ui,
                Pos2::new(x + width + space::S, center),
                &files.len().to_string(),
                theme::small(),
                palette.text_tertiary,
                section.width(),
            );
            if files.is_empty() {
                centered_message(ui, list, "No file changes", "This stash holds no changes");
                return;
            }
            region(ui, "stash-files", list, |ui| {
                file_list(
                    ui,
                    files.len(),
                    |index| {
                        let stashed = &files[index];
                        FileItem {
                            file: &stashed.file,
                            untracked: stashed.untracked,
                            rev: &stashed.new,
                        }
                    },
                    view.selected_file,
                    &FileListKind::STASH,
                    actions,
                );
            });
        }
    }
}

fn stash_header(ui: &mut Ui, rect: Rect, entry: &StashEntry, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let inner = rect.shrink2(Vec2::new(space::M, space::M));
    let (branch, message) = split_message(&entry.message);
    let room = inner.width();
    let mut y = inner.top() + 9.0;
    text_line(
        ui,
        Pos2::new(inner.left(), y),
        message,
        theme::strong(),
        palette.text,
        room,
    );
    y += 22.0;
    let now = unix_seconds(SystemTime::now());
    let mut detail = entry.selector.clone();
    if let Some(branch) = branch {
        detail.push_str(&format!(" · on {branch}"));
    }
    detail.push_str(&format!(" · {}", relative_time(now, entry.time)));
    let response = ui.interact(
        Rect::from_min_size(
            Pos2::new(inner.left(), y - 9.0),
            Vec2::new(
                text_line(
                    ui,
                    Pos2::new(inner.left(), y),
                    &detail,
                    theme::small(),
                    palette.text_tertiary,
                    room,
                ),
                18.0,
            ),
        ),
        Id::unique("stash-detail"),
        Sense::hover(),
    );
    response.on_hover_text(format!(
        "{}\n{}{}",
        entry.oid,
        entry.message,
        if entry.untracked {
            "\nIncludes untracked files"
        } else {
            ""
        }
    ));

    let buttons = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - theme::ROW_HEIGHT),
        inner.max,
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("stash-buttons")
            .max_rect(buttons)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            if button(
                ui,
                ButtonKind::Primary,
                Some(icon::CHECK),
                "Pop",
                "Apply this stash to the working copy and drop it",
            )
            .clicked()
            {
                actions.push(Action::PopStash(entry.index));
            }
            if button(
                ui,
                ButtonKind::Secondary,
                None,
                "Apply",
                "Apply this stash to the working copy and keep it",
            )
            .clicked()
            {
                actions.push(Action::ApplyStash(entry.index));
            }
            if button(
                ui,
                ButtonKind::Secondary,
                None,
                "Drop…",
                "Delete this stash (asks first; can be undone)",
            )
            .clicked()
            {
                actions.push(Action::DropStash(entry.index));
            }
        },
    );
}

fn stash_diff(
    ui: &mut Ui,
    rect: Rect,
    view: &mut StashView,
    mode: ImageMode,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    ui.painter().rect_filled(rect, 0.0, palette.surface);
    if view.diff.key.is_none() {
        if matches!(&view.body, StashBody::Loaded(files) if !files.is_empty()) {
            centered_message(ui, rect, "No file selected", "Pick a file in the list");
        }
        return;
    }
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT));
    plain_diff_header(ui, header, &view.diff);
    widgets::hairline_h(ui, rect.x_range().into(), header.bottom());
    let body = Rect::from_min_max(Pos2::new(rect.left(), header.bottom() + 1.0), rect.max);
    // Stash diffs are read-only: `area` is not used for actions.
    diff_body(ui, body, &mut view.diff, Area::Staged, mode, actions);
}

#[cfg(test)]
mod tests {
    use super::split_message;

    #[test]
    fn stash_messages_split_into_branch_and_text() {
        assert_eq!(split_message("On main: wip"), (Some("main"), "wip"));
        assert_eq!(
            split_message("WIP on feature/x: abc1234 Subject: with colon"),
            (Some("feature/x"), "abc1234 Subject: with colon")
        );
        assert_eq!(split_message("odd"), (None, "odd"));
    }
}
