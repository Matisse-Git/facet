//! The confirmation dialog shown before a destructive action (discarding changes). It names
//! exactly what will happen, offers Cancel first, and Enter / Escape answer it.

use eframe::egui::{
    self, text::LayoutJob, Align, CornerRadius, Id, Key, Layout, Modifiers, Order, Pos2, Rect,
    Sense, StrokeKind, TextFormat, Ui, UiBuilder, Vec2,
};

use super::widgets::{button, ButtonKind};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::theme::{self, motion, radius, space, Palette};

const WIDTH: f32 = 420.0;

/// A question waiting for the user's answer.
pub struct Confirm {
    pub title: String,
    pub body: String,
    /// Label of the button that goes ahead ("Discard").
    pub confirm_label: String,
    /// What runs when the user confirms.
    pub action: Action,
}

impl FacetApp {
    pub(super) fn confirm_layer(&mut self, ui: &mut Ui, window: Rect) {
        let ctx = ui.ctx().clone();
        let visible = ctx.animate_bool_with_time_and_easing(
            Id::unique("confirm-visible"),
            self.confirm.is_some(),
            motion::NORMAL,
            motion::ease_out,
        );
        let Some(confirm) = &self.confirm else {
            return;
        };
        let palette = Palette::of(&ctx);

        let (mut accepted, mut cancelled) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::Enter),
                input.consume_key(Modifiers::NONE, Key::Escape),
            )
        });

        let width = WIDTH.min(window.width() - space::XL);
        let text_width = width - space::L * 2.0;
        let wrapped = |text: &str, font: egui::FontId, color: egui::Color32| {
            let mut job = LayoutJob::single_section(
                text.to_owned(),
                TextFormat {
                    font_id: font,
                    color,
                    ..Default::default()
                },
            );
            job.wrap.max_width = text_width;
            ctx.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let title = wrapped(&confirm.title, theme::strong(), palette.text);
        let body = wrapped(&confirm.body, theme::body(), palette.text_secondary);
        let height = space::L
            + title.size().y
            + space::S
            + body.size().y
            + space::L
            + theme::ROW_HEIGHT
            + space::M;
        let panel = Rect::from_min_size(
            Pos2::new(
                window.center().x - width / 2.0,
                window.top() + (window.height() * 0.24).max(theme::TITLE_BAR_HEIGHT + space::M)
                    - (1.0 - visible) * space::S,
            ),
            Vec2::new(width, height),
        );

        egui::Area::new(Id::unique("confirm"))
            .order(Order::Foreground)
            .fixed_pos(window.min)
            .show(&ctx, |ui| {
                ui.set_opacity(visible);

                // Scrim: dims the app and swallows clicks outside the dialog.
                let scrim = ui.allocate_rect(window, Sense::click());
                ui.painter().rect_filled(window, 0.0, palette.scrim);
                if scrim.clicked() {
                    cancelled = true;
                }

                let corner = CornerRadius::same(radius::L);
                ui.painter()
                    .add(ui.visuals().window_shadow.as_shape(panel, corner));
                ui.painter().rect(
                    panel,
                    corner,
                    palette.raised,
                    palette.hairline_stroke(),
                    StrokeKind::Outside,
                );
                ui.interact(panel, Id::unique("confirm-panel"), Sense::click());

                let mut y = panel.top() + space::L;
                let left = panel.left() + space::L;
                let title_height = title.size().y;
                ui.painter()
                    .galley(Pos2::new(left, y), title.clone(), palette.text);
                y += title_height + space::S;
                ui.painter()
                    .galley(Pos2::new(left, y), body.clone(), palette.text_secondary);

                let row = Rect::from_min_max(
                    Pos2::new(left, panel.bottom() - space::M - theme::ROW_HEIGHT),
                    Pos2::new(panel.right() - space::M, panel.bottom() - space::M),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .id_salt("confirm-buttons")
                        .max_rect(row)
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        if button(
                            ui,
                            ButtonKind::Danger,
                            None,
                            &confirm.confirm_label,
                            &format!("{}  (Enter)", confirm.confirm_label),
                        )
                        .clicked()
                        {
                            accepted = true;
                        }
                        if button(ui, ButtonKind::Secondary, None, "Cancel", "Cancel  (Esc)")
                            .clicked()
                        {
                            cancelled = true;
                        }
                    },
                );
            });

        if cancelled {
            self.confirm = None;
        } else if accepted {
            if let Some(confirm) = self.confirm.take() {
                self.pending.push(confirm.action);
            }
        }
    }
}
