//! Toasts: brief, non-modal messages stacked at the bottom of the content area. Errors stay
//! longer and carry git's own words as detail; nothing here ever blocks the app.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, text::LayoutJob, Align2, CornerRadius, Id, Order, Pos2, Rect, Sense, StrokeKind,
    TextFormat, Ui, Vec2,
};

use super::widgets::icon_button_at;
use crate::app::FacetApp;
use crate::theme::{self, icon, motion, radius, space, Palette};

const MAX_WIDTH: f32 = 440.0;
const DETAIL_MAX_ROWS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub detail: Option<String>,
    created: Instant,
    dismissed: bool,
}

impl Toast {
    pub fn new(id: u64, kind: ToastKind, title: String, detail: Option<String>) -> Self {
        Self {
            id,
            kind,
            title,
            detail,
            created: Instant::now(),
            dismissed: false,
        }
    }

    fn lifetime(&self) -> Duration {
        match self.kind {
            ToastKind::Info => Duration::from_millis(3500),
            ToastKind::Error => Duration::from_secs(12),
        }
    }

    /// Time left before the toast dismisses itself, or `None` once it should be gone.
    fn remaining(&self) -> Option<Duration> {
        if self.dismissed {
            return None;
        }
        self.lifetime().checked_sub(self.created.elapsed())
    }
}

impl FacetApp {
    pub(super) fn toast_layer(&mut self, ui: &mut Ui, content: Rect) {
        if self.toasts.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let palette = Palette::of(&ctx);
        let width = MAX_WIDTH.min(content.width() - space::XL);
        let text_left_inset = space::M + 16.0 + space::S;
        let text_width = width - text_left_inset - space::S - 28.0 - space::S;

        let mut bottom = content.bottom() - space::L;
        let mut finished: Vec<u64> = Vec::new();
        let mut dismiss: Vec<u64> = Vec::new();

        // Newest toast sits at the bottom of the stack.
        for toast in self.toasts.iter().rev() {
            let remaining = toast.remaining();
            let visible = ctx.animate_bool_with_time_and_easing(
                Id::unique(("toast-visible", toast.id)),
                remaining.is_some(),
                motion::NORMAL,
                motion::ease_out,
            );
            match remaining {
                Some(remaining) => ctx.request_repaint_after(remaining),
                None if visible <= 0.0 => {
                    finished.push(toast.id);
                    continue;
                }
                None => {}
            }

            let title = ctx.fonts_mut(|fonts| {
                fonts.layout_job(wrapped(
                    &toast.title,
                    theme::strong(),
                    palette.text,
                    text_width,
                    2,
                ))
            });
            let detail = toast.detail.as_ref().map(|detail| {
                ctx.fonts_mut(|fonts| {
                    fonts.layout_job(wrapped(
                        detail,
                        theme::small(),
                        palette.text_secondary,
                        text_width,
                        DETAIL_MAX_ROWS,
                    ))
                })
            });
            let text_height = title.size().y
                + detail
                    .as_ref()
                    .map_or(0.0, |detail| space::XS + detail.size().y);
            let height =
                (text_height + space::M + space::XS * 2.0).max(theme::ROW_HEIGHT + space::S);

            let slide = (1.0 - visible) * space::M;
            let rect = Rect::from_min_size(
                Pos2::new(content.center().x - width / 2.0, bottom - height + slide),
                Vec2::new(width, height),
            );
            bottom -= (height + space::S) * visible;

            egui::Area::new(Id::unique(("toast", toast.id)))
                .order(Order::Middle)
                .fixed_pos(rect.min)
                .show(&ctx, |ui| {
                    ui.set_opacity(visible);
                    // Occupy the rect so clicks do not fall through to the view below.
                    ui.allocate_rect(rect, Sense::click());

                    let corner = CornerRadius::same(radius::L);
                    ui.painter()
                        .add(ui.visuals().popup_shadow.as_shape(rect, corner));
                    ui.painter().rect(
                        rect,
                        corner,
                        palette.raised,
                        palette.hairline_stroke(),
                        StrokeKind::Outside,
                    );

                    let (glyph, tint) = match toast.kind {
                        ToastKind::Info => (icon::INFO, palette.accent),
                        ToastKind::Error => (icon::ERROR, palette.danger),
                    };
                    let first_line_center =
                        rect.top() + space::S + space::XS + title.size().y.min(20.0) / 2.0;
                    ui.painter().text(
                        Pos2::new(rect.left() + space::M + 8.0, first_line_center),
                        Align2::CENTER_CENTER,
                        glyph,
                        theme::icon_font(16.0),
                        tint,
                    );

                    let mut y = rect.top() + space::S + space::XS;
                    let text_x = rect.left() + text_left_inset;
                    let title_height = title.size().y;
                    ui.painter()
                        .galley(Pos2::new(text_x, y), title.clone(), palette.text);
                    y += title_height + space::XS;
                    if let Some(detail) = &detail {
                        ui.painter().galley(
                            Pos2::new(text_x, y),
                            detail.clone(),
                            palette.text_secondary,
                        );
                    }

                    let close = Rect::from_center_size(
                        Pos2::new(rect.right() - space::S - 14.0, rect.top() + space::S + 14.0),
                        Vec2::splat(28.0),
                    );
                    if icon_button_at(
                        ui,
                        close,
                        Id::unique(("toast-close", toast.id)),
                        icon::CLOSE,
                        "Dismiss",
                    )
                    .clicked()
                    {
                        dismiss.push(toast.id);
                    }
                });
        }

        for toast in &mut self.toasts {
            if dismiss.contains(&toast.id) {
                toast.dismissed = true;
            }
        }
        self.toasts.retain(|toast| !finished.contains(&toast.id));
    }
}

/// Text wrapped to `width`, cut off with "…" after `max_rows` lines.
fn wrapped(
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
    max_rows: usize,
) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap.max_width = width.max(0.0);
    job.wrap.max_rows = max_rows;
    job.wrap.overflow_character = Some('…');
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_outlive_info_toasts() {
        let info = Toast::new(1, ToastKind::Info, "Copied".into(), None);
        let error = Toast::new(2, ToastKind::Error, "Failed".into(), None);
        assert!(error.lifetime() > info.lifetime());
        assert!(info.remaining().is_some());
    }

    #[test]
    fn dismissed_toasts_have_no_time_left() {
        let mut toast = Toast::new(1, ToastKind::Error, "Failed".into(), None);
        toast.dismissed = true;
        assert!(toast.remaining().is_none());
    }
}
