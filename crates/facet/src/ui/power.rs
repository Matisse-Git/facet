//! M3 views: the banner of the operation in progress, the search / filter bar above the
//! commit list, the blame view, and the two modal editors (interactive rebase, conflict
//! resolution). All of them only read state and queue actions, like the other views.

use std::time::SystemTime;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Modifiers, Order,
    Pos2, Rect, RichText, ScrollArea, Sense, StrokeKind, TextEdit, Ui, UiBuilder, Vec2,
};
use facet_git::conflict::{Choice, Region, Side};
use facet_git::rebase::TodoAction;
use facet_git::SearchKind;

use super::widgets::{
    self, button, button_enabled, icon_button_at, interaction_fill, text_button_at,
    text_button_width, text_line, ButtonKind,
};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::format::{relative_time, unix_seconds};
use crate::history::{FilterKind, History};
use crate::power::{BlameView, ConflictEditor, RebaseEditor, SearchBar};
use crate::theme::{self, icon, motion, radius, space, Palette};

/// Height of the search / filter bar above the commit list.
pub const FILTER_BAR: f32 = 40.0;
/// Height of the banner shown while an operation is in progress.
pub const BANNER: f32 = 40.0;
const BLAME_HEADER: f32 = 44.0;
const BLAME_ROW: f32 = 20.0;
const BLAME_GUTTER: f32 = 280.0;

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// The search field (or the title of a file history / the reflog) above the commit list.
pub fn filter_bar(
    ui: &mut Ui,
    rect: Rect,
    search: &mut SearchBar,
    history: &History,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let center = rect.center().y;
    let close = Rect::from_center_size(
        Pos2::new(rect.right() - space::S - 12.0, center),
        Vec2::splat(24.0),
    );
    if icon_button_at(
        ui,
        close,
        Id::unique("filter-close"),
        icon::CLOSE,
        "Back to the full history  (Esc)",
    )
    .clicked()
    {
        actions.push(Action::ClearFilter);
    }

    // How many results, on the right.
    let mut right = close.left() - space::S;
    let count = match (history.loading, history.commits.len()) {
        (true, 0) => "Searching…".to_owned(),
        (true, n) => format!("{n}…"),
        (false, 1) if history.filter.is_some() => "1 commit".to_owned(),
        (false, n) if history.filter.is_some() => format!("{n} commits"),
        _ => String::new(),
    };
    if !count.is_empty() {
        let painted = ui.painter().text(
            Pos2::new(right, center),
            Align2::RIGHT_CENTER,
            count,
            theme::small(),
            palette.text_tertiary,
        );
        right = painted.left() - space::S;
    }

    let searching = search.open
        || matches!(
            history.filter.as_ref().map(|filter| &filter.kind),
            Some(FilterKind::Search { .. })
        );
    if !searching {
        if let Some(filter) = &history.filter {
            text_line(
                ui,
                Pos2::new(rect.left() + space::M, center),
                &filter.kind.title(),
                theme::strong(),
                palette.text,
                (right - rect.left() - space::M).max(0.0),
            );
        }
        return;
    }

    // Kind chips, then the text field.
    let mut x = rect.left() + space::S;
    for kind in SearchKind::ALL {
        let label = kind.label();
        let width = text_button_width(ui, label);
        let chip = Rect::from_min_size(Pos2::new(x, center - 12.0), Vec2::new(width, 24.0));
        let active = kind == search.kind;
        if text_button_at(
            ui,
            chip,
            Id::unique(("search-kind", label)),
            label,
            kind.help(),
            if active {
                palette.text
            } else {
                palette.text_secondary
            },
            active,
        )
        .clicked()
        {
            actions.push(Action::SetSearchKind(kind));
        }
        x += width + space::XS;
    }
    x += space::XS;
    let field = Rect::from_min_max(Pos2::new(x, center - 13.0), Pos2::new(right, center + 13.0));
    if field.width() < 60.0 {
        return;
    }
    ui.painter().rect(
        field,
        CornerRadius::same(radius::M),
        palette.surface,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    let id = Id::unique("commit-search-field");
    let hint = match search.kind {
        SearchKind::Message => "Words in the commit message — Enter to search",
        SearchKind::Author => "Name or e-mail — Enter to search",
        SearchKind::Path => "Part of a file path — Enter to search",
        SearchKind::Pickaxe => "Text added or removed — Enter to search",
        SearchKind::DiffRegex => "Regular expression on changed lines — Enter to search",
    };
    let response = ui
        .scope_builder(UiBuilder::new().max_rect(field), |ui| {
            ui.add_sized(
                field.size(),
                TextEdit::singleline(&mut search.text)
                    .id(id)
                    .hint_text(hint)
                    .font(theme::body())
                    .align(Align2::LEFT_CENTER)
                    .frame(Frame::NONE.inner_margin(Margin::symmetric(space::S as i8, 0))),
            )
        })
        .inner;
    if std::mem::take(&mut search.focus) {
        response.request_focus();
    }
    if response.lost_focus() {
        let (enter, escape) = ui.input(|input| {
            (
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Escape),
            )
        });
        if enter {
            actions.push(Action::RunSearch);
            search.focus = true;
        } else if escape {
            actions.push(Action::ClearFilter);
        }
    }
}

