//! Views. Each file adds an `impl FacetApp` block for one region of the window; this module
//! owns the overall layout (title bar · sidebar · content · command log) and the splitters.
//!
//! Layout is computed by hand from rectangles rather than egui panels so that every edge sits
//! exactly on the design grid and the chrome regions can stay transparent over Mica.

mod add_repo;
mod changes;
mod command_log;
pub mod confirm;
mod content;
mod diff;
mod history;
pub use diff::LINE_HEIGHT as DIFF_LINE;
pub use history::commit_row_height;
mod image_diff;
pub mod palette;
mod power;
pub mod prompt;
mod settings;
pub(crate) use settings::version_label;
mod sidebar;
mod stash;
mod titlebar;
mod vector;
pub mod toast;
pub mod widgets;

use eframe::egui::{
    self, Align, Align2, CornerRadius, CursorIcon, Id, Layout, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder,
};

use crate::app::FacetApp;
use crate::settings::{Settings, COMMAND_LOG_HEIGHT, SIDEBAR_WIDTH};
use crate::theme::{self, motion, radius, space, Palette, TITLE_BAR_HEIGHT};

/// Minimum height kept for the content area when the command log is resized.
const MIN_CONTENT_HEIGHT: f32 = 160.0;
/// Width of the invisible grab area around a splitter.
pub(super) const SPLITTER_GRAB: f32 = 6.0;

/// Build a child `Ui` confined (and clipped) to `rect`, laid out top-down.
pub fn region<R>(
    ui: &mut Ui,
    salt: &'static str,
    rect: Rect,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(salt)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            add_contents(ui)
        },
    )
    .inner
}

impl FacetApp {
    pub(crate) fn layout(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let palette = Palette::of(&ctx);
        let window = ui.max_rect();

        let title = Rect::from_min_max(
            window.min,
            Pos2::new(window.right(), window.top() + TITLE_BAR_HEIGHT),
        );
        let body = Rect::from_min_max(Pos2::new(window.left(), title.bottom()), window.max);

        // Animated open/closed fractions for the two collapsible regions.
        let sidebar_open = ctx.animate_bool_with_time_and_easing(
            Id::unique("sidebar-open"),
            // No repository open: the welcome view has the whole window.
            self.settings.sidebar_visible && !self.tabs.is_empty(),
            motion::NORMAL,
            motion::ease_out,
        );
        let log_open = ctx.animate_bool_with_time_and_easing(
            Id::unique("command-log-open"),
            self.settings.command_log_visible,
            motion::NORMAL,
            motion::ease_out,
        );

        let sidebar_full = self
            .settings
            .sidebar_width
            .min((body.width() - 240.0).max(*SIDEBAR_WIDTH.start()));
        let sidebar_width = (sidebar_full * sidebar_open).round();
        let log_full = self
            .settings
            .command_log_height
            .min((body.height() - MIN_CONTENT_HEIGHT).max(*COMMAND_LOG_HEIGHT.start()));
        let log_height = (log_full * log_open).round();

        let sidebar = Rect::from_min_max(
            body.min,
            Pos2::new(body.left() + sidebar_width, body.bottom()),
        );
        let main = Rect::from_min_max(Pos2::new(sidebar.right(), body.top()), body.max);
        let log = Rect::from_min_max(Pos2::new(main.left(), main.bottom() - log_height), main.max);
        let content = Rect::from_min_max(main.min, Pos2::new(main.right(), log.top()));

        // The content surface: the only opaque area. Title bar and sidebar stay Mica.
        // Extending it one point past the right/bottom window edges hides those two strokes.
        let corner = if sidebar_width > 0.5 {
            CornerRadius {
                nw: radius::LAYER,
                ..CornerRadius::ZERO
            }
        } else {
            CornerRadius::ZERO
        };
        let surface = Rect::from_min_max(
            Pos2::new(
                main.left() - if sidebar_width > 0.5 { 0.0 } else { 1.0 },
                main.top(),
            ),
            main.max + egui::vec2(1.0, 1.0),
        );
        ui.painter().rect(
            surface,
            corner,
            if self.settings_view.is_some() {
                palette.chrome
            } else {
                palette.surface
            },
            palette.hairline_stroke(),
            StrokeKind::Inside,
        );

        diff::set_split(self.settings.split_diff);
        self.title_bar(ui, title);
        if sidebar_width > 1.0 {
            // Slide the full-width sidebar in from the left while it animates.
            let full = Rect::from_min_max(
                Pos2::new(sidebar.right() - sidebar_full, sidebar.top()),
                sidebar.max,
            );
            region(ui, "sidebar", sidebar, |ui| self.sidebar(ui, full));
        }
        region(ui, "content", content.shrink2(egui::vec2(1.0, 1.0)), |ui| {
            self.content(ui, content);
        });
        if log_height > 1.0 {
            let full = Rect::from_min_size(log.min, egui::vec2(log.width(), log_full));
            region(ui, "command-log", log, |ui| {
                self.command_log_panel(ui, full)
            });
        }

        // Splitters go last so they sit above the views they resize.
        if self.settings.sidebar_visible && !self.tabs.is_empty() && sidebar_open >= 1.0 {
            let grab = Rect::from_min_max(
                Pos2::new(
                    sidebar.right() - SPLITTER_GRAB / 2.0,
                    body.top() + f32::from(radius::LAYER),
                ),
                Pos2::new(sidebar.right() + SPLITTER_GRAB / 2.0, body.bottom()),
            );
            let response = ui.interact(
                grab,
                Id::unique("sidebar-splitter"),
                Sense::click_and_drag(),
            );
            if let Some(pointer) = response
                .interact_pointer_pos()
                .filter(|_| response.dragged())
            {
                self.settings.sidebar_width = (pointer.x - body.left())
                    .clamp(*SIDEBAR_WIDTH.start(), *SIDEBAR_WIDTH.end())
                    .round();
            }
            if response.double_clicked() {
                self.settings.sidebar_width = Settings::default().sidebar_width;
            }
            splitter_feedback(ui, &response, CursorIcon::ResizeHorizontal, |stroke| {
                ui.painter().vline(sidebar.right(), grab.y_range(), stroke);
            });
        }
        if self.settings.command_log_visible && log_open >= 1.0 {
            let grab = Rect::from_min_max(
                Pos2::new(log.left(), log.top() - SPLITTER_GRAB / 2.0),
                Pos2::new(log.right(), log.top() + SPLITTER_GRAB / 2.0),
            );
            let response = ui.interact(
                grab,
                Id::unique("command-log-splitter"),
                Sense::click_and_drag(),
            );
            if let Some(pointer) = response
                .interact_pointer_pos()
                .filter(|_| response.dragged())
            {
                self.settings.command_log_height = (main.bottom() - pointer.y)
                    .clamp(*COMMAND_LOG_HEIGHT.start(), *COMMAND_LOG_HEIGHT.end())
                    .round();
            }
            if response.double_clicked() {
                self.settings.command_log_height = Settings::default().command_log_height;
            }
            splitter_feedback(ui, &response, CursorIcon::ResizeVertical, |stroke| {
                ui.painter().hline(grab.x_range(), log.top(), stroke);
            });
        }

        self.drop_overlay(ui, content);
        self.toast_layer(ui, content);
        self.palette_layer(ui, window);
        self.rebase_layer(ui, window);
        self.conflict_layer(ui, window);
        self.add_repo_layer(ui, window);
        self.confirm_layer(ui, window);
        self.prompt_layer(ui, window);
    }

