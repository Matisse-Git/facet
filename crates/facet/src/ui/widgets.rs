//! Facet's small set of custom-drawn controls. Everything is painted directly so it follows
//! the design system exactly: hairlines, one accent color, 120ms ease-out hover feedback, and a
//! visible focus ring for keyboard navigation.

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, Align2, Color32, CornerRadius, FontId, Frame, Galley, Id, Margin,
    Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextEdit, TextFormat, Ui,
    Vec2,
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
    /// Filled with the danger color: confirms a destructive action.
    Danger,
}

/// A text button with an optional leading icon. `tooltip` should include the shortcut.
pub fn button(
    ui: &mut Ui,
    kind: ButtonKind,
    glyph: Option<char>,
    label: &str,
    tooltip: &str,
) -> Response {
    button_enabled(ui, kind, glyph, label, tooltip, true)
}

/// [`button`] that can be unavailable: it is dimmed, does not react, and its tooltip should
/// say what is missing.
pub fn button_enabled(
    ui: &mut Ui,
    kind: ButtonKind,
    glyph: Option<char>,
    label: &str,
    tooltip: &str,
    enabled: bool,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let font = theme::body();
    let text_color = match kind {
        ButtonKind::Primary | ButtonKind::Danger if enabled => palette.on_accent,
        ButtonKind::Secondary if enabled => palette.text,
        _ => palette.text_tertiary,
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
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let corner = CornerRadius::same(radius::M);
    match kind {
        ButtonKind::Primary | ButtonKind::Danger if !enabled => {
            ui.painter().rect_filled(rect, corner, palette.pressed);
        }
        ButtonKind::Primary | ButtonKind::Danger => {
            let hover = ui.ctx().animate_bool_with_time_and_easing(
                response.id.with("hover"),
                response.hovered(),
                motion::FAST,
                motion::ease_out,
            );
            let base = if kind == ButtonKind::Danger {
                palette.danger
            } else {
                palette.accent
            };
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

/// A compact text button at a fixed rectangle (hunk actions, toast actions, segments).
/// `active` draws it as the chosen option of a group.
pub fn text_button_at(
    ui: &Ui,
    rect: Rect,
    id: Id,
    label: &str,
    tooltip: &str,
    color: Color32,
    active: bool,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let fill = if active {
        palette.pressed
    } else {
        interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::S), fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::small_strong(),
        color,
    );
    focus_ring(ui, &response, rect, radius::S, palette);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// Width [`text_button_at`] needs for `label`.
pub fn text_button_width(ui: &Ui, label: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), theme::small_strong(), Color32::WHITE)
        .size()
        .x
        + space::S * 2.0
}

/// The state a [`check_box_at`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    On,
    Off,
    /// Some but not all (a partly staged file, some files of a list).
    Mixed,
}

/// A check mark (the design's `m5 12 5 5 9-10` path) filling `rect`.
pub fn check_mark(ui: &Ui, rect: Rect, color: Color32, width: f32) {
    let to = |x: f32, y: f32| rect.min + Vec2::new(x, y) * (rect.width() / 24.0);
    ui.painter().add(egui::Shape::line(
        vec![to(5.0, 12.0), to(10.0, 17.0), to(19.0, 7.0)],
        Stroke::new(width, color),
    ));
}

/// A bare checkbox filling `rect` (16 or 18 pt): accent with a check or a dash when on or
/// mixed, an outline when off. `clicked()` means "toggle".
pub fn check_box_at(ui: &Ui, rect: Rect, id: Id, state: Check, tooltip: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect.expand(3.0), id, Sense::click());
    let corner = CornerRadius::same(radius::S);
    let size = rect.width();
    let to = |x: f32, y: f32| rect.min + Vec2::new(x, y) * (size / 24.0);
    match state {
        Check::On | Check::Mixed => {
            let fill = if response.hovered() {
                mix(palette.accent, palette.text, 0.1)
            } else {
                palette.accent
            };
            ui.painter().rect_filled(rect, corner, fill);
            let stroke = Stroke::new(size / 24.0 * 3.2, palette.on_accent);
            if state == Check::On {
                ui.painter().add(egui::Shape::line(
                    vec![to(6.0, 12.5), to(10.5, 17.0), to(18.5, 7.5)],
                    stroke,
                ));
            } else {
                ui.painter().line_segment([to(6.5, 12.0), to(17.5, 12.0)], stroke);
            }
        }
        Check::Off => {
            let color = if response.hovered() {
                palette.text_secondary
            } else {
                palette.text_tertiary
            };
            ui.painter().rect(
                rect,
                corner,
                interaction_fill(ui, &response, palette),
                Stroke::new(1.5, color),
                StrokeKind::Inside,
            );
        }
    }
    focus_ring(ui, &response, rect.expand(1.0), radius::S + 1, palette);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// A checkbox with a label. Returns the response; `clicked()` means "toggle".
pub fn checkbox(ui: &mut Ui, checked: bool, label: &str, tooltip: &str, enabled: bool) -> Response {
    let palette = Palette::of(ui.ctx());
    let text_color = if enabled {
        palette.text
    } else {
        palette.text_tertiary
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::body(), text_color);
    let box_size = 16.0;
    let size = Vec2::new(
        box_size + space::S + galley.size().x + space::XS,
        theme::ROW_HEIGHT,
    );
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let square = Rect::from_center_size(
        Pos2::new(rect.left() + box_size / 2.0, rect.center().y),
        Vec2::splat(box_size),
    );
    let corner = CornerRadius::same(radius::S);
    if checked {
        ui.painter().rect_filled(
            square,
            corner,
            if enabled {
                palette.accent
            } else {
                palette.text_tertiary
            },
        );
        ui.painter().text(
            square.center(),
            Align2::CENTER_CENTER,
            theme::icon::CHECK,
            theme::icon_font(11.0),
            palette.on_accent,
        );
    } else {
        let stroke_color = if response.hovered() {
            palette.text_secondary
        } else {
            palette.text_tertiary
        };
        ui.painter().rect(
            square,
            corner,
            interaction_fill(ui, &response, palette),
            Stroke::new(1.0, stroke_color),
            StrokeKind::Inside,
        );
    }
    ui.painter().galley(
        Pos2::new(
            square.right() + space::S,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text_color,
    );
    focus_ring(ui, &response, rect.expand(1.0), radius::S, palette);
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

/// Facet's mark: an accent tile with the facet diamond, as in the title bar and welcome view.
pub fn app_mark(painter: &Painter, rect: Rect, palette: &Palette) {
    let corner = (rect.width() * 0.28).round().clamp(2.0, 255.0) as u8;
    painter.rect_filled(rect, CornerRadius::same(corner), palette.accent);
    let center = rect.center();
    let r = rect.width() * 0.3;
    let stroke = Stroke::new((rect.width() / 11.0).max(1.2), palette.on_accent);
    painter.add(Shape::closed_line(
        vec![
            center + Vec2::new(0.0, -r),
            center + Vec2::new(r, 0.0),
            center + Vec2::new(0.0, r),
            center + Vec2::new(-r, 0.0),
        ],
        stroke,
    ));
    painter.line_segment(
        [center - Vec2::new(r, 0.0), center + Vec2::new(r, 0.0)],
        stroke,
    );
}

/// The three tile hues repositories and people cycle through (accent, orange, green).
pub fn monogram_color(palette: &Palette, name: &str) -> Color32 {
    let hues = [palette.accent, palette.orange, palette.success];
    let hash = name
        .bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
    hues[hash % hues.len()]
}

/// A rounded letter tile that tells repositories apart at a glance.
pub fn monogram(painter: &Painter, rect: Rect, name: &str, palette: &Palette) {
    let color = monogram_color(palette, name);
    painter.rect_filled(
        rect,
        CornerRadius::same(radius::LAYER),
        color.gamma_multiply(if palette.dark { 0.2 } else { 0.12 }),
    );
    let letter: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "·".to_owned());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId::new((rect.height() * 0.42).round(), theme::strong().family),
        color,
    );
}

/// A round letter avatar for a person, tinted by their name.
pub fn avatar(painter: &Painter, center: Pos2, diameter: f32, name: &str, palette: &Palette) {
    let color = monogram_color(palette, name);
    painter.circle_filled(center, diameter / 2.0, palette.soft(color));
    let letter: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "·".to_owned());
    painter.text(
        center,
        Align2::CENTER_CENTER,
        letter,
        FontId::new((diameter * 0.46).round(), theme::strong().family),
        color,
    );
}

