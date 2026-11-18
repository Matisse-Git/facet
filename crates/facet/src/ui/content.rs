//! The content area: the Settings page while it is open; with no repository open, the welcome
//! view (open / clone / new, and the recent repositories); a working copy's Changes or History;
//! and for a bare repository, its overview (identity + HEAD).

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{
    Align, Align2, CornerRadius, Id, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    Vec2,
};
use facet_git::{Head, RepoInfo};

use super::widgets::{self, button, skeleton, text_line, ButtonKind};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, TabState};
use crate::app_add::AddTab;
use crate::format::{relative_time, unix_seconds};
use crate::theme::{self, icon, radius, space, Palette};

const COLUMN_MAX_WIDTH: f32 = 720.0;
const LABEL_WIDTH: f32 = 128.0;
const LINE_HEIGHT: f32 = 24.0;
const WELCOME_WIDTH: f32 = 640.0;
const TILE_HEIGHT: f32 = 128.0;
const RECENT_ROW: f32 = 60.0;
const MAX_RECENT: usize = 6;

impl FacetApp {
    pub(super) fn content(&mut self, ui: &mut Ui, rect: Rect) {
        if self.settings_view.is_some() {
            self.settings_page(ui, rect);
            return;
        }
        let working_copy = self.active_tab().is_some_and(
            |tab| matches!(&tab.state, TabState::Ready { info, .. } if !info.is_bare()),
        );
        if working_copy {
            self.work_view(ui, rect);
            return;
        }
        let mut actions = Vec::new();
        match self.active_tab().map(|tab| &tab.state) {
            None => welcome(ui, rect, &self.settings.recent_repos, &mut actions),
            Some(TabState::Loading) => overview_skeleton(ui, rect),
            Some(TabState::Ready { info, head }) => overview(ui, rect, info, head, &mut actions),
        }
        self.pending.extend(actions);
    }
}

