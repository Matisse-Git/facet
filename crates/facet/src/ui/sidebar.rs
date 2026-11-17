//! The sidebar. In M0 it lists recent repositories and reports git availability; branches,
//! stashes, tags, remotes, LFS locks, submodules and worktrees join it in later milestones.
//!
//! It paints no background of its own: the Mica backdrop shows through.

use eframe::egui::{Align2, CornerRadius, Id, Pos2, Rect, ScrollArea, Sense, Ui, UiBuilder, Vec2};

use super::widgets::{self, interaction_fill, text_line};
use crate::actions::Action;
use crate::app::{FacetApp, GitStatus};
use crate::settings::same_path;
use crate::theme::{self, icon, radius, space, Palette, ROW_HEIGHT};

const SECTION_HEADER_HEIGHT: f32 = 32.0;
const FOOTER_HEIGHT: f32 = 36.0;

impl FacetApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());

        // Section header.
        ui.painter().text(
            Pos2::new(
                rect.left() + space::M,
                rect.top() + SECTION_HEADER_HEIGHT / 2.0,
            ),
            Align2::LEFT_CENTER,
            "Recent",
            theme::small_strong(),
            palette.text_tertiary,
        );

        let list = Rect::from_min_max(
            Pos2::new(rect.left() + space::S, rect.top() + SECTION_HEADER_HEIGHT),
            Pos2::new(rect.right() - space::S, rect.bottom() - FOOTER_HEIGHT),
        );
        let active_path = self.active_tab().map(|tab| tab.path.clone());
        let mut actions = Vec::new();

        ui.scope_builder(UiBuilder::new().id_salt("recent").max_rect(list), |ui| {
            ui.set_clip_rect(list.intersect(ui.clip_rect()));
            if self.settings.recent_repos.is_empty() {
                let text = widgets::elided(
                    ui,
                    "Repositories you open appear here.",
                    theme::small(),
                    palette.text_tertiary,
                    list.width() - space::M,
                );
                ui.painter().galley(
                    Pos2::new(list.left() + space::S, list.top() + space::XS),
                    text,
                    palette.text_tertiary,
                );
                return;
            }
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for path in &self.settings.recent_repos {
                    let (row, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    let is_active = active_path.as_deref().is_some_and(|a| same_path(a, path));

                    // "Remove from recents" appears on hover, on top of the row.
                    let remove_rect = Rect::from_center_size(
                        Pos2::new(row.right() - space::XS - 12.0, row.center().y),
                        Vec2::splat(24.0),
                    );
                    let remove = ui.interact(
                        remove_rect,
                        Id::unique(("recent-remove", path)),
                        Sense::click(),
                    );
                    let remove_hovered = remove.hovered() || remove.has_focus();
                    let hovered = response.hovered() || remove_hovered;

                    let fill = if is_active {
                        palette.pressed
                    } else if remove_hovered {
                        palette.hover
                    } else {
                        interaction_fill(ui, &response, palette)
                    };
                    ui.painter()
                        .rect_filled(row, CornerRadius::same(radius::M), fill);

                    ui.painter().text(
                        Pos2::new(row.left() + space::S + 8.0, row.center().y),
                        Align2::CENTER_CENTER,
                        icon::FOLDER,
                        theme::icon_font(14.0),
                        if is_active {
                            palette.accent
                        } else {
                            palette.text_secondary
                        },
                    );
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    let text_left = row.left() + space::S + 16.0 + space::S;
                    let text_right = if hovered {
                        remove_rect.left() - space::XS
                    } else {
                        row.right() - space::S
                    };
                    text_line(
                        ui,
                        Pos2::new(text_left, row.center().y),
                        &name,
                        theme::body(),
                        palette.text,
                        text_right - text_left,
                    );

                    if hovered {
                        if remove_hovered {
                            ui.painter().rect_filled(
                                remove_rect,
                                CornerRadius::same(radius::S),
                                palette.hover,
                            );
                        }
                        ui.painter().text(
                            remove_rect.center(),
                            Align2::CENTER_CENTER,
                            icon::CLOSE,
                            theme::icon_font(9.0),
                            if remove_hovered {
                                palette.text
                            } else {
                                palette.text_secondary
                            },
                        );
                    }
                    widgets::focus_ring(ui, &remove, remove_rect, radius::S, palette);
                    widgets::focus_ring(ui, &response, row, radius::M, palette);

                    if remove.clicked() {
                        actions.push(Action::ForgetRecent(path.clone()));
                    } else if response.clicked() {
                        actions.push(Action::OpenPath(path.clone()));
                    }
                    remove.on_hover_text("Remove from recent repositories");
                    response.on_hover_text(path.display().to_string());
                }
            });
        });
        self.pending.extend(actions);

        // Footer: which git Facet is driving.
        let footer = Pos2::new(rect.left() + space::M, rect.bottom() - FOOTER_HEIGHT / 2.0);
        match &self.git_status {
            GitStatus::Checking => {}
            GitStatus::Found(version) => {
                ui.painter().text(
                    footer,
                    Align2::LEFT_CENTER,
                    format!("Git {version}"),
                    theme::small(),
                    palette.text_tertiary,
                );
            }
            GitStatus::Missing(detail) => {
                let text_rect = ui.painter().text(
                    footer,
                    Align2::LEFT_CENTER,
                    "Git not found",
                    theme::small_strong(),
                    palette.danger,
                );
                ui.interact(text_rect, Id::unique("git-missing"), Sense::hover())
                    .on_hover_text(detail);
            }
        }
    }
}
