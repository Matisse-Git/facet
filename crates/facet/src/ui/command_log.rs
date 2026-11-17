//! The command log panel: every git command Facet has run, exactly as executed, with its
//! duration and outcome. Rows are virtualized, so a full log costs the same as an empty one.

use eframe::egui::{
    Align2, CornerRadius, FontFamily, FontId, Id, Pos2, Rect, ScrollArea, Sense, Ui, UiBuilder,
    Vec2,
};
use facet_git::{CommandOutcome, CommandRecord};

use super::widgets::{self, icon_button_at, text_line};
use crate::actions::{shortcut, Action};
use crate::app::FacetApp;
use crate::format::{clock_time, duration_label};
use crate::platform;
use crate::theme::{self, icon, radius, space, Palette};

const HEADER_HEIGHT: f32 = 36.0;
const ROW_HEIGHT: f32 = 24.0;
const TIME_WIDTH: f32 = 64.0;
const DURATION_WIDTH: f32 = 72.0;

fn mono_small() -> FontId {
    FontId::new(12.0, FontFamily::Monospace)
}

impl FacetApp {
    pub(super) fn command_log_panel(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        widgets::hairline_h(ui, rect.x_range().into(), rect.top());

        // Refresh the snapshot only when the log actually changed.
        let revision = self.command_log.revision();
        if self.log_view.0 != revision {
            self.log_view = (revision, self.command_log.snapshot());
        }
        let count = self.log_view.1.len();

        // Header.
        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_HEIGHT));
        let title_width = text_line(
            ui,
            Pos2::new(header.left() + space::M, header.center().y),
            "Command Log",
            theme::small_strong(),
            palette.text_secondary,
            200.0,
        );
        if count > 0 {
            ui.painter().text(
                Pos2::new(
                    header.left() + space::M + title_width + space::S,
                    header.center().y,
                ),
                Align2::LEFT_CENTER,
                if count == 1 {
                    "1 command".to_owned()
                } else {
                    format!("{count} commands")
                },
                theme::small(),
                palette.text_tertiary,
            );
        }

        let close = Rect::from_center_size(
            Pos2::new(header.right() - space::S - 14.0, header.center().y),
            Vec2::splat(28.0),
        );
        let tooltip =
            widgets::tooltip_with_shortcut(ui, "Hide command log", &shortcut::COMMAND_LOG);
        if icon_button_at(
            ui,
            close,
            Id::unique("log-close"),
            icon::CHEVRON_DOWN,
            &tooltip,
        )
        .clicked()
        {
            self.pending.push(Action::ToggleCommandLog);
        }
        if count > 0 {
            let clear = close.translate(Vec2::new(-(28.0 + space::XS), 0.0));
            if icon_button_at(
                ui,
                clear,
                Id::unique("log-clear"),
                icon::DELETE,
                "Clear command log",
            )
            .clicked()
            {
                self.pending.push(Action::ClearCommandLog);
            }
        }

        // Rows.
        let list = Rect::from_min_max(Pos2::new(rect.left(), header.bottom()), rect.max);
        if count == 0 {
            ui.painter().text(
                list.center(),
                Align2::CENTER_CENTER,
                "Git commands appear here as they run.",
                theme::small(),
                palette.text_tertiary,
            );
            return;
        }

        let utc_offset = platform::local_utc_offset_seconds();
        let records = &self.log_view.1;
        let mut copy: Option<String> = None;
        ui.scope_builder(UiBuilder::new().id_salt("log-rows").max_rect(list), |ui| {
            ui.set_clip_rect(list.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing.y = 0.0;
            ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show_rows(ui, ROW_HEIGHT, count, |ui, range| {
                    for record in &records[range] {
                        if log_row(ui, palette, record, utc_offset) {
                            copy = Some(record.command_line.clone());
                        }
                    }
                });
        });
        if let Some(text) = copy {
            self.pending.push(Action::CopyText {
                text,
                what: "Command",
            });
        }
    }
}

/// Paint one log row. Returns `true` if it was clicked.
fn log_row(ui: &mut Ui, palette: &Palette, record: &CommandRecord, utc_offset: i64) -> bool {
    let (row, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(row) {
        return false;
    }
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            row.shrink2(Vec2::new(space::S, 0.0)),
            CornerRadius::same(radius::S),
            palette.hover,
        );
    }
    let center_y = row.center().y;
    let mut x = row.left() + space::M;

    ui.painter().text(
        Pos2::new(x, center_y),
        Align2::LEFT_CENTER,
        clock_time(record.started, utc_offset),
        mono_small(),
        palette.text_tertiary,
    );
    x += TIME_WIDTH + space::S;

    let (dot, failed) = match &record.outcome {
        CommandOutcome::Running => (palette.accent, false),
        CommandOutcome::Exited(0) => (palette.success, false),
        CommandOutcome::Cancelled => (palette.text_tertiary, false),
        CommandOutcome::Exited(_) | CommandOutcome::SpawnFailed(_) => (palette.danger, true),
    };
    ui.painter()
        .circle_filled(Pos2::new(x + 3.0, center_y), 3.0, dot);
    x += 6.0 + space::S;

    let duration = match (&record.outcome, record.duration) {
        (CommandOutcome::Running, _) => "running".to_owned(),
        (_, Some(duration)) => duration_label(duration),
        (_, None) => String::new(),
    };
    ui.painter().text(
        Pos2::new(x + DURATION_WIDTH, center_y),
        Align2::RIGHT_CENTER,
        duration,
        mono_small(),
        palette.text_secondary,
    );
    x += DURATION_WIDTH + space::M;

    text_line(
        ui,
        Pos2::new(x, center_y),
        &record.command_line,
        mono_small(),
        if failed { palette.danger } else { palette.text },
        row.right() - space::M - x,
    );

    let clicked = response.clicked();
    response.on_hover_ui(|ui| {
        ui.set_max_width(560.0);
        ui.label(record.command_line.as_str());
        if let Some(cwd) = &record.cwd {
            ui.label(format!("in {}", cwd.display()));
        }
        match &record.outcome {
            CommandOutcome::Running => {
                ui.label("Running…");
            }
            CommandOutcome::Exited(code) => {
                ui.label(format!("Exit code {code}"));
            }
            CommandOutcome::Cancelled => {
                ui.label("Cancelled");
            }
            CommandOutcome::SpawnFailed(error) => {
                ui.label(format!("Could not start: {error}"));
            }
        }
        if !record.stderr.is_empty() {
            ui.label(record.stderr.as_str());
        }
        ui.label("Click to copy the command");
    });
    clicked
}