impl FacetApp {
    /// "Rebase in progress" with Continue / Skip / Abort. Returns whether it was drawn.
    pub(super) fn operation_banner(&self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let Some(operation) = &tab.power.operation else {
            return;
        };
        let palette = Palette::of(ui.ctx());
        ui.painter()
            .rect_filled(rect, 0.0, theme::fade(palette.warning, 0.14));
        let center = rect.center().y;
        let conflicts = tab
            .work
            .entries
            .iter()
            .filter(|entry| entry.conflict.is_some())
            .count();

        let mut text = format!("{} in progress", operation.kind.label());
        if let Some((step, total)) = operation.step {
            text.push_str(&format!(" · step {step} of {total}"));
        }
        if let Some(branch) = &operation.branch {
            text.push_str(&format!(" · {branch}"));
        }
        if let Some(commit) = &operation.commit {
            text.push_str(&format!(" · at {}", short(commit)));
        }
        let status = match conflicts {
            0 => "No conflicts left".to_owned(),
            1 => "1 conflicted file".to_owned(),
            n => format!("{n} conflicted files"),
        };

        let buttons = Rect::from_min_max(
            Pos2::new(rect.center().x, rect.top()),
            Pos2::new(rect.right() - space::M, rect.bottom()),
        );
        let kind = operation.kind;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("operation-banner")
                .max_rect(buttons)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                if kind.can_continue() {
                    let tooltip = if conflicts > 0 {
                        "Resolve every conflicted file first (Changes view)".to_owned()
                    } else {
                        format!("Continue the {}", kind.label().to_lowercase())
                    };
                    if button_enabled(
                        ui,
                        ButtonKind::Primary,
                        None,
                        "Continue",
                        &tooltip,
                        conflicts == 0,
                    )
                    .clicked()
                    {
                        actions.push(Action::ContinueOperation);
                    }
                }
                if kind.can_skip()
                    && button(
                        ui,
                        ButtonKind::Secondary,
                        None,
                        "Skip",
                        "Leave out the commit being applied and go on",
                    )
                    .clicked()
                {
                    actions.push(Action::SkipOperation);
                }
                if button(
                    ui,
                    ButtonKind::Secondary,
                    None,
                    "Abort…",
                    "Stop and put everything back as it was (asks first)",
                )
                .clicked()
                {
                    actions.push(Action::AbortOperation);
                }
            },
        );
        let mut x = rect.left() + space::M;
        x += text_line(
            ui,
            Pos2::new(x, center),
            &text,
            theme::strong(),
            palette.text,
            (buttons.left() - x).max(0.0) * 0.7,
        );
        text_line(
            ui,
            Pos2::new(x + space::M, center),
            &status,
            theme::small(),
            if conflicts > 0 {
                palette.danger
            } else {
                palette.text_secondary
            },
            (buttons.left() - x - space::M).max(0.0),
        );
    }

    /// Who last changed each line of a file, in place of the view's body.
    pub(super) fn blame_view(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let Some(index) = self.active else {
            return;
        };
        let Some(blame) = self.tabs[index].power.blame.as_mut() else {
            return;
        };
        let palette = Palette::of(ui.ctx());
        ui.painter().rect_filled(rect, 0.0, palette.surface);

        let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), BLAME_HEADER));
        let center = header.center().y;
        let close = Rect::from_center_size(
            Pos2::new(header.right() - space::M - 12.0, center),
            Vec2::splat(28.0),
        );
        if icon_button_at(
            ui,
            close,
            Id::unique("blame-close"),
            icon::CLOSE,
            "Close the blame  (Esc)",
        )
        .clicked()
        {
            actions.push(Action::CloseBlame);
        }
        let history_label = "File History";
        let width = text_button_width(ui, history_label);
        let history_button = Rect::from_min_max(
            Pos2::new(close.left() - space::S - width, center - 12.0),
            Pos2::new(close.left() - space::S, center + 12.0),
        );
        if text_button_at(
            ui,
            history_button,
            Id::unique("blame-history"),
            history_label,
            "Every commit that changed this file",
            palette.text_secondary,
            false,
        )
        .clicked()
        {
            actions.push(Action::CloseBlame);
            actions.push(Action::ShowFileHistory(blame.path.clone()));
        }
        let mut x = header.left() + space::M;
        x += text_line(
            ui,
            Pos2::new(x, center),
            &format!("Blame: {}", file_name(&blame.path)),
            theme::strong(),
            palette.text,
            (history_button.left() - x - space::M).max(0.0) * 0.6,
        );
        let at = match &blame.rev {
            Some(rev) => format!("at {}", short(rev)),
            None => "working copy".to_owned(),
        };
        let state = if blame.loading {
            format!("{at} · reading history…")
        } else {
            at
        };
        text_line(
            ui,
            Pos2::new(x + space::S, center),
            &format!("{} · {state}", blame.path),
            theme::small(),
            palette.text_tertiary,
            (history_button.left() - x - space::M).max(0.0),
        );
        widgets::hairline_h(ui, rect.x_range().into(), header.bottom());

        let body = Rect::from_min_max(Pos2::new(rect.left(), header.bottom() + 1.0), rect.max);
        if let Some(error) = &blame.error {
            ui.painter().text(
                body.center(),
                Align2::CENTER_CENTER,
                error,
                theme::body(),
                palette.danger,
            );
            return;
        }
        if blame.lines.is_empty() {
            if blame.loading {
                widgets::skeleton(
                    ui,
                    Rect::from_min_size(
                        body.min + Vec2::new(space::M, space::M),
                        Vec2::new(body.width() * 0.5, 12.0),
                    ),
                );
            } else {
                ui.painter().text(
                    body.center(),
                    Align2::CENTER_CENTER,
                    "The file is empty",
                    theme::body(),
                    palette.text_secondary,
                );
            }
            return;
        }
        let now = unix_seconds(SystemTime::now());
        super::region(ui, "blame-body", body, |ui| {
            blame_lines(ui, blame, now, actions);
        });
    }

    /// The interactive rebase editor (modal).
    pub(super) fn rebase_layer(&mut self, ui: &mut Ui, window: Rect) {
        let Some(index) = self.active else {
            return;
        };
        let Some(editor) = self
            .tabs
            .get_mut(index)
            .and_then(|tab| tab.power.rebase.as_mut())
        else {
            return;
        };
        let ctx = ui.ctx().clone();
        let palette = Palette::of(&ctx);
        let mut actions = Vec::new();

        // Keys: the editor owns the keyboard while it is open.
        let typing = ctx.memory(|memory| memory.focused().is_some());
        ctx.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::Escape) && !typing {
                actions.push(Action::CancelRebase);
            }
            if input.consume_key(Modifiers::CTRL, Key::Enter) {
                actions.push(Action::StartRebase);
            }
            if input.consume_key(Modifiers::ALT, Key::ArrowUp) {
                actions.push(Action::RebaseMoveRow {
                    row: editor.selected,
                    step: -1,
                });
            }
            if input.consume_key(Modifiers::ALT, Key::ArrowDown) {
                actions.push(Action::RebaseMoveRow {
                    row: editor.selected,
                    step: 1,
                });
            }
            if !typing {
                if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    editor.selected = editor.selected.saturating_sub(1);
                }
                if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    editor.selected =
                        (editor.selected + 1).min(editor.items.len().saturating_sub(1));
                }
                for (key, action) in [
                    (Key::P, TodoAction::Pick),
                    (Key::R, TodoAction::Reword),
                    (Key::E, TodoAction::Edit),
                    (Key::S, TodoAction::Squash),
                    (Key::F, TodoAction::Fixup),
                    (Key::D, TodoAction::Drop),
                ] {
                    if input.consume_key(Modifiers::NONE, key) {
                        actions.push(Action::SetRebaseStep {
                            row: editor.selected,
                            action,
                        });
                    }
                }
            }
        });

        let panel = modal_panel(window, 760.0, 0.8);
        modal_area(&ctx, "rebase-editor", window, panel, palette, |ui| {
            let inner = panel.shrink(space::L);
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("rebase-editor-body")
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Min)),
                |ui| rebase_editor(ui, inner, editor, &mut actions),
            );
        });
        self.pending.extend(actions);
    }

    /// The chunk-by-chunk conflict resolver (modal).
    pub(super) fn conflict_layer(&mut self, ui: &mut Ui, window: Rect) {
        let Some(index) = self.active else {
            return;
        };
        let merge_tool = self.tabs.get(index).and_then(|tab| {
            let editor = tab.power.conflict.as_ref()?;
            Some(self.settings.external_tool(&editor.path, true).is_some())
        });
        let Some(merge_tool) = merge_tool else {
            return;
        };
        let Some(editor) = self.tabs[index].power.conflict.as_ref() else {
            return;
        };
        let ctx = ui.ctx().clone();
        let palette = Palette::of(&ctx);
        let mut actions = Vec::new();
        ctx.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::Escape) {
                actions.push(Action::CloseConflictEditor);
            }
            if input.consume_key(Modifiers::CTRL, Key::Enter)
                || input.consume_key(Modifiers::CTRL, Key::S)
            {
                actions.push(Action::SaveResolution);
            }
        });

        let panel = modal_panel(window, 1100.0, 0.86);
        modal_area(&ctx, "conflict-editor", window, panel, palette, |ui| {
            let inner = panel.shrink(space::L);
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("conflict-editor-body")
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Min)),
                |ui| conflict_editor(ui, inner, editor, merge_tool, &mut actions),
            );
        });
        self.pending.extend(actions);
    }
}

