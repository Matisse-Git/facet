//! Image comparison: side by side, a swipe with a draggable divider, or the new version laid
//! over the old one with adjustable opacity (onion skin). Transparent pixels show a
//! checkerboard; small images are magnified by whole steps so pixels stay crisp.

use eframe::egui::{
    Align2, Color32, ColorImage, CursorIcon, Id, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, TextureWrapMode, Ui, Vec2,
};

use crate::filekind::size_label;
use crate::settings::ImageMode;
use crate::theme::{self, space, Palette};
use crate::workspace::{ImageDiff, ImageSide};

const LABEL_HEIGHT: f32 = 28.0;
const CONTROL_HEIGHT: f32 = 36.0;
/// Checkerboard square size, in points.
const CHECKER: f32 = 8.0;
const FULL_UV: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

pub fn show(
    ui: &Ui,
    rect: Rect,
    images: &ImageDiff,
    mode: ImageMode,
    swipe: &mut f32,
    onion: &mut f32,
) {
    let area = rect.shrink(space::M);
    match (&images.old, &images.new) {
        (Some(old), Some(new)) => match mode {
            ImageMode::SideBySide => {
                let half = ((area.width() - space::M) / 2.0).max(0.0);
                let left = Rect::from_min_size(area.min, Vec2::new(half, area.height()));
                let right =
                    Rect::from_min_max(Pos2::new(left.right() + space::M, area.top()), area.max);
                panel(ui, left, "Before", old);
                panel(ui, right, "After", new);
            }
            ImageMode::Swipe => swipe_view(ui, area, old, new, swipe),
            ImageMode::OnionSkin => onion_view(ui, area, old, new, onion),
        },
        (None, Some(new)) => panel(ui, area, "Added", new),
        (Some(old), None) => panel(ui, area, "Deleted", old),
        (None, None) => {}
    }
}

fn describe(side: &ImageSide) -> String {
    format!(
        "{} × {} px · {}",
        side.width,
        side.height,
        size_label(side.bytes)
    )
}

/// Label on the left (and optionally another on the right) above an image.
fn labels(ui: &Ui, rect: Rect, left: &str, right: Option<&str>) {
    let palette = Palette::of(ui.ctx());
    let y = rect.top() + LABEL_HEIGHT / 2.0;
    ui.painter().text(
        Pos2::new(rect.left(), y),
        Align2::LEFT_CENTER,
        left,
        theme::small(),
        palette.text_secondary,
    );
    if let Some(right) = right {
        ui.painter().text(
            Pos2::new(rect.right(), y),
            Align2::RIGHT_CENTER,
            right,
            theme::small(),
            palette.text_secondary,
        );
    }
}

/// Where an image of `width` × `height` pixels is shown inside `frame`: at its native size,
/// shrunk to fit, or magnified by a whole factor (up to 8×) when it is much smaller.
fn fit(ui: &Ui, frame: Rect, width: u32, height: u32) -> Rect {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let native = Vec2::new(width as f32, height as f32) / pixels_per_point;
    if native.x <= 0.0 || native.y <= 0.0 || frame.width() <= 0.0 || frame.height() <= 0.0 {
        return Rect::from_center_size(frame.center(), Vec2::ZERO);
    }
    let scale = scale_to_fit(native, frame.size());
    let size = native * scale;
    // Snap to physical pixels so 1:1 images stay sharp.
    let min = ((frame.center() - size / 2.0) * pixels_per_point).round() / pixels_per_point;
    Rect::from_min_size(min, size)
}

fn scale_to_fit(native: Vec2, available: Vec2) -> f32 {
    let scale = (available.x / native.x).min(available.y / native.y);
    if scale >= 2.0 {
        scale.floor().min(8.0)
    } else {
        scale.min(1.0)
    }
}

fn checker(ui: &Ui, rect: Rect) {
    let palette = Palette::of(ui.ctx());
    let id = Id::unique(("facet-checker", palette.dark));
    let texture = ui
        .ctx()
        .data_mut(|data| data.get_temp::<TextureHandle>(id))
        .unwrap_or_else(|| {
            let (a, b) = if palette.dark { (44, 58) } else { (255, 230) };
            let pixels: Vec<u8> = [a, b, b, a]
                .into_iter()
                .flat_map(|gray| [gray, gray, gray, 255])
                .collect();
            let image = ColorImage::from_rgba_unmultiplied([2, 2], &pixels);
            let texture = ui.ctx().load_texture(
                "checkerboard",
                image,
                TextureOptions {
                    wrap_mode: TextureWrapMode::Repeat,
                    ..TextureOptions::NEAREST
                },
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        });
    let uv = Rect::from_min_size(Pos2::ZERO, rect.size() / (CHECKER * 2.0));
    ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
}

fn outline(ui: &Ui, rect: Rect) {
    let palette = Palette::of(ui.ctx());
    ui.painter().rect_stroke(
        rect,
        0.0,
        palette.hairline_stroke(),
        eframe::egui::StrokeKind::Outside,
    );
}

/// One image with its label above and its size below.
fn panel(ui: &Ui, rect: Rect, label: &str, side: &ImageSide) {
    labels(ui, rect, label, None);
    let palette = Palette::of(ui.ctx());
    let frame = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + LABEL_HEIGHT),
        Pos2::new(rect.right(), rect.bottom() - LABEL_HEIGHT),
    );
    let shown = fit(ui, frame, side.width, side.height);
    checker(ui, shown);
    ui.painter()
        .image(side.texture.id(), shown, FULL_UV, Color32::WHITE);
    outline(ui, shown);
    ui.painter().text(
        Pos2::new(shown.center().x, shown.bottom() + LABEL_HEIGHT / 2.0),
        Align2::CENTER_CENTER,
        describe(side),
        theme::small(),
        palette.text_tertiary,
    );
}

