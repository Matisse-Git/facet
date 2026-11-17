//! The command palette (Ctrl+P): a fuzzy-searchable list of every action available right now.
//! It is built from `actions::commands`, so anything Facet can do is reachable from here.

use eframe::egui::{
    self, text::LayoutJob, Align2, CornerRadius, Id, Key, Modifiers, Order, Pos2, Rect, ScrollArea,
    Sense, StrokeKind, TextEdit, TextFormat, Ui, UiBuilder, Vec2,
};

use super::widgets;
use crate::actions::Command;
use crate::app::FacetApp;
use crate::fuzzy::fuzzy_match;
use crate::theme::{self, icon, motion, radius, space, Palette};

const WIDTH: f32 = 560.0;
const INPUT_HEIGHT: f32 = 48.0;
const ROW_HEIGHT: f32 = 36.0;
const MAX_VISIBLE_ROWS: usize = 9;

#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    query: String,
    selected: usize,
    /// Give the text field keyboard focus on the next frame.
    focus_pending: bool,
    /// Scroll the selected row into view on the next frame (after keyboard navigation).
    reveal_pending: bool,
}

impl PaletteState {
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.query.clear();
            self.selected = 0;
            self.focus_pending = true;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

/// Commands matching `query`, best match first, with the matched character positions.
fn filter(commands: Vec<Command>, query: &str) -> Vec<(Command, Vec<usize>)> {
    let mut matches: Vec<(i32, Command, Vec<usize>)> = commands
        .into_iter()
        .filter_map(|command| {
            let found = fuzzy_match(query, &command.title)?;
            Some((found.score, command, found.positions))
        })
        .collect();
    // Stable sort: equal scores keep palette order.
    matches.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    matches
        .into_iter()
        .map(|(_, command, positions)| (command, positions))
        .collect()
}

impl FacetApp {
    pub(super) fn palette_layer(&mut self, ui: &mut Ui, window: Rect) {
        let ctx = ui.ctx().clone();
        let visible = ctx.animate_bool_with_time_and_easing(
            Id::unique("palette-visible"),
            self.palette.open,
            motion::NORMAL,
            motion::ease_out,
        );
        if visible <= 0.0 {
            return;
        }
        let palette = Palette::of(&ctx);

        // Keyboard: consumed before the text field sees it.
        let mut execute = false;
        if self.palette.open {
            let (down, up, enter, escape) = ctx.input_mut(|input| {
                (
                    input.consume_key(Modifiers::NONE, Key::ArrowDown),
                    input.consume_key(Modifiers::NONE, Key::ArrowUp),
                    input.consume_key(Modifiers::NONE, Key::Enter),
                    input.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if escape {
                self.palette.close();
            }
            if down {
                self.palette.selected = self.palette.selected.saturating_add(1);
                self.palette.reveal_pending = true;
            }
            if up {
                self.palette.selected = self.palette.selected.saturating_sub(1);
                self.palette.reveal_pending = true;
            }
            execute = enter;
        }

        let matches = filter(self.commands(), &self.palette.query);
        if matches.is_empty() {
            self.palette.selected = 0;
        } else {
            self.palette.selected = self.palette.selected.min(matches.len() - 1);
        }

        let rows = matches.len().clamp(1, MAX_VISIBLE_ROWS);
        let width = WIDTH.min(window.width() - space::XL);
        let height = INPUT_HEIGHT + 1.0 + rows as f32 * ROW_HEIGHT + space::S * 2.0;
        let top = window.top() + (window.height() * 0.16).max(theme::TITLE_BAR_HEIGHT + space::M)
            - (1.0 - visible) * space::S;
        let panel = Rect::from_min_size(
            Pos2::new(window.center().x - width / 2.0, top),
            Vec2::new(width, height),
        );

        let mut clicked: Option<usize> = None;
        let mut dismissed = false;

        egui::Area::new(Id::unique("palette"))
            .order(Order::Foreground)
            .fixed_pos(window.min)
            .show(&ctx, |ui| {
                ui.set_opacity(visible);

                // Scrim: dims the app and swallows clicks outside the panel.
                let scrim = ui.allocate_rect(window, Sense::click());
                ui.painter().rect_filled(window, 0.0, palette.scrim);
                if scrim.clicked() {
                    dismissed = true;
                }

                // Panel.
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
                // Clicks on the panel itself must not reach the scrim.
                ui.interact(panel, Id::unique("palette-panel"), Sense::click());

                // Search field.
                let input = Rect::from_min_size(panel.min, Vec2::new(panel.width(), INPUT_HEIGHT));
                ui.painter().text(
                    Pos2::new(input.left() + space::M + 8.0, input.center().y),
                    Align2::CENTER_CENTER,
                    icon::SEARCH,
                    theme::icon_font(14.0),
                    palette.text_secondary,
                );
                // A single text row, vertically centered in the input strip.
                let line_height = ui.fonts_mut(|fonts| fonts.row_height(&theme::body()));
                let field = Rect::from_min_max(
                    Pos2::new(
                        input.left() + space::M + 16.0 + space::S,
                        input.center().y - line_height / 2.0,
                    ),
                    Pos2::new(
                        input.right() - space::M,
                        input.center().y + line_height / 2.0,
                    ),
                );
                let response = ui.put(
                    field,
                    TextEdit::singleline(&mut self.palette.query)
                        .id(Id::unique("palette-query"))
                        .hint_text("Type a command…")
                        .font(theme::body())
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .lock_focus(true)
                        .desired_width(field.width()),
                );
                if self.palette.focus_pending || (self.palette.open && !response.has_focus()) {
                    response.request_focus();
                    self.palette.focus_pending = false;
                }
                if response.changed() {
                    self.palette.selected = 0;
                    self.palette.reveal_pending = true;
                }
                widgets::hairline_h(ui, panel.x_range().into(), input.bottom());

                // Results.
                let list = Rect::from_min_max(
                    Pos2::new(panel.left() + space::S, input.bottom() + 1.0 + space::S),
                    Pos2::new(panel.right() - space::S, panel.bottom() - space::S),
                );
                if matches.is_empty() {
                    ui.painter().text(
                        Pos2::new(list.left() + space::S, list.top() + ROW_HEIGHT / 2.0),
                        Align2::LEFT_CENTER,
                        "No matching commands",
                        theme::body(),
                        palette.text_tertiary,
                    );
                    return;
                }
                ui.scope_builder(
                    UiBuilder::new().id_salt("palette-rows").max_rect(list),
                    |ui| {
                        ui.set_clip_rect(list.intersect(ui.clip_rect()));
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                            let pointer_moved = ui.input(|input| input.pointer.is_moving());
                            for (index, (command, positions)) in matches.iter().enumerate() {
                                let (row, response) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), ROW_HEIGHT),
                                    Sense::click(),
                                );
                                // The mouse only takes over the selection when it actually moves,
                                // so it never fights the arrow keys.
                                if response.hovered() && pointer_moved {
                                    self.palette.selected = index;
                                }
                                let selected = self.palette.selected == index;
                                if selected {
                                    ui.painter().rect_filled(
                                        row,
                                        CornerRadius::same(radius::M),
                                        palette.accent_soft,
                                    );
                                    if self.palette.reveal_pending {
                                        response.scroll_to_me(None);
                                    }
                                }

                                let shortcut = command
                                    .shortcut
                                    .as_ref()
                                    .map(|shortcut| widgets::shortcut_text(ui, shortcut));
                                let mut title_right = row.right() - space::S;
                                if let Some(shortcut) = shortcut {
                                    let painted = ui.painter().text(
                                        Pos2::new(
                                            row.right() - space::M + space::XS,
                                            row.center().y,
                                        ),
                                        Align2::RIGHT_CENTER,
                                        shortcut,
                                        theme::small(),
                                        palette.text_tertiary,
                                    );
                                    title_right = painted.left() - space::M;
                                }

                                let title_left = row.left() + space::M - space::XS;
                                let galley = ui.painter().layout_job(title_job(
                                    &command.title,
                                    positions,
                                    palette,
                                    title_right - title_left,
                                ));
                                ui.painter().galley(
                                    Pos2::new(title_left, row.center().y - galley.size().y / 2.0),
                                    galley,
                                    palette.text,
                                );

                                if response.clicked() {
                                    clicked = Some(index);
                                }
                            }
                        });
                    },
                );
            });
        self.palette.reveal_pending = false;