/// The colors of a file status letter (`M`, `A`, `D`, `R`, `U`…): (letter color).
pub fn status_color(status: char, palette: &Palette) -> Color32 {
    match status {
        'A' | 'U' | '?' => palette.success,
        'D' => palette.danger,
        'R' | 'C' => palette.accent,
        '!' => palette.warning,
        _ => palette.warning,
    }
}

/// A file's status as a small tinted square with its letter, centered at `center`.
pub fn status_badge(painter: &Painter, center: Pos2, status: char, palette: &Palette) {
    let color = status_color(status, palette);
    let rect = Rect::from_center_size(center, Vec2::splat(18.0));
    painter.rect_filled(rect, CornerRadius::same(radius::S), palette.soft(color));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        status,
        FontId::new(10.5, theme::strong().family),
        color,
    );
}

/// Five blocks showing how big a change is and how much of it adds or removes lines, with
/// its right edge at `right_center`. Returns its width.
pub fn change_bar(
    painter: &Painter,
    right_center: Pos2,
    added: usize,
    removed: usize,
    palette: &Palette,
) -> f32 {
    const BLOCKS: usize = 5;
    let total = added + removed;
    let colored = match total {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=4 => 3,
        5..=19 => 4,
        _ => 5,
    };
    let mut green = (colored * added).checked_div(total).unwrap_or(0);
    if added > 0 {
        green = green.max(1);
    }
    let red = if removed > 0 {
        (colored - green.min(colored)).max(1)
    } else {
        0
    };
    let green = green.min(colored - red.min(colored));
    let width = BLOCKS as f32 * 6.0 + (BLOCKS - 1) as f32 * 2.0;
    let left = right_center.x - width;
    for block in 0..BLOCKS {
        let color = if block < green {
            palette.success
        } else if block < green + red {
            palette.danger
        } else {
            palette.hairline
        };
        let rect = Rect::from_min_size(
            Pos2::new(left + block as f32 * 8.0, right_center.y - 5.0),
            Vec2::new(6.0, 10.0),
        );
        painter.rect_filled(rect, CornerRadius::same(1), color);
    }
    width
}