/// The common box both images are drawn in, and each image's rectangle inside it (same scale,
/// centered).
fn overlay_layout(ui: &Ui, frame: Rect, old: &ImageSide, new: &ImageSide) -> (Rect, Rect, Rect) {
    let width = old.width.max(new.width);
    let height = old.height.max(new.height);
    let bounds = fit(ui, frame, width, height);
    let scale = if width > 0 {
        bounds.width() / width as f32
    } else {
        0.0
    };
    let place = |side: &ImageSide| {
        Rect::from_center_size(
            bounds.center(),
            Vec2::new(side.width as f32, side.height as f32) * scale,
        )
    };
    (bounds, place(old), place(new))
}

fn swipe_view(ui: &Ui, area: Rect, old: &ImageSide, new: &ImageSide, swipe: &mut f32) {
    let palette = Palette::of(ui.ctx());
    labels(
        ui,
        area,
        &format!("Before  {}", describe(old)),
        Some(&format!("After  {}", describe(new))),
    );
    let frame = Rect::from_min_max(Pos2::new(area.left(), area.top() + LABEL_HEIGHT), area.max);
    let (bounds, old_rect, new_rect) = overlay_layout(ui, frame, old, new);
    if bounds.width() <= 0.0 {
        return;
    }

    let response = ui.interact(
        bounds.expand(space::S),
        Id::unique("image-swipe"),
        Sense::click_and_drag(),
    );
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            *swipe = ((pos.x - bounds.left()) / bounds.width()).clamp(0.0, 1.0);
        }
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }

    let divider = bounds.left() + bounds.width() * *swipe;
    checker(ui, bounds);
    let clip = ui.painter().clip_rect();
    let before = Rect::from_min_max(bounds.min, Pos2::new(divider, bounds.bottom()));
    let after = Rect::from_min_max(Pos2::new(divider, bounds.top()), bounds.max);
    ui.painter().with_clip_rect(before.intersect(clip)).image(
        old.texture.id(),
        old_rect,
        FULL_UV,
        Color32::WHITE,
    );
    ui.painter().with_clip_rect(after.intersect(clip)).image(
        new.texture.id(),
        new_rect,
        FULL_UV,
        Color32::WHITE,
    );
    outline(ui, bounds);
    ui.painter()
        .vline(divider, bounds.y_range(), Stroke::new(2.0, palette.accent));
    let handle = Pos2::new(divider, bounds.center().y);
    ui.painter().circle(
        handle,
        9.0,
        palette.accent,
        Stroke::new(2.0, palette.on_accent),
    );
}

fn onion_view(ui: &Ui, area: Rect, old: &ImageSide, new: &ImageSide, onion: &mut f32) {
    let palette = Palette::of(ui.ctx());
    labels(
        ui,
        area,
        &format!("Before  {}", describe(old)),
        Some(&format!("After  {}", describe(new))),
    );
    let frame = Rect::from_min_max(
        Pos2::new(area.left(), area.top() + LABEL_HEIGHT),
        Pos2::new(area.right(), area.bottom() - CONTROL_HEIGHT),
    );
    let (bounds, old_rect, new_rect) = overlay_layout(ui, frame, old, new);
    checker(ui, bounds);
    ui.painter()
        .image(old.texture.id(), old_rect, FULL_UV, Color32::WHITE);
    let alpha = (*onion * 255.0).round() as u8;
    ui.painter().image(
        new.texture.id(),
        new_rect,
        FULL_UV,
        Color32::from_white_alpha(alpha),
    );
    outline(ui, bounds);

    // Opacity slider: "Before" ——o—— "After".
    let y = area.bottom() - CONTROL_HEIGHT / 2.0;
    let track_width = 200.0_f32.min(area.width() - 120.0).max(40.0);
    let track = Rect::from_center_size(Pos2::new(area.center().x, y), Vec2::new(track_width, 4.0));
    for (text, x, align) in [
        ("Before", track.left() - space::S, Align2::RIGHT_CENTER),
        ("After", track.right() + space::S, Align2::LEFT_CENTER),
    ] {
        ui.painter().text(
            Pos2::new(x, y),
            align,
            text,
            theme::small(),
            palette.text_secondary,
        );
    }
    let response = ui.interact(
        track.expand2(Vec2::new(8.0, 10.0)),
        Id::unique("onion-slider"),
        Sense::click_and_drag(),
    );
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            *onion = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
        }
    }
    let knob = Pos2::new(track.left() + track.width() * *onion, y);
    ui.painter().rect_filled(track, 2.0, palette.pressed);
    ui.painter().rect_filled(
        Rect::from_min_max(track.min, Pos2::new(knob.x, track.bottom())),
        2.0,
        palette.accent,
    );
    let hovered = response.hovered() || response.dragged();
    ui.painter().circle(
        knob,
        if hovered { 8.0 } else { 7.0 },
        palette.on_accent,
        Stroke::new(1.0, palette.hairline),
    );
    response.on_hover_text(format!(
        "Opacity of the new image: {}%",
        alpha as u32 * 100 / 255
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_grow_by_whole_steps_and_large_ones_shrink() {
        assert_eq!(
            scale_to_fit(Vec2::new(16.0, 16.0), Vec2::new(400.0, 300.0)),
            8.0
        );
        assert_eq!(
            scale_to_fit(Vec2::new(100.0, 100.0), Vec2::new(250.0, 290.0)),
            2.0
        );
        assert_eq!(
            scale_to_fit(Vec2::new(100.0, 100.0), Vec2::new(150.0, 150.0)),
            1.0
        );
        assert_eq!(
            scale_to_fit(Vec2::new(800.0, 400.0), Vec2::new(400.0, 400.0)),
            0.5
        );
    }
}
