//! Facet's small set of custom-drawn controls. Everything is painted directly so it follows
//! the design system exactly: hairlines, one accent color, 120ms ease-out hover feedback, and a
//! visible focus ring for keyboard navigation.

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, Align2, Color32, CornerRadius, FontId, Galley, Id, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, TextFormat, Ui, Vec2,
};

use crate::theme::{self, motion, radius, space, Palette};

/// Linear blend between two premultiplied colors.
pub fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
        channel(from.a(), to.a()),
    )
}

/// Animated background for a hoverable/pressable control: transparent → hover → pressed.
pub fn interaction_fill(ui: &Ui, response: &Response, palette: &Palette) -> Color32 {
    let hover = ui.ctx().animate_bool_with_time_and_easing(
        response.id.with("hover"),
        response.hovered() || response.has_focus(),
        motion::FAST,
        motion::ease_out,
    );
    if response.is_pointer_button_down_on() {
        palette.pressed
    } else {
        mix(Color32::TRANSPARENT, palette.hover, hover)
    }
}

/// Accent outline shown while a control has keyboard focus.
pub fn focus_ring(ui: &Ui, response: &Response, rect: Rect, corner: u8, palette: &Palette) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(corner),
            Stroke::new(1.5, palette.accent),
            StrokeKind::Outside,
        );
    }
}

/// Lay out one line of text, ending in "…" if it does not fit `max_width`.
pub fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap.max_width = max_width.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

/// Paint one elided line with its left edge at `left_center`, vertically centered on it.
/// Returns the painted width.
pub fn text_line(
    ui: &Ui,
    left_center: Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> f32 {
    let galley = elided(ui, text, font, color, max_width);
    let size = galley.size();
    ui.painter().galley(
        Pos2::new(left_center.x, left_center.y - size.y / 2.0),
        galley,
        color,
    );
    size.x
}

/// A square, borderless icon button at a fixed rectangle.
pub fn icon_button_at(ui: &Ui, rect: Rect, id: Id, glyph: char, tooltip: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let fill = interaction_fill(ui, &response, palette);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);
    let color = if response.hovered() || response.has_focus() {
        palette.text
    } else {
        palette.text_secondary
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(rect.height().min(rect.width()) * 0.5),
        color,
    );
    focus_ring(ui, &response, rect, radius::M, palette);
    response.on_hover_text(tooltip)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Filled with the accent color: the one main action of a view.
    Primary,
    /// Hairline outline: secondary actions.
    Secondary,
}

/// A text button with an optional leading icon. `tooltip` should include the shortcut.
pub fn button(
    ui: &mut Ui,
    kind: ButtonKind,
    glyph: Option<char>,
    label: &str,
    tooltip: &str,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let font = theme::body();
    let text_color = match kind {
        ButtonKind::Primary => palette.on_accent,
        ButtonKind::Secondary => palette.text,
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, text_color);
    let icon_width = if glyph.is_some() {
        16.0 + space::S
    } else {
        0.0
    };
    let size = Vec2::new(
        galley.size().x + icon_width + space::M * 2.0,
        theme::ROW_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let corner = CornerRadius::same(radius::M);
    match kind {
        ButtonKind::Primary => {
            let hover = ui.ctx().animate_bool_with_time_and_easing(
                response.id.with("hover"),
                response.hovered(),
                motion::FAST,
                motion::ease_out,
            );
            let base = palette.accent;
            let lit = mix(base, Color32::WHITE, 0.12);
            let fill = if response.is_pointer_button_down_on() {
                mix(base, Color32::BLACK, 0.12)
            } else {
                mix(base, lit, hover)
            };
            ui.painter().rect_filled(rect, corner, fill);
        }
        ButtonKind::Secondary => {
            let fill = interaction_fill(ui, &response, palette);
            ui.painter().rect(
                rect,
                corner,
                fill,
                palette.hairline_stroke(),
                StrokeKind::Inside,
            );
        }
    }

    let mut x = rect.left() + space::M;
    if let Some(glyph) = glyph {
        ui.painter().text(
            Pos2::new(x + 8.0, rect.center().y),
            Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(14.0),
            text_color,
        );
        x += icon_width;
    }
    ui.painter().galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        text_color,
    );
    focus_ring(ui, &response, rect.expand(1.0), radius::M + 1, palette);
    response.on_hover_text(tooltip)
}

/// A pulsing placeholder block shown while content loads (instead of a spinner).
pub fn skeleton(ui: &Ui, rect: Rect) {
    let palette = Palette::of(ui.ctx());
    let time = ui.input(|input| input.time);
    let pulse = ((time * std::f64::consts::TAU / 1.4).sin() * 0.5 + 0.5) as f32;
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(radius::S),
        mix(palette.skeleton, palette.skeleton_highlight, pulse),
    );
    ui.ctx().request_repaint();
}

/// A horizontal hairline across `x_range` at `y`.
pub fn hairline_h(ui: &Ui, x_range: std::ops::RangeInclusive<f32>, y: f32) {
    let palette = Palette::of(ui.ctx());
    ui.painter().hline(x_range, y, palette.hairline_stroke());
}

/// Format a shortcut the way Windows users read it (`Ctrl+Shift+E`).
pub fn shortcut_text(ui: &Ui, shortcut: &egui::KeyboardShortcut) -> String {
    ui.ctx().format_shortcut(shortcut)
}

/// "Label (Ctrl+X)" for tooltips.
pub fn tooltip_with_shortcut(ui: &Ui, label: &str, shortcut: &egui::KeyboardShortcut) -> String {
    format!("{label}  ({})", shortcut_text(ui, shortcut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_blends_and_clamps() {
        let a = Color32::from_rgba_premultiplied(0, 0, 0, 0);
        let b = Color32::from_rgba_premultiplied(200, 100, 50, 250);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 7.0), b);
        assert_eq!(
            mix(a, b, 0.5),
            Color32::from_rgba_premultiplied(100, 50, 25, 125)
        );
    }
}