/// Width [`outline_button_at`] needs.
pub fn outline_button_width(ui: &Ui, glyph: Option<char>, label: &str) -> f32 {
    let text = if label.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::body(), Color32::WHITE)
            .size()
            .x
    };
    match (glyph.is_some(), label.is_empty()) {
        (true, true) => 32.0,
        (true, false) => 12.0 + 14.0 + space::S + text + 12.0,
        _ => 14.0 + text + 14.0,
    }
}

/// The design's secondary button: surface fill, hairline outline, 8 px corners, an optional
/// icon. `label` may be empty for a square icon button.
pub fn outline_button_at(
    ui: &Ui,
    rect: Rect,
    id: Id,
    glyph: Option<char>,
    label: &str,
    tooltip: &str,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let corner = CornerRadius::same(radius::LAYER);
    ui.painter().rect(
        rect,
        corner,
        palette.surface,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    let fill = interaction_fill(ui, &response, palette);
    ui.painter().rect_filled(rect.shrink(1.0), corner, fill);
    let center = rect.center().y;
    match (glyph, label.is_empty()) {
        (Some(glyph), true) => {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                glyph,
                theme::icon_font(14.0),
                palette.text,
            );
        }
        (glyph, _) => {
            let mut x = rect.left() + if glyph.is_some() { 12.0 } else { 14.0 };
            if let Some(glyph) = glyph {
                ui.painter().text(
                    Pos2::new(x + 7.0, center),
                    Align2::CENTER_CENTER,
                    glyph,
                    theme::icon_font(14.0),
                    palette.text,
                );
                x += 14.0 + space::S;
            }
            ui.painter().text(
                Pos2::new(x, center),
                Align2::LEFT_CENTER,
                label,
                theme::body(),
                palette.text,
            );
        }
    }
    focus_ring(ui, &response, rect, radius::LAYER, palette);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// The design's accent button at a fixed rectangle: accent fill, an optional icon and a
/// semibold label, centered.
pub fn primary_button_at(
    ui: &Ui,
    rect: Rect,
    id: Id,
    glyph: Option<char>,
    label: &str,
    enabled: bool,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let corner = CornerRadius::same(radius::LAYER);
    let fill = if !enabled {
        palette.hairline
    } else if response.is_pointer_button_down_on() {
        mix(palette.accent, Color32::BLACK, 0.12)
    } else {
        let hover = ui.ctx().animate_bool_with_time_and_easing(
            response.id.with("hover"),
            response.hovered(),
            motion::FAST,
            motion::ease_out,
        );
        mix(palette.accent, mix(palette.accent, Color32::WHITE, 0.12), hover)
    };
    ui.painter().rect_filled(rect, corner, fill);
    let color = if enabled {
        palette.on_accent
    } else {
        palette.text_tertiary
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::strong(), color);
    let icon_width = if glyph.is_some() { 14.0 + space::S } else { 0.0 };
    let left = rect.center().x - (icon_width + galley.size().x) / 2.0;
    if let Some(glyph) = glyph {
        ui.painter().text(
            Pos2::new(left + 7.0, rect.center().y),
            Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(13.0),
            color,
        );
    }
    ui.painter().galley(
        Pos2::new(left + icon_width, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    focus_ring(ui, &response, rect.expand(1.0), radius::LAYER + 1, palette);
    response
}

/// The design's search box: quiet fill, hairline outline (accent while focused), a
/// magnifier, and the text. Returns the text edit's response.
pub fn search_field(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    text: &mut String,
    hint: &str,
    right_inset: f32,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let focused = ui.ctx().memory(|memory| memory.has_focus(id));
    let corner = CornerRadius::same(radius::LAYER);
    ui.painter().rect(
        rect,
        corner,
        palette.surface_alt,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    if focused {
        // Windows text boxes: an accent underline while typing.
        let line = Rect::from_min_max(
            Pos2::new(rect.left() + 1.0, rect.bottom() - 2.0),
            Pos2::new(rect.right() - 1.0, rect.bottom()),
        );
        ui.painter().rect_filled(
            line,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius::LAYER,
                se: radius::LAYER,
            },
            palette.accent,
        );
    }
    ui.painter().text(
        Pos2::new(rect.left() + 18.0, rect.center().y),
        Align2::CENTER_CENTER,
        theme::icon::SEARCH,
        theme::icon_font(13.0),
        palette.text_tertiary,
    );
    let field = Rect::from_min_max(
        Pos2::new(rect.left() + 30.0, rect.top()),
        Pos2::new(rect.right() - right_inset, rect.bottom()),
    );
    ui.put(
        field,
        TextEdit::singleline(text)
            .id(id)
            .hint_text(hint)
            .font(theme::body())
            .text_color(palette.text)
            .frame(Frame::NONE.inner_margin(Margin::symmetric(2, 0)))
            .desired_width(field.width())
            .align(Align2::LEFT_CENTER),
    )
}

/// A single-line text field: surface fill, hairline outline, accent outline while focused.
pub fn text_field(ui: &mut Ui, rect: Rect, id: Id, text: &mut String, hint: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    let focused = ui.ctx().memory(|memory| memory.has_focus(id));
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::M),
        palette.surface,
        if focused {
            Stroke::new(1.5, palette.accent)
        } else {
            palette.hairline_stroke()
        },
        StrokeKind::Inside,
    );
    ui.put(
        rect,
        TextEdit::singleline(text)
            .id(id)
            .hint_text(hint)
            .font(theme::body())
            .text_color(palette.text)
            .frame(Frame::NONE.inner_margin(Margin::symmetric(10, 0)))
            .desired_width(rect.width())
            .align(Align2::LEFT_CENTER),
    )
}

/// A hoverable row inside a flyout or list: rounded hover fill (or the selected tint), and
/// the focus ring. Returns the click response.
pub fn list_row(ui: &Ui, rect: Rect, id: Id, selected: bool) -> Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let fill = if selected {
        palette.accent_soft
    } else {
        interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);
    focus_ring(ui, &response, rect, radius::M, palette);
    response
}

/// A keyboard shortcut drawn as a small outlined key cap, right-aligned at `right_center`.
/// Returns its width.
pub fn key_cap(ui: &Ui, right_center: Pos2, text: &str) -> f32 {
    let palette = Palette::of(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::new(11.0, egui::FontFamily::Proportional),
        palette.text_tertiary,
    );
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let rect = Rect::from_min_size(
        Pos2::new(right_center.x - size.x, right_center.y - size.y / 2.0),
        size,
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius::S),
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        palette.text_tertiary,
    );
    size.x
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
