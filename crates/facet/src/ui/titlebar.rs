//! The custom title bar: sidebar toggle, repository tabs, "open" button, and empty space that
//! drags the window. The minimize/maximize/close buttons on the right are drawn by Windows
//! (see `platform::Chrome`); this view only leaves room for them.

use eframe::egui::{Align2, CornerRadius, Id, Pos2, Rect, Sense, Ui, Vec2};

use super::widgets::{self, icon_button_at, interaction_fill, text_line};
use crate::actions::{shortcut, Action};
use crate::app::FacetApp;
use crate::platform::{Chrome, PxRect};
use crate::theme::{self, icon, radius, space, Palette};

const CONTROL_HEIGHT: f32 = 28.0;
const TAB_MAX_WIDTH: f32 = 200.0;
const TAB_MIN_WIDTH: f32 = 44.0;
const TAB_GAP: f32 = 2.0;
/// A strip of title bar that is always left empty so the window can be grabbed.
const DRAG_STRIP: f32 = space::XL;

impl FacetApp {
    pub(super) fn title_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let pixels_per_point = ui.ctx().pixels_per_point();
        let center_y = rect.center().y;
        let mut controls: Vec<Rect> = Vec::new();

        // Room for the caption buttons Windows draws at the top right.
        let caption_width = self
            .chrome
            .and_then(Chrome::caption_buttons)
            .map_or(0.0, |bounds| {
                (bounds.right - bounds.left) as f32 / pixels_per_point
            });

        // Sidebar toggle.
        let toggle = Rect::from_center_size(
            Pos2::new(rect.left() + space::S + 16.0, center_y),
            Vec2::new(32.0, CONTROL_HEIGHT),
        );
        let label = if self.settings.sidebar_visible {
            "Hide sidebar"
        } else {
            "Show sidebar"
        };
        let tooltip = widgets::tooltip_with_shortcut(ui, label, &shortcut::SIDEBAR);
        if icon_button_at(
            ui,
            toggle,
            Id::unique("title-sidebar"),
            icon::SIDEBAR,
            &tooltip,
        )
        .clicked()
        {
            self.pending.push(Action::ToggleSidebar);
        }
        controls.push(toggle);

        let mut x = toggle.right() + space::S;
        let add_width = 32.0;
        let tabs_right = rect.right() - caption_width - DRAG_STRIP - add_width - space::XS;

        if self.tabs.is_empty() {
            x += space::XS;
            x += text_line(
                ui,
                Pos2::new(x, center_y),
                "Facet",
                theme::strong(),
                palette.text_secondary,
                200.0,
            );
            x += space::S;
        } else {
            let count = self.tabs.len() as f32;
            let available = (tabs_right - x).max(0.0);
            let width = ((available - TAB_GAP * (count - 1.0)) / count)
                .clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
                .floor();

            for index in 0..self.tabs.len() {
                let tab_rect = Rect::from_min_size(
                    Pos2::new(x, center_y - CONTROL_HEIGHT / 2.0),
                    Vec2::new(width, CONTROL_HEIGHT),
                );
                if tab_rect.right() > tabs_right + 0.5 {
                    // Out of room: the remaining tabs stay reachable through Ctrl+Tab and the
                    // command palette.
                    break;
                }
                self.tab(ui, index, tab_rect);
                controls.push(tab_rect);
                x = tab_rect.right() + TAB_GAP;
            }
            x += space::XS;
        }

        // Open-repository button, right after the tabs.
        let add = Rect::from_min_size(
            Pos2::new(x, center_y - CONTROL_HEIGHT / 2.0),
            Vec2::new(add_width, CONTROL_HEIGHT),
        );
        let tooltip = widgets::tooltip_with_shortcut(ui, "Open repository", &shortcut::OPEN);
        if icon_button_at(ui, add, Id::unique("title-open"), icon::ADD, &tooltip).clicked() {
            self.pending.push(Action::OpenRepository);
        }
        controls.push(add);

        // Tell the window procedure which parts are controls; the rest drags the window.
        if let Some(chrome) = self.chrome {
            let to_px = |value: f32| (value * pixels_per_point).round() as i32;
            let px: Vec<PxRect> = controls
                .iter()
                .map(|r| PxRect {
                    left: to_px(r.left()),
                    top: to_px(r.top()),
                    right: to_px(r.right()),
                    bottom: to_px(r.bottom()),
                })
                .collect();
            chrome.set_title_bar(to_px(rect.bottom()), &px);
        }
    }

    fn tab(&mut self, ui: &mut Ui, index: usize, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let tab = &self.tabs[index];
        let active = self.active == Some(index);
        let name = tab.name();
        let ready = tab.is_ready();

        let response = ui.interact(rect, Id::unique(("tab", tab.id)), Sense::click());

        // The close button is its own control on top of the tab; it needs enough room.
        let close_rect = Rect::from_center_size(
            Pos2::new(rect.right() - space::XS - 10.0, rect.center().y),
            Vec2::splat(20.0),
        );
        let has_close = rect.width() >= 72.0;
        let close = has_close.then(|| {
            ui.interact(
                close_rect,
                Id::unique(("tab-close", tab.id)),
                Sense::click(),
            )
        });
        let close_hovered = close.as_ref().is_some_and(|c| c.hovered() || c.has_focus());
        let hovered = response.hovered() || close_hovered;

        let fill = if active {
            palette.pressed
        } else if close_hovered {
            palette.hover
        } else {
            interaction_fill(ui, &response, palette)
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::M), fill);

        let show_close = has_close && (active || hovered);
        let text_right = if show_close {
            close_rect.left() - space::XS
        } else {
            rect.right() - space::S
        };
        let color = if !ready {
            palette.text_tertiary
        } else if active {
            palette.text
        } else {
            palette.text_secondary
        };
        text_line(
            ui,
            Pos2::new(rect.left() + space::S + space::XS, rect.center().y),
            &name,
            theme::body(),
            color,
            text_right - (rect.left() + space::S + space::XS),
        );

        if let Some(close) = &close {
            if show_close {
                if close_hovered {
                    ui.painter().rect_filled(
                        close_rect,
                        CornerRadius::same(radius::S),
                        if close.is_pointer_button_down_on() {
                            palette.pressed
                        } else {
                            palette.hover
                        },
                    );
                }
                ui.painter().text(
                    close_rect.center(),
                    Align2::CENTER_CENTER,
                    icon::CLOSE,
                    theme::icon_font(9.0),
                    if close_hovered {
                        palette.text
                    } else {
                        palette.text_secondary
                    },
                );
                widgets::focus_ring(ui, close, close_rect, radius::S, palette);
            }
        }
        widgets::focus_ring(ui, &response, rect, radius::M, palette);

        let close_clicked = close.as_ref().is_some_and(|c| c.clicked());
        if let Some(close) = close {
            let tooltip = widgets::tooltip_with_shortcut(ui, "Close tab", &shortcut::CLOSE_TAB);
            close.on_hover_text(tooltip);
        }
        if close_clicked || response.middle_clicked() {
            self.pending.push(Action::CloseTab(index));
        } else if response.clicked() {
            self.pending.push(Action::SwitchTab(index));
        }
        response.on_hover_text(self.tabs[index].path.display().to_string());
    }
}
