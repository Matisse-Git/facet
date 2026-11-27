//! The Remove Repository dialog: take a repository off Facet's lists, and optionally move its
//! folder to the Recycle Bin after showing what exists only there. Enter confirms, Escape
//! cancels; Cancel comes first.

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, Align, Color32, CornerRadius, Galley, Id, Key, Layout, Modifiers, Order,
    Pos2, Rect, Sense, StrokeKind, TextFormat, Ui, UiBuilder, Vec2,
};

use super::widgets::{button_enabled, checkbox, ButtonKind};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::app_remove::FolderCheck;
use crate::theme::{self, motion, radius, space, Palette};

const WIDTH: f32 = 460.0;

/// One piece of the dialog's column, top to bottom.
enum Block {
    Text(Arc<Galley>, Color32),
    /// The "Also move the folder to the Recycle Bin" checkbox.
    Check,
    Gap(f32),
}

impl FacetApp {
    pub(super) fn remove_repo_layer(&mut self, ui: &mut Ui, window: Rect) {
        let ctx = ui.ctx().clone();
        let visible = ctx.animate_bool_with_time_and_easing(
            Id::unique("remove-repo-visible"),
            self.remove_view.is_some(),
            motion::NORMAL,
            motion::ease_out,
        );
        let Some(view) = &self.remove_view else {
            return;
        };        let palette = Palette::of(&ctx);
        let mut actions: Vec<Action> = Vec::new();
        ctx.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::Enter) {
                actions.push(Action::ConfirmRemoveRepository);
            }
            if input.consume_key(Modifiers::NONE, Key::Escape) {
                actions.push(Action::CloseRemoveRepository);
            }
        });

        let width = WIDTH.min(window.width() - space::XL);
        let text_width = width - space::L * 2.0;
        let text = |text: &str, font: egui::FontId, color: Color32| {
            let mut job = LayoutJob::single_section(
                text.to_owned(),
                TextFormat {
                    font_id: font,
                    color,
                    ..Default::default()
                },
            );
            job.wrap.max_width = text_width;
            Block::Text(ctx.fonts_mut(|fonts| fonts.layout_job(job)), color)
        };

        let blocked = view.recycle_blocked();
        let warnings = view.warnings();
        let mut blocks = vec![
            text(
                &format!("Remove {} from Facet?", view.name),
                theme::strong(),
                palette.text,
            ),
            Block::Gap(space::XS),
            text(
                &view.path.display().to_string(),
                theme::small(),
                palette.text_tertiary,
            ),
            Block::Gap(space::S),
        ];
        if view.recycle {
            let mut body = "The folder and everything in it goes to the Recycle Bin. Facet \
                            can't undo that; restore it from the Recycle Bin if you need it."
                .to_owned();
            if let FolderCheck::Ready(check) = &view.check {
                if let Some(main) = &check.main_worktree {
                    body.push_str(&format!(
                        " It is a worktree of {}; git's record of it there is cleaned up \
                         afterwards.",
                        crate::app::display_name(main)
                    ));
                }
            }
            blocks.push(text(&body, theme::body(), palette.text_secondary));
        } else {
            blocks.push(text(
                "Facet stops listing it and closes its tab. The folder stays on disk, \
                 untouched.",
                theme::body(),
                palette.text_secondary,
            ));
        }
        blocks.push(Block::Gap(space::S));
        blocks.push(Block::Check);
        if let Some(reason) = &blocked {
            blocks.push(text(reason, theme::small(), palette.text_secondary));
        }
        if view.recycle {
            blocks.push(Block::Gap(space::S));
            if view.check == FolderCheck::Checking {
                blocks.push(text(
                    "Checking what exists only in this folder…",
                    theme::small(),
                    palette.text_tertiary,
                ));
            } else if !warnings.is_empty() {
                blocks.push(text(
                    "Only in this folder, and lost with it:",
                    theme::small_strong(),
                    palette.danger,
                ));
                blocks.push(Block::Gap(space::XS));
                for line in &warnings {
                    blocks.push(text(&format!("•  {line}"), theme::body(), palette.text));
                }
            } else if matches!(view.check, FolderCheck::Ready(_)) {
                blocks.push(text(
                    "Everything here is committed and pushed, with nothing stashed.",
                    theme::small(),
                    palette.success,
                ));
            }
        }

        let content_height: f32 = blocks
            .iter()
            .map(|block| match block {
                Block::Text(galley, _) => galley.size().y,
                Block::Check => theme::ROW_HEIGHT,
                Block::Gap(gap) => *gap,
            })
            .sum();
        let height = space::L + content_height + space::L + theme::ROW_HEIGHT + space::M;
        let panel = Rect::from_min_size(
            Pos2::new(
                window.center().x - width / 2.0,
                window.top() + (window.height() * 0.2).max(theme::TITLE_BAR_HEIGHT + space::M)
                    - (1.0 - visible) * space::S,
            ),
            Vec2::new(width, height),
        );
        let recycle = view.recycle;
        let recycling = view.recycling;
        let can_confirm = view.can_confirm();
        let confirm_label = view.confirm_label();

        egui::Area::new(Id::unique("remove-repo"))
            .order(Order::Foreground)
            .fixed_pos(window.min)
            .show(&ctx, |ui| {
                ui.set_opacity(visible);

                // Scrim: dims the app and swallows clicks outside the dialog.
                let scrim = ui.allocate_rect(window, Sense::click());
                ui.painter().rect_filled(window, 0.0, palette.scrim);
                if scrim.clicked() {
                    actions.push(Action::CloseRemoveRepository);
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
                ui.interact(panel, Id::unique("remove-repo-panel"), Sense::click());

                let left = panel.left() + space::L;
                let mut y = panel.top() + space::L;
                for block in blocks {
                    match block {
                        Block::Text(galley, color) => {
                            let height = galley.size().y;
                            ui.painter().galley(Pos2::new(left, y), galley, color);
                            y += height;
                        }
                        Block::Gap(gap) => y += gap,
                        Block::Check => {
                            let row = Rect::from_min_size(
                                Pos2::new(left, y),
                                Vec2::new(text_width, theme::ROW_HEIGHT),
                            );
                            ui.scope_builder(
                                UiBuilder::new()
                                    .id_salt("remove-repo-recycle")
                                    .max_rect(row)
                                    .layout(Layout::left_to_right(Align::Center)),
                                |ui| {
                                    let enabled = blocked.is_none() && !recycling;
                                    if checkbox(
                                        ui,
                                        recycle,
                                        "Also move the folder to the Recycle Bin",
                                        "Never a permanent delete: if Windows can't recycle \
                                         it, the folder stays where it is",
                                        enabled,
                                    )
                                    .clicked()
                                    {
                                        actions.push(Action::ToggleRemoveRecycle);
                                    }
                                },
                            );
                            y += theme::ROW_HEIGHT;
                        }
                    }
                }

                let row = Rect::from_min_max(
                    Pos2::new(left, panel.bottom() - space::M - theme::ROW_HEIGHT),
                    Pos2::new(panel.right() - space::M, panel.bottom() - space::M),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .id_salt("remove-repo-buttons")
                        .max_rect(row)
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        let kind = if recycle {
                            ButtonKind::Danger
                        } else {
                            ButtonKind::Primary
                        };
                        if button_enabled(
                            ui,
                            kind,
                            None,
                            confirm_label,
                            &format!("{confirm_label}  (Enter)"),
                            can_confirm,
                        )
                        .clicked()
                        {
                            actions.push(Action::ConfirmRemoveRepository);
                        }
                        if button_enabled(
                            ui,
                            ButtonKind::Secondary,
                            None,
                            "Cancel",
                            "Cancel  (Esc)",
                            !recycling,
                        )
                        .clicked()
                        {
                            actions.push(Action::CloseRemoveRepository);
                        }
                    },
                );
            });

        self.pending.extend(actions);
    }
}
