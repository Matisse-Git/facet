//! The content area. With no repository open it shows the welcome view; with one open it shows
//! the repository overview (identity + HEAD). Status, diffs and history replace the overview in
//! later milestones.

use std::time::SystemTime;

use eframe::egui::{Align, Align2, Id, Layout, Pos2, Rect, RichText, Sense, Ui, UiBuilder, Vec2};
use facet_git::{Head, RepoInfo};

use super::widgets::{self, button, skeleton, text_line, ButtonKind};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, TabState};
use crate::format::{relative_time, unix_seconds};
use crate::theme::{self, icon, space, Palette};

const COLUMN_MAX_WIDTH: f32 = 720.0;
const LABEL_WIDTH: f32 = 128.0;
const LINE_HEIGHT: f32 = 24.0;

impl FacetApp {
    pub(super) fn content(&mut self, ui: &mut Ui, rect: Rect) {
        let mut actions = Vec::new();
        match self.active_tab().map(|tab| &tab.state) {
            None => welcome(ui, rect, &mut actions),
            Some(TabState::Loading) => overview_skeleton(ui, rect),
            Some(TabState::Ready { info, head }) => overview(ui, rect, info, head, &mut actions),
        }
        self.pending.extend(actions);
    }
}

fn welcome(ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let column = Rect::from_center_size(
        rect.center() - Vec2::new(0.0, space::L),
        Vec2::new((rect.width() - space::XL * 2.0).clamp(0.0, 440.0), 220.0),
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("welcome")
            .max_rect(column)
            .layout(Layout::top_down(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(
                RichText::new("Facet")
                    .font(theme::title())
                    .color(palette.text),
            );
            ui.add_space(space::S);
            ui.label(
                RichText::new("Open a repository to get started.")
                    .font(theme::body())
                    .color(palette.text_secondary),
            );
            ui.add_space(space::L);
            let tooltip = widgets::tooltip_with_shortcut(ui, "Open a repository", &shortcut::OPEN);
            if button(
                ui,
                ButtonKind::Primary,
                Some(icon::FOLDER_OPEN),
                "Open Repository…",
                &tooltip,
            )
            .clicked()
            {
                actions.push(Action::OpenRepository);
            }
            ui.add_space(space::M);
            ui.label(
                RichText::new(format!(
                    "Or drop a folder here.  Press {} for every command.",
                    widgets::shortcut_text(ui, &shortcut::PALETTE)
                ))
                .font(theme::small())
                .color(palette.text_tertiary),
            );
        },
    );
}

/// The padded column both the overview and its skeleton are laid out in.
fn column(rect: Rect) -> Rect {
    let inner = rect.shrink(space::XL);
    Rect::from_min_size(
        inner.min,
        Vec2::new(inner.width().min(COLUMN_MAX_WIDTH), inner.height()),
    )
}

fn overview(ui: &mut Ui, rect: Rect, info: &RepoInfo, head: &Head, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let column = column(rect);
    let left = column.left();
    let mut y = column.top();

    // Identity.
    let title_height = 36.0;
    text_line(
        ui,
        Pos2::new(left, y + title_height / 2.0),
        &info.name(),
        theme::title(),
        palette.text,
        column.width(),
    );
    y += title_height + space::XS;
    text_line(
        ui,
        Pos2::new(left, y + LINE_HEIGHT / 2.0),
        &info.root().display().to_string(),
        theme::small(),
        palette.text_secondary,
        column.width(),
    );
    y += LINE_HEIGHT + space::M;
    widgets::hairline_h(ui, left..=column.right(), y);
    y += space::M;

    // Key/value rows.
    let value_left = left + LABEL_WIDTH;
    let value_width = column.right() - value_left;
    let label = |ui: &Ui, y: f32, text: &str| {
        ui.painter().text(
            Pos2::new(left, y + LINE_HEIGHT / 2.0),
            Align2::LEFT_CENTER,
            text,
            theme::small(),
            palette.text_secondary,
        );
    };

    match head {
        Head::Branch { name, .. } => {
            label(ui, y, "Branch");
            text_line(
                ui,
                Pos2::new(value_left, y + LINE_HEIGHT / 2.0),
                name,
                theme::strong(),
                palette.text,
                value_width,
            );
        }
        Head::Detached { .. } => {
            label(ui, y, "Branch");
            text_line(
                ui,
                Pos2::new(value_left, y + LINE_HEIGHT / 2.0),
                "Detached HEAD",
                theme::strong(),
                palette.warning,
                value_width,
            );
        }
    }
    y += LINE_HEIGHT + space::S;

    label(ui, y, "Latest commit");
    match head.commit() {
        Some(commit) => {
            // The short id copies the full id when clicked.
            let id_galley = ui.painter().layout_no_wrap(
                commit.short_oid.clone(),
                theme::mono(),
                palette.accent,
            );
            let id_rect = Rect::from_min_size(
                Pos2::new(value_left, y + (LINE_HEIGHT - id_galley.size().y) / 2.0),
                id_galley.size(),
            );
            let response = ui.interact(
                id_rect.expand2(Vec2::new(space::XS, 2.0)),
                Id::unique("overview-commit-id"),
                Sense::click(),
            );
            if response.hovered() || response.has_focus() {
                ui.painter()
                    .rect_filled(response.rect, theme::radius::S, palette.accent_soft);
                ui.ctx()
                    .set_cursor_icon(eframe::egui::CursorIcon::PointingHand);
            }
            ui.painter().galley(id_rect.min, id_galley, palette.accent);
            if response.clicked() {
                actions.push(Action::CopyText {
                    text: commit.oid.clone(),
                    what: "Commit ID",
                });
            }
            response.on_hover_text(format!("{}\nClick to copy", commit.oid));

            let subject_left = id_rect.right() + space::M - space::XS;
            text_line(
                ui,
                Pos2::new(subject_left, y + LINE_HEIGHT / 2.0),
                &commit.subject,
                theme::body(),
                palette.text,
                column.right() - subject_left,
            );
            y += LINE_HEIGHT;
            let when = relative_time(unix_seconds(SystemTime::now()), commit.author_time);
            text_line(
                ui,
                Pos2::new(value_left, y + LINE_HEIGHT / 2.0),
                &format!("{}  ·  {when}", commit.author),
                theme::small(),
                palette.text_secondary,
                value_width,
            );
        }
        None => {
            text_line(
                ui,
                Pos2::new(value_left, y + LINE_HEIGHT / 2.0),
                "No commits yet",
                theme::body(),
                palette.text_secondary,
                value_width,
            );
        }
    }
    y += LINE_HEIGHT + space::S;

    if info.is_bare() {
        label(ui, y, "Type");
        text_line(
            ui,
            Pos2::new(value_left, y + LINE_HEIGHT / 2.0),
            "Bare repository (no working tree)",
            theme::body(),
            palette.text,
            value_width,
        );
        y += LINE_HEIGHT + space::S;
    }

    // Actions.
    y += space::M;
    let row = Rect::from_min_max(
        Pos2::new(left, y),
        Pos2::new(column.right(), y + theme::ROW_HEIGHT),
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("overview-actions")
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            let tooltip =
                widgets::tooltip_with_shortcut(ui, "Open in File Explorer", &shortcut::REVEAL);
            if button(
                ui,
                ButtonKind::Secondary,
                Some(icon::FOLDER),
                "Open in Explorer",
                &tooltip,
            )
            .clicked()
            {
                actions.push(Action::RevealInExplorer);
            }
            let tooltip =
                widgets::tooltip_with_shortcut(ui, "Copy repository path", &shortcut::COPY_PATH);
            if button(
                ui,
                ButtonKind::Secondary,
                Some(icon::COPY),
                "Copy Path",
                &tooltip,
            )
            .clicked()
            {
                actions.push(Action::CopyRepositoryPath);
            }
            let tooltip = widgets::tooltip_with_shortcut(ui, "Refresh", &shortcut::REFRESH);
            if button(
                ui,
                ButtonKind::Secondary,
                Some(icon::REFRESH),
                "Refresh",
                &tooltip,
            )
            .clicked()
            {
                actions.push(Action::Refresh);
            }
        },
    );
}

/// Placeholder with the same geometry as [`overview`], shown while a repository loads.
fn overview_skeleton(ui: &mut Ui, rect: Rect) {
    let column = column(rect);
    let left = column.left();
    let mut y = column.top();
    let block = |ui: &Ui, x: f32, y: f32, width: f32, height: f32| {
        skeleton(
            ui,
            Rect::from_min_size(
                Pos2::new(x, y),
                Vec2::new(width.min(column.width()), height),
            ),
        );
    };

    block(ui, left, y + 4.0, 260.0, 28.0);
    y += 36.0 + space::XS;
    block(ui, left, y + 6.0, 380.0, 12.0);
    y += LINE_HEIGHT + space::M;
    widgets::hairline_h(ui, left..=column.right(), y);
    y += space::M;

    let value_left = left + LABEL_WIDTH;
    block(ui, left, y + 6.0, 56.0, 12.0);
    block(ui, value_left, y + 5.0, 160.0, 14.0);
    y += LINE_HEIGHT + space::S;
    block(ui, left, y + 6.0, 88.0, 12.0);
    block(ui, value_left, y + 5.0, 340.0, 14.0);
    y += LINE_HEIGHT;
    block(ui, value_left, y + 6.0, 200.0, 12.0);
}