fn blame_lines(ui: &mut Ui, blame: &mut BlameView, now: i64, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    let digits = blame.lines.len().to_string().len() as f32;
    let number_width = digits * 8.0 + space::M;
    ui.spacing_mut().item_spacing.y = 0.0;
    ScrollArea::both()
        .id_salt("blame-scroll")
        .auto_shrink(false)
        .show_rows(ui, BLAME_ROW, blame.lines.len(), |ui, range| {
            let left = ui.max_rect().left();
            let width = ui.max_rect().width().max(ui.clip_rect().width());
            let top = ui.max_rect().top() - range.start as f32 * BLAME_ROW;
            let mut widest = 0.0f32;
            let mut select = None;
            for line in range {
                let row = Rect::from_min_size(
                    Pos2::new(left, top + line as f32 * BLAME_ROW),
                    Vec2::new(width, BLAME_ROW),
                );
                let owner = blame.owners.get(line).copied().unwrap_or(0);
                let commit = blame.owner(line);
                let selected = owner != 0 && blame.selected == Some(owner as usize);
                let gutter = Rect::from_min_size(row.min, Vec2::new(BLAME_GUTTER, BLAME_ROW));
                let response =
                    ui.interact(gutter, Id::unique(("blame-gutter", line)), Sense::click());
                // Alternate the gutter shade between runs so commits stand apart.
                let shade = if selected {
                    palette.accent_soft
                } else if response.hovered() {
                    interaction_fill(ui, &response, palette)
                } else if owner % 2 == 1 {
                    theme::fade(palette.hover, 0.5)
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(gutter, 0.0, shade);
                let center = row.center().y;
                match commit {
                    Some(commit) if blame.starts_run(line) => {
                        let id = if commit.uncommitted {
                            "———————".to_owned()
                        } else {
                            short(&commit.oid).to_owned()
                        };
                        let mut x = gutter.left() + space::S;
                        x += text_line(
                            ui,
                            Pos2::new(x, center),
                            &id,
                            theme::mono(),
                            palette.text_tertiary,
                            64.0,
                        ) + space::S;
                        let when = if commit.uncommitted {
                            "not committed".to_owned()
                        } else {
                            relative_time(now, commit.time)
                        };
                        let when_width = 86.0;
                        text_line(
                            ui,
                            Pos2::new(gutter.right() - space::S - when_width, center),
                            &when,
                            theme::small(),
                            palette.text_tertiary,
                            when_width,
                        );
                        text_line(
                            ui,
                            Pos2::new(x, center),
                            &commit.author,
                            theme::small(),
                            palette.text_secondary,
                            (gutter.right() - space::M - when_width - x).max(0.0),
                        );
                    }
                    _ => {}
                }
                if let Some(commit) = commit {
                    if response.clicked() {
                        select = Some(owner as usize);
                    }
                    if response.double_clicked() && !commit.uncommitted {
                        actions.push(Action::CloseBlame);
                        actions.push(Action::ClearFilter);
                        actions.push(Action::RevealCommit(commit.oid.clone()));
                    }
                    let summary = if commit.uncommitted {
                        "Not committed yet".to_owned()
                    } else {
                        format!(
                            "{} · {}\n{}\n\nDouble-click to show it in the history",
                            short(&commit.oid),
                            commit.author,
                            commit.summary
                        )
                    };
                    response.on_hover_text(summary);
                }
                ui.painter().vline(
                    gutter.right() + 0.5,
                    row.y_range(),
                    palette.hairline_stroke(),
                );
                ui.painter().text(
                    Pos2::new(gutter.right() + number_width - space::S, center),
                    Align2::RIGHT_CENTER,
                    (line + 1).to_string(),
                    theme::mono(),
                    palette.text_tertiary,
                );
                let text_left = gutter.right() + number_width + space::S;
                let galley = ui.painter().layout_no_wrap(
                    blame.lines[line].replace('\t', "    "),
                    theme::mono(),
                    palette.text,
                );
                widest = widest.max(galley.size().x);
                ui.painter().galley(
                    Pos2::new(text_left, center - galley.size().y / 2.0),
                    galley,
                    palette.text,
                );
            }
            if select.is_some() {
                blame.selected = select;
            }
            // Room for horizontal scrolling of long lines.
            ui.allocate_space(Vec2::new(
                BLAME_GUTTER + number_width + space::M + widest,
                0.0,
            ));
        });
}

/// Where a modal panel goes: centered, `max_width` wide at most, `height_share` of the window.
pub(super) fn modal_panel(window: Rect, max_width: f32, height_share: f32) -> Rect {
    let width = max_width.min(window.width() - space::XL * 2.0);
    let height = (window.height() * height_share)
        .max(320.0)
        .min(window.height() - space::L);
    Rect::from_center_size(
        Pos2::new(
            window.center().x,
            window.center().y + theme::TITLE_BAR_HEIGHT / 4.0,
        ),
        Vec2::new(width, height),
    )
}

/// Scrim plus a raised panel in the foreground; the contents draw over the panel.
pub(super) fn modal_area(
    ctx: &egui::Context,
    salt: &'static str,
    window: Rect,
    panel: Rect,
    palette: &Palette,
    contents: impl FnOnce(&mut Ui),
) {
    let visible = ctx.animate_bool_with_time_and_easing(
        Id::unique((salt, "visible")),
        true,
        motion::NORMAL,
        motion::ease_out,
    );
    egui::Area::new(Id::unique(salt))
        .order(Order::Foreground)
        .fixed_pos(window.min)
        .show(ctx, |ui| {
            ui.set_opacity(visible);
            ui.allocate_rect(window, Sense::click());
            ui.painter().rect_filled(window, 0.0, palette.scrim);
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
            ui.interact(panel, Id::unique((salt, "panel")), Sense::click());
            contents(ui);
        });
}

fn rebase_editor(ui: &mut Ui, rect: Rect, editor: &mut RebaseEditor, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    ui.label(
        RichText::new("Interactive Rebase")
            .font(theme::title())
            .color(palette.text),
    );
    ui.add_space(space::XS);
    let count = editor.items.len();
    let summary = if editor.loading {
        "Reading the commits…".to_owned()
    } else {
        format!(
            "{count} commit{} after {}, oldest first. Choose what happens to each; reorder by \
             dragging the handle or with Alt+↑ / Alt+↓. Keys: P R E S F D.",
            if count == 1 { "" } else { "s" },
            editor.base_label
        )
    };
    ui.label(
        RichText::new(summary)
            .font(theme::small())
            .color(palette.text_secondary),
    );
    ui.add_space(space::S);

    let footer = theme::ROW_HEIGHT + space::M;
    let error_height = if editor.error.is_some() { 28.0 } else { 0.0 };
    let list_height = (rect.bottom() - ui.cursor().top() - footer - error_height).max(80.0);
    ScrollArea::vertical()
        .id_salt("rebase-rows")
        .max_height(list_height)
        .min_scrolled_height(list_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if editor.loading {
                for _ in 0..4 {
                    let (rect, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
                    widgets::skeleton(ui, rect.shrink2(Vec2::new(0.0, 8.0)));
                }
                return;
            }
            let last = count.saturating_sub(1);
            let mut rows = Vec::with_capacity(count);
            let mut dragged = None;
            for row in 0..count {
                let (rect, grip) = rebase_row(ui, editor, row, last, actions);
                rows.push(rect);
                if grip.drag_started() {
                    editor.selected = row;
                }
                if grip.dragged() || grip.drag_stopped() {
                    dragged = Some((row, grip.drag_stopped()));
                }
            }
            if let Some((row, released)) = dragged {
                drag_rebase_row(ui, &rows, row, released, actions);
            }
        });

    if let Some(error) = &editor.error {
        ui.add_space(space::XS);
        ui.label(
            RichText::new(error)
                .font(theme::small())
                .color(palette.danger),
        );
    }

    let buttons = Rect::from_min_max(
        Pos2::new(rect.left(), rect.bottom() - theme::ROW_HEIGHT),
        rect.max,
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("rebase-buttons")
            .max_rect(buttons)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            if button_enabled(
                ui,
                ButtonKind::Primary,
                None,
                "Start Rebase",
                "Rewrite the commits as planned (Ctrl+Enter); you can undo it",
                !editor.loading && count > 0,
            )
            .clicked()
            {
                actions.push(Action::StartRebase);
            }
            if button(
                ui,
                ButtonKind::Secondary,
                None,
                "Cancel",
                "Close without changing anything  (Esc)",
            )
            .clicked()
            {
                actions.push(Action::CancelRebase);
            }
        },
    );
}