    /// Highlight the content area while files are dragged over the window.
    fn drop_overlay(&self, ui: &Ui, content: Rect) {
        let hovering = ui.input(|input| !input.raw.hovered_files.is_empty());
        let opacity = ui.ctx().animate_bool_with_time_and_easing(
            Id::unique("drop-overlay"),
            hovering,
            motion::FAST,
            motion::ease_out,
        );
        if opacity <= 0.0 {
            return;
        }
        let palette = Palette::of(ui.ctx());
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::unique("drop-overlay-layer"),
        ));
        let target = content.shrink(space::M);
        painter.rect(
            target,
            CornerRadius::same(radius::L),
            theme::fade(palette.accent_soft, opacity),
            Stroke::new(1.5, theme::fade(palette.accent, opacity)),
            StrokeKind::Inside,
        );
        painter.text(
            target.center(),
            Align2::CENTER_CENTER,
            "Drop a folder to open its repository",
            theme::strong(),
            theme::fade(palette.text, opacity),
        );
    }
}

/// Shared hover/drag behavior for splitters: resize cursor plus an accent line.
pub(super) fn splitter_feedback(
    ui: &Ui,
    response: &egui::Response,
    cursor: CursorIcon,
    paint_line: impl FnOnce(Stroke),
) {
    let active = response.hovered() || response.dragged();
    if active {
        ui.ctx().set_cursor_icon(cursor);
    }
    let strength = ui.ctx().animate_bool_with_time_and_easing(
        response.id.with("active"),
        active,
        motion::FAST,
        motion::ease_out,
    );
    if strength > 0.0 {
        let palette = Palette::of(ui.ctx());
        paint_line(Stroke::new(2.0, theme::fade(palette.accent, strength)));
    }
}