        if dismissed {
            self.palette.close();
        }
        let chosen = clicked.or(execute.then_some(self.palette.selected));
        if let Some((command, _)) = chosen.and_then(|index| matches.get(index)) {
            self.palette.close();
            self.pending.push(command.action.clone());
        }
    }
}

/// One line of title text with the fuzzy-matched characters emphasized.
fn title_job(title: &str, positions: &[usize], palette: &Palette, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = max_width.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');

    let plain = TextFormat {
        font_id: theme::body(),
        color: if positions.is_empty() {
            palette.text
        } else {
            palette.text_secondary
        },
        ..Default::default()
    };
    let matched = TextFormat {
        font_id: theme::strong(),
        color: palette.text,
        ..Default::default()
    };

    // Group consecutive characters with the same emphasis into one section.
    let mut run = String::new();
    let mut run_matched = false;
    for (index, ch) in title.chars().enumerate() {
        let is_matched = positions.binary_search(&index).is_ok();
        if is_matched != run_matched && !run.is_empty() {
            let format = if run_matched { &matched } else { &plain };
            job.append(&run, 0.0, format.clone());
            run.clear();
        }
        run_matched = is_matched;
        run.push(ch);
    }
    if !run.is_empty() {
        job.append(&run, 0.0, if run_matched { matched } else { plain });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Action;

    fn command(title: &str) -> Command {
        Command {
            title: title.to_owned(),
            shortcut: None,
            action: Action::Refresh,
        }
    }

    fn titles(matches: &[(Command, Vec<usize>)]) -> Vec<&str> {
        matches.iter().map(|(c, _)| c.title.as_str()).collect()
    }

    #[test]
    fn empty_query_keeps_palette_order() {
        let commands = vec![
            command("Open Repository…"),
            command("Refresh"),
            command("Quit Facet"),
        ];
        let matches = filter(commands, "");
        assert_eq!(
            titles(&matches),
            ["Open Repository…", "Refresh", "Quit Facet"]
        );
    }

    #[test]
    fn filters_and_ranks_by_relevance() {
        let commands = vec![
            command("Open Repository…"),
            command("Theme: Dark"),
            command("Hide Sidebar"),
            command("Show Command Log"),
        ];
        let matches = filter(commands, "dark");
        assert_eq!(titles(&matches), ["Theme: Dark"]);

        let commands = vec![
            command("Show Command Log"),
            command("Copy Repository Path"),
            command("Clear Command Log"),
        ];
        let matches = filter(commands, "ccl");
        assert_eq!(matches[0].0.title, "Clear Command Log");
    }

    #[test]
    fn toggling_resets_the_query_and_selection() {
        let mut state = PaletteState::default();
        state.toggle();
        assert!(state.open);
        state.query.push_str("abc");
        state.selected = 3;
        state.toggle();
        assert!(!state.open);
        state.toggle();
        assert!(state.open);
        assert!(state.query.is_empty());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn title_job_splits_matched_runs() {
        let job = title_job("Theme: Dark", &[7, 8, 9, 10], &theme::DARK, 400.0);
        assert_eq!(job.text, "Theme: Dark");
        assert_eq!(job.sections.len(), 2, "one plain run, one matched run");
        assert_ne!(
            job.sections[0].format.font_id,
            job.sections[1].format.font_id
        );

        let job = title_job("Refresh", &[], &theme::DARK, 400.0);
        assert_eq!(job.sections.len(), 1);
    }
}