/// A row of the rebase editor is being dragged by its handle: show where it would land, and
/// move it there on release. `rows` are the rows' rects in their current order.
fn drag_rebase_row(
    ui: &mut Ui,
    rows: &[Rect],
    row: usize,
    released: bool,
    actions: &mut Vec<Action>,
) {
    let Some(pointer) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    // The row's index once moved: how many other rows would sit above it.
    let target = rows
        .iter()
        .enumerate()
        .filter(|(other, rect)| *other != row && rect.center().y < pointer.y)
        .count();
    if released {
        if target != row {
            actions.push(Action::RebaseMoveRow {
                row,
                step: target as isize - row as isize,
            });
        }
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    if target == row {
        return;
    }
    // Rows past the dragged one move up a place, so it lands below row `target` when
    // moving down and above it when moving up.
    let y = if target > row {
        rows[target].bottom() + 1.0
    } else {
        rows[target].top() - 1.0
    };
    let palette = Palette::of(ui.ctx());
    let span = rows[row].x_range();
    ui.painter()
        .hline(span, y, egui::Stroke::new(2.0, palette.accent));
}

/// One commit of the rebase editor. Returns the row's rect (with the reword message) and its
/// drag handle.
fn rebase_row(
    ui: &mut Ui,
    editor: &mut RebaseEditor,
    row: usize,
    last: usize,
    actions: &mut Vec<Action>,
) -> (Rect, egui::Response) {
    let palette = Palette::of(ui.ctx());
    let selected = editor.selected == row;
    let item = &mut editor.items[row];
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    if response.clicked() {
        editor.selected = row;
    }
    let fill = if selected {
        palette.accent_soft
    } else {
        interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);
    let center = rect.center().y;

    // Drag handle on the left.
    let grip_rect = Rect::from_min_size(rect.min, Vec2::new(24.0, rect.height()));
    let grip = ui
        .interact(
            grip_rect,
            Id::unique(("rebase-grip", row)),
            Sense::click_and_drag(),
        )
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to reorder");
    ui.painter().text(
        grip_rect.center(),
        Align2::CENTER_CENTER,
        icon::GRIPPER,
        theme::icon_font(12.0),
        if grip.hovered() || grip.dragged() {
            palette.text_secondary
        } else {
            palette.text_tertiary
        },
    );

    // Move buttons on the right.
    let down = Rect::from_center_size(
        Pos2::new(rect.right() - space::XS - 12.0, center),
        Vec2::splat(24.0),
    );
    let up = down.translate(Vec2::new(-26.0, 0.0));
    if row < last
        && icon_button_at(
            ui,
            down,
            Id::unique(("rebase-down", row)),
            icon::CHEVRON_DOWN,
            "Move down  (Alt+↓)",
        )
        .clicked()
    {
        actions.push(Action::RebaseMoveRow { row, step: 1 });
    }
    if row > 0
        && icon_button_at(
            ui,
            up,
            Id::unique(("rebase-up", row)),
            icon::CHEVRON_UP,
            "Move up  (Alt+↑)",
        )
        .clicked()
    {
        actions.push(Action::RebaseMoveRow { row, step: -1 });
    }

    // Action segments.
    let mut x = grip_rect.right() + space::XS;
    for action in TodoAction::ALL {
        let label = action.label();
        let width = text_button_width(ui, label);
        let chip = Rect::from_min_size(Pos2::new(x, center - 12.0), Vec2::new(width, 24.0));
        let active = item.action == action;
        let color = match action {
            _ if !active => palette.text_tertiary,
            TodoAction::Drop => palette.danger,
            TodoAction::Pick => palette.text,
            _ => palette.accent,
        };
        if text_button_at(
            ui,
            chip,
            Id::unique(("rebase-action", row, label)),
            label,
            action.help(),
            color,
            active,
        )
        .clicked()
        {
            actions.push(Action::SetRebaseStep { row, action });
        }
        x += width + 2.0;
    }
    x += space::S;
    x += text_line(
        ui,
        Pos2::new(x, center),
        short(&item.oid),
        theme::mono(),
        palette.text_tertiary,
        64.0,
    ) + space::S;
    let subject_color = if item.action == TodoAction::Drop {
        palette.text_tertiary
    } else {
        palette.text
    };
    text_line(
        ui,
        Pos2::new(x, center),
        &item.subject,
        theme::body(),
        subject_color,
        (up.left() - space::S - x).max(0.0),
    );

    if item.action == TodoAction::Reword {
        if let Some(message) = &mut item.message {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(space::L);
                ui.add(
                    TextEdit::multiline(message)
                        .id(Id::unique(("rebase-message", row)))
                        .font(theme::body())
                        .desired_rows(2)
                        .desired_width(ui.available_width() - space::S)
                        .hint_text("The new commit message"),
                );
            });
            ui.add_space(space::XS);
        }
    }
    let bottom = ui.min_rect().bottom().max(rect.bottom());
    (
        Rect::from_min_max(rect.min, Pos2::new(rect.right(), bottom)),
        grip,
    )
}