fn welcome(ui: &mut Ui, rect: Rect, recent: &[PathBuf], actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let width = (rect.width() - space::XL * 2.0).clamp(0.0, WELCOME_WIDTH);
    let recent_shown = recent.len().min(MAX_RECENT);
    let recent_height = if recent_shown == 0 {
        0.0
    } else {
        space::XL + 24.0 + recent_shown as f32 * RECENT_ROW + 8.0
    };
    let height = 64.0
        + space::L
        + 36.0
        + space::S
        + 20.0
        + space::XL
        + TILE_HEIGHT
        + recent_height
        + space::L
        + 20.0;
    let top = (rect.center().y - height / 2.0).max(rect.top() + space::XL);
    let left = rect.center().x - width / 2.0;
    let center_x = rect.center().x;
    let mut y = top;

    // Mark, title, subtitle.
    widgets::app_mark(
        ui.painter(),
        Rect::from_center_size(Pos2::new(center_x, y + 32.0), Vec2::splat(64.0)),
        palette,
    );
    y += 64.0 + space::L;
    ui.painter().text(
        Pos2::new(center_x, y + 18.0),
        Align2::CENTER_CENTER,
        "Welcome to Facet",
        theme::title(),
        palette.text,
    );
    y += 36.0 + space::S;
    ui.painter().text(
        Pos2::new(center_x, y + 10.0),
        Align2::CENTER_CENTER,
        "A calm, fast Git client. Pick a repository to get started.",
        theme::body(),
        palette.text_secondary,
    );
    y += 20.0 + space::XL;

    // Open · Clone · New.
    let gap = space::M;
    let tile_width = (width - gap * 2.0) / 3.0;
    let open_tip = widgets::tooltip_with_shortcut(
        ui,
        "Open a folder that has a Git repository",
        &shortcut::OPEN,
    );
    for (index, (glyph, title, detail, tooltip, action)) in [
        (
            icon::FOLDER_OPEN,
            "Open folder",
            "A repository on this PC",
            open_tip.as_str(),
            Action::OpenRepository,
        ),
        (
            icon::DOWNLOAD,
            "Clone",
            "From GitHub or any URL",
            "Download a repository from a URL",
            Action::ShowAddRepository(Some(AddTab::Clone)),
        ),
        (
            icon::NEW_FOLDER,
            "New repository",
            "Start from an empty folder",
            "Create a new, empty repository",
            Action::ShowAddRepository(Some(AddTab::New)),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let tile = Rect::from_min_size(
            Pos2::new(left + index as f32 * (tile_width + gap), y),
            Vec2::new(tile_width, TILE_HEIGHT),
        );
        if welcome_tile(ui, tile, glyph, title, detail, palette)
            .on_hover_text(tooltip)
            .clicked()
        {
            actions.push(action);
        }
    }
    y += TILE_HEIGHT;

    // Recent repositories.
    if recent_shown > 0 {
        y += space::XL;
        ui.painter().text(
            Pos2::new(left, y + 12.0),
            Align2::LEFT_CENTER,
            "Recent",
            theme::strong(),
            palette.text,
        );
        y += 24.0 + 8.0;
        let card = Rect::from_min_size(
            Pos2::new(left, y),
            Vec2::new(width, recent_shown as f32 * RECENT_ROW),
        );
        ui.painter().rect(
            card,
            CornerRadius::same(radius::LAYER),
            palette.surface_alt,
            palette.hairline_stroke(),
            StrokeKind::Inside,
        );
        for (index, path) in recent.iter().take(MAX_RECENT).enumerate() {
            let row = Rect::from_min_size(
                Pos2::new(card.left(), card.top() + index as f32 * RECENT_ROW),
                Vec2::new(width, RECENT_ROW),
            );
            if index > 0 {
                widgets::hairline_h(
                    ui,
                    row.left() + space::M..=row.right() - space::M,
                    row.top(),
                );
            }
            let response = widgets::list_row(
                ui,
                row.shrink(3.0),
                Id::unique(("welcome-recent", index)),
                false,
            );
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let tile = Rect::from_min_size(
                Pos2::new(row.left() + space::M, row.center().y - 18.0),
                Vec2::splat(36.0),
            );
            widgets::monogram(ui.painter(), tile, &name, palette);
            let text_left = tile.right() + space::M - space::XS;
            let text_width = row.right() - space::XL - text_left;
            text_line(
                ui,
                Pos2::new(text_left, row.center().y - 9.0),
                &name,
                theme::strong(),
                palette.text,
                text_width,
            );
            text_line(
                ui,
                Pos2::new(text_left, row.center().y + 10.0),
                &path.display().to_string(),
                theme::small(),
                palette.text_tertiary,
                text_width,
            );
            if response.hovered() {
                ui.painter().text(
                    Pos2::new(row.right() - space::M - 6.0, row.center().y),
                    Align2::CENTER_CENTER,
                    icon::CHEVRON_RIGHT,
                    theme::icon_font(10.0),
                    palette.text_secondary,
                );
            }
            if response.clicked() {
                actions.push(Action::OpenPath(path.clone()));
            }
            response.context_menu(|ui| {
                if ui.button("Remove from Recent").clicked() {
                    actions.push(Action::ForgetRecent(path.clone()));
                }
            });
        }
        y = card.bottom();
    }

    // Hint.
    y += space::L;
    ui.painter().text(
        Pos2::new(center_x, y + 10.0),
        Align2::CENTER_CENTER,
        format!(
            "Drop a folder anywhere, or press {} for every command.",
            widgets::shortcut_text(ui, &shortcut::PALETTE)
        ),
        theme::small(),
        palette.text_tertiary,
    );
}

/// One of the welcome view's three big choices.
fn welcome_tile(
    ui: &Ui,
    rect: Rect,
    glyph: char,
    title: &str,
    detail: &str,
    palette: &Palette,
) -> eframe::egui::Response {
    let response = ui.interact(rect, Id::unique(("welcome-tile", title)), Sense::click());
    let hover = ui.ctx().animate_bool_with_time_and_easing(
        response.id.with("hover"),
        response.hovered(),
        theme::motion::FAST,
        theme::motion::ease_out,
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::LAYER),
        widgets::mix(palette.surface_alt, palette.raised, hover),
        Stroke::new(
            1.0,
            widgets::mix(palette.hairline, palette.text_tertiary, hover * 0.6),
        ),
        StrokeKind::Inside,
    );
    widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);
    let badge = Rect::from_min_size(rect.min + Vec2::splat(space::M), Vec2::splat(36.0));
    ui.painter().rect_filled(
        badge,
        CornerRadius::same(radius::LAYER),
        palette.accent_soft,
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(18.0),
        palette.accent,
    );
    let text_width = rect.width() - space::M * 2.0;
    text_line(
        ui,
        Pos2::new(rect.left() + space::M, badge.bottom() + space::M + 2.0),
        title,
        theme::strong(),
        palette.text,
        text_width,
    );
    text_line(
        ui,
        Pos2::new(rect.left() + space::M, badge.bottom() + space::M + 22.0),
        detail,
        theme::small(),
        palette.text_secondary,
        text_width,
    );
    response
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