fn conflict_editor(
    ui: &mut Ui,
    rect: Rect,
    editor: &ConflictEditor,
    merge_tool: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let name = file_name(&editor.path);
    ui.label(
        RichText::new(format!("Resolve Conflicts: {name}"))
            .font(theme::title())
            .color(palette.text),
    );
    ui.add_space(space::XS);
    let status = if editor.loading {
        "Reading the file…".to_owned()
    } else if editor.regions.is_some() {
        format!(
            "{} · {} of {} conflicts resolved. Pick a side for each; the result replaces the \
             file and marks it resolved.",
            editor.path,
            editor.resolved(),
            editor.chunks()
        )
    } else {
        editor.path.clone()
    };
    ui.label(
        RichText::new(status)
            .font(theme::small())
            .color(palette.text_secondary),
    );
    ui.add_space(space::S);

    let footer = theme::ROW_HEIGHT + space::M;
    let body_height = (rect.bottom() - ui.cursor().top() - footer).max(80.0);
    ScrollArea::vertical()
        .id_salt("conflict-regions")
        .max_height(body_height)
        .min_scrolled_height(body_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(error) = &editor.error {
                ui.label(
                    RichText::new(error)
                        .font(theme::body())
                        .color(palette.text_secondary),
                );
                return;
            }
            let Some(regions) = &editor.regions else {
                return;
            };
            let mut chunk = 0;
            for region in regions {
                match region {
                    Region::Common(text) => common_text(ui, text, palette),
                    Region::Conflict(conflict) => {
                        let choice = editor.choices.get(chunk).copied().flatten();
                        conflict_chunk(ui, chunk, conflict, choice, actions);
                        chunk += 1;
                    }
                }
            }
        });

    let buttons = Rect::from_min_max(
        Pos2::new(rect.left(), rect.bottom() - theme::ROW_HEIGHT),
        rect.max,
    );
    let path = editor.path.clone();
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("conflict-buttons")
            .max_rect(buttons)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            let ready = editor.regions.is_some() && editor.resolved() == editor.chunks();
            if button_enabled(
                ui,
                ButtonKind::Primary,
                None,
                "Save and Mark Resolved",
                if ready {
                    "Write the result and stage the file  (Ctrl+S)"
                } else {
                    "Choose a side for every conflict first"
                },
                ready,
            )
            .clicked()
            {
                actions.push(Action::SaveResolution);
            }
            if button(
                ui,
                ButtonKind::Secondary,
                None,
                "Cancel",
                "Close without changing the file  (Esc)",
            )
            .clicked()
            {
                actions.push(Action::CloseConflictEditor);
            }
            ui.add_space(space::M);
            if merge_tool
                && button(
                    ui,
                    ButtonKind::Secondary,
                    None,
                    "Merge Tool",
                    "Open the file in the external merge tool set up for its type",
                )
                .clicked()
            {
                actions.push(Action::CloseConflictEditor);
                actions.push(Action::OpenMergeTool(path.clone()));
            }
            for (label, side, tooltip) in [
                (
                    "Whole File: Theirs",
                    Side::Theirs,
                    "Take the incoming version of the whole file",
                ),
                (
                    "Whole File: Ours",
                    Side::Ours,
                    "Keep the current branch's version of the whole file",
                ),
            ] {
                if button(ui, ButtonKind::Secondary, None, label, tooltip).clicked() {
                    actions.push(Action::CloseConflictEditor);
                    actions.push(Action::TakeSide {
                        path: path.clone(),
                        side,
                    });
                }
            }
            if editor.chunks() > 1 {
                ui.add_space(space::M);
                for choice in [Choice::Theirs, Choice::Ours] {
                    let label = match choice {
                        Choice::Ours => "All Ours",
                        _ => "All Theirs",
                    };
                    if button(
                        ui,
                        ButtonKind::Secondary,
                        None,
                        label,
                        "Choose this side for every conflict",
                    )
                    .clicked()
                    {
                        actions.push(Action::ChooseAllChunks(choice));
                    }
                }
            }
        },
    );
}

/// Text both sides agree on: shown dimmed, long stretches shortened to their ends.
fn common_text(ui: &mut Ui, text: &str, palette: &Palette) {
    const KEEP: usize = 3;
    let lines: Vec<&str> = text.lines().collect();
    let shown = if lines.len() > KEEP * 2 + 1 {
        let hidden = lines.len() - KEEP * 2;
        format!(
            "{}\n   ⋯ {hidden} unchanged lines ⋯\n{}",
            lines[..KEEP].join("\n"),
            lines[lines.len() - KEEP..].join("\n")
        )
    } else {
        lines.join("\n")
    };
    if shown.is_empty() {
        return;
    }
    ui.label(
        RichText::new(shown.replace('\t', "    "))
            .font(theme::mono())
            .color(palette.text_tertiary),
    );
    ui.add_space(space::XS);
}

fn conflict_chunk(
    ui: &mut Ui,
    chunk: usize,
    conflict: &facet_git::conflict::Chunk,
    choice: Option<Choice>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let stroke = if choice.is_some() {
        egui::Stroke::new(1.0, palette.success)
    } else {
        egui::Stroke::new(1.0, palette.warning)
    };
    Frame::NONE
        .stroke(stroke)
        .corner_radius(CornerRadius::same(radius::M))
        .inner_margin(Margin::same(space::S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Choices.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Conflict {}", chunk + 1))
                        .font(theme::small_strong())
                        .color(palette.text_secondary),
                );
                ui.add_space(space::S);
                for option in Choice::ALL {
                    let active = choice == Some(option);
                    let label = option.label();
                    let width = text_button_width(ui, label);
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
                    if text_button_at(
                        ui,
                        rect,
                        Id::unique(("conflict-choice", chunk, label)),
                        label,
                        "",
                        if active {
                            palette.accent
                        } else {
                            palette.text_secondary
                        },
                        active,
                    )
                    .clicked()
                    {
                        actions.push(Action::ChooseChunk {
                            chunk,
                            choice: if active { None } else { Some(option) },
                        });
                    }
                }
            });
            ui.add_space(space::XS);
            // Ours, the common ancestor (when known) and theirs, next to each other.
            let both = matches!(
                choice,
                Some(Choice::OursThenTheirs | Choice::TheirsThenOurs)
            );
            let mut columns = vec![(
                format!("Ours · {}", conflict.ours_label),
                &conflict.ours,
                both || choice == Some(Choice::Ours),
                false,
            )];
            if let Some(base) = &conflict.base {
                columns.push(("Base · common ancestor".to_owned(), base, false, true));
            }
            columns.push((
                format!("Theirs · {}", conflict.theirs_label),
                &conflict.theirs,
                both || choice == Some(Choice::Theirs),
                false,
            ));
            let gaps = space::S * (columns.len() - 1) as f32;
            let column_width = (ui.available_width() - gaps) / columns.len() as f32;
            ui.horizontal_top(|ui| {
                for (title, text, picked, is_base) in columns {
                    let fill = if picked {
                        theme::fade(palette.success, 0.10)
                    } else if choice.is_some() || is_base {
                        Color32::TRANSPARENT
                    } else {
                        palette.hover
                    };
                    Frame::NONE
                        .fill(fill)
                        .corner_radius(CornerRadius::same(radius::S))
                        .inner_margin(Margin::same(space::XS as i8))
                        .show(ui, |ui| {
                            ui.set_width(column_width - space::S);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(title)
                                        .font(theme::small_strong())
                                        .color(palette.text_secondary),
                                );
                                let body = if text.is_empty() {
                                    "(nothing)".to_owned()
                                } else {
                                    text.trim_end_matches(['\r', '\n'])
                                        .replace('\r', "")
                                        .replace('\t', "    ")
                                };
                                ui.label(RichText::new(body).font(theme::mono()).color(
                                    if is_base || (choice.is_some() && !picked) {
                                        palette.text_tertiary
                                    } else {
                                        palette.text
                                    },
                                ));
                            });
                        });
                }
            });
        });
    ui.add_space(space::S);
}
