//! The history view: the filter field and the virtualized commit graph on the left; the
//! selected commit on the right as a page — title, author, message, actions and a card of
//! the files it changed — that opens a file's diff in its place.
//!
//! Every row is drawn from its own `GraphRow` (lanes, segments, node), so a frame only touches
//! the rows on screen — the list is as cheap at 100k commits as at 100.

use std::time::SystemTime;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Key, Pos2, Rect, ScrollArea, Sense,
    Shape, Stroke, StrokeKind, Ui, Vec2,
};
use facet_git::forge::ForgeKind;
use facet_git::graph::{GraphRow, Half};
use facet_git::parse::{ChangedFile, CommitDetail, LogCommit};
use facet_git::{ResetMode, SearchKind};

use super::diff::{diff_body, plain_diff_header};
use super::power::filter_bar;
use super::widgets::{
    self, change_bar, interaction_fill, outline_button_at, outline_button_width, skeleton,
    status_badge, text_line,
};
use super::{region, splitter_feedback, SPLITTER_GRAB};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::format::{relative_time, unix_seconds};
use crate::history::{Badge, BadgeKind, DetailBody, FilterKind, History};
use crate::power::SearchBar;
use crate::settings::{Settings, DETAIL_WIDTH};
use crate::theme::{self, icon, radius, space, Palette};
use crate::workspace::Area;

/// Height of a commit row: subject over "author · time".
/// Height of a commit row (shorter in the Compact density).
pub fn commit_row_height() -> f32 {
    theme::dense(52.0, 40.0)
}
/// The filter field above the list, and the diff header of an opened file.
const BAR: f32 = 48.0;
/// A file row of a commit or a stash.
/// Height of a file row (shorter in the Compact density).
fn file_row() -> f32 {
    theme::dense(40.0, 32.0)
}
/// Width of one graph lane; lanes narrow towards `MIN_LANE` when many are open.
const LANE: f32 = 16.0;
const MIN_LANE: f32 = 8.0;
/// Center of the first lane from the row's left edge, and the narrowest graph column.
const GRAPH_LEFT: f32 = 14.0;
const GRAPH_MIN_WIDTH: f32 = 44.0;
/// The graph takes at most this share of the list width. Lanes that still do not fit are
/// drawn on the last visible lane: hundreds of concurrent branches must not push the
/// subjects off screen.
const GRAPH_SHARE: f32 = 0.4;
const NODE_RADIUS: f32 = 4.5;
/// Side padding of the list and of its rows.
const LIST_PAD: f32 = 6.0;
/// Padding of the commit page.
const PAGE_PAD_X: f32 = 32.0;
const PAGE_PAD_Y: f32 = 28.0;

/// Lane width and how many lanes are drawn, for the current list width.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GraphLayout {
    lane: f32,
    lanes: u16,
}

impl GraphLayout {
    fn new(max_lanes: u16, list_width: f32) -> Self {
        let wanted = max_lanes.max(1);
        let share = (list_width * GRAPH_SHARE).max(LANE);
        let lane = (share / f32::from(wanted)).clamp(MIN_LANE, LANE);
        let fit = ((share / lane).floor() as u16).max(1);
        Self {
            lane,
            lanes: wanted.min(fit),
        }
    }

    fn width(self) -> f32 {
        (GRAPH_LEFT + f32::from(self.lanes - 1) * self.lane + GRAPH_LEFT).max(GRAPH_MIN_WIDTH)
    }

    fn node_radius(self) -> f32 {
        NODE_RADIUS.min(self.lane * 0.34)
    }
}
const MIN_LIST_WIDTH: f32 = 360.0;

/// Lane colors after the first two (accent, orange), picked to stay apart from each other and
/// readable on both themes.
const LANES_DARK: [Color32; 6] = [
    Color32::from_rgb(0x7B, 0xD8, 0x8F),
    Color32::from_rgb(0xE8, 0x6A, 0xA6),
    Color32::from_rgb(0xB4, 0x8C, 0xF2),
    Color32::from_rgb(0x4F, 0xD1, 0xC5),
    Color32::from_rgb(0xF2, 0xCC, 0x60),
    Color32::from_rgb(0xFF, 0x7A, 0x7A),
];
const LANES_LIGHT: [Color32; 6] = [
    Color32::from_rgb(0x2E, 0x9A, 0x4A),
    Color32::from_rgb(0xC2, 0x39, 0x7A),
    Color32::from_rgb(0x7A, 0x4F, 0xD0),
    Color32::from_rgb(0x12, 0x8A, 0x80),
    Color32::from_rgb(0xA8, 0x83, 0x10),
    Color32::from_rgb(0xD1, 0x3B, 0x3B),
];

fn lane_color(palette: &Palette, color: u16) -> Color32 {
    let rest = if palette.dark {
        &LANES_DARK
    } else {
        &LANES_LIGHT
    };
    match usize::from(color) % (rest.len() + 2) {
        0 => palette.accent,
        1 => palette.orange,
        n => rest[n - 2],
    }
}

/// The name of a hosting service for menu titles.
pub fn forge_name(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "GitHub",
        ForgeKind::GitLab => "GitLab",
        ForgeKind::AzureDevOps => "Azure DevOps",
    }
}

/// A menu entry that queues `action` and closes the menu.
pub(super) fn menu_item(ui: &mut Ui, label: &str, action: Action, actions: &mut Vec<Action>) {
    if ui.button(label).clicked() {
        actions.push(action);
        ui.close();
    }
}

/// `folder/` and `name` of a path.
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(slash) => (&path[..slash], &path[slash + 1..]),
        None => ("", path),
    }
}

impl FacetApp {
    /// The history body below the title bar.
    pub(super) fn history_view(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let palette = Palette::of(ui.ctx());
        let Some(index) = self.active else {
            return;
        };
        let width = self
            .settings
            .detail_width
            .min((rect.width() - MIN_LIST_WIDTH).max(*DETAIL_WIDTH.start()))
            .round();
        let list = Rect::from_min_max(
            rect.min,
            Pos2::new(rect.right() - width - 1.0, rect.bottom()),
        );
        let detail = Rect::from_min_max(Pos2::new(list.right() + 1.0, rect.top()), rect.max);
        let forge = self.forge().map(|forge| forge.kind);
        let mode = self.settings.image_mode;
        let all_branches = self.settings.history_all_branches;
        let context = self.repo_context();
        let rewrite = context
            .as_ref()
            .filter(|cx| !cx.bare && cx.operation.is_none())
            .map(|cx| Rewrite {
                branch: cx.branch.clone(),
            });
        let tab = &mut self.tabs[index];

        ui.painter().rect_filled(rect, 0.0, palette.surface);

        // The filter field (or the title of a file's history / the reflog) above the list.
        let bar = Rect::from_min_size(list.min, Vec2::new(list.width(), BAR));
        region(ui, "history-bar", bar, |ui| {
            let titled = matches!(
                tab.history.filter.as_ref().map(|filter| &filter.kind),
                Some(FilterKind::File { .. } | FilterKind::Reflog)
            );
            if titled {
                filter_bar(ui, bar, &mut tab.power.search, &tab.history, actions);
            } else {
                history_bar(
                    ui,
                    bar,
                    &mut tab.power.search,
                    &tab.history,
                    all_branches,
                    actions,
                );
            }
        });
        widgets::hairline_h(ui, list.x_range().into(), bar.bottom());
        let list_body = Rect::from_min_max(Pos2::new(list.left(), bar.bottom() + 1.0), list.max);

        let rows = RowContext {
            forge,
            rewrite: rewrite.as_ref(),
        };
        region(ui, "commit-list", list_body, |ui| {
            commit_list(
                ui,
                list_body,
                &mut tab.history,
                &tab.refs.badges,
                &rows,
                actions,
            );
        });
        ui.painter().vline(
            list.right() + 0.5,
            rect.y_range(),
            palette.hairline_stroke(),
        );
        region(ui, "commit-detail", detail, |ui| {
            detail_pane(ui, detail, &mut tab.history, &rows, mode, actions);
        });

        // Splitter between the list and the detail.
        let grab = Rect::from_min_max(
            Pos2::new(list.right() - SPLITTER_GRAB / 2.0, rect.top()),
            Pos2::new(list.right() + SPLITTER_GRAB / 2.0, rect.bottom()),
        );
        let response = ui.interact(grab, Id::unique("detail-splitter"), Sense::click_and_drag());
        if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            self.settings.detail_width = (rect.right() - pointer.x)
                .clamp(*DETAIL_WIDTH.start(), *DETAIL_WIDTH.end())
                .round();
        }
        if response.double_clicked() {
            self.settings.detail_width = Settings::default().detail_width;
        }
        splitter_feedback(ui, &response, CursorIcon::ResizeHorizontal, |stroke| {
            ui.painter().vline(list.right(), rect.y_range(), stroke);
        });
    }
}

/// "Filter by message, author or hash" and the "All branches" menu.
fn history_bar(
    ui: &mut Ui,
    rect: Rect,
    search: &mut SearchBar,
    history: &History,
    all_branches: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let center = rect.center().y;

    // "All branches ⌄" on the right.
    let label = if all_branches {
        "All branches"
    } else {
        "Current branch"
    };
    let label_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::small(), palette.text)
        .size()
        .x;
    let scope = Rect::from_min_size(
        Pos2::new(rect.right() - 12.0 - (label_width + 40.0), center - 16.0),
        Vec2::new(label_width + 40.0, 32.0),
    );
    let response = ui.interact(scope, Id::unique("history-scope"), Sense::click());
    ui.painter().rect(
        scope,
        CornerRadius::same(radius::LAYER),
        palette.surface,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    ui.painter().rect_filled(
        scope.shrink(1.0),
        CornerRadius::same(radius::LAYER),
        interaction_fill(ui, &response, palette),
    );
    ui.painter().text(
        Pos2::new(scope.left() + 10.0, center),
        Align2::LEFT_CENTER,
        label,
        theme::small(),
        palette.text_secondary,
    );
    ui.painter().text(
        Pos2::new(scope.right() - 16.0, center + 1.0),
        Align2::CENTER_CENTER,
        icon::CHEVRON_DOWN,
        theme::icon_font(9.0),
        palette.text_secondary,
    );
    egui::Popup::menu(&response).show(|ui| {
        for (all, label, help) in [
            (true, "All branches", "Every branch, remote branch and tag"),
            (false, "Current branch", "Only commits the checked-out branch contains"),
        ] {
            let text = if all == all_branches {
                format!("✓  {label}")
            } else {
                format!("    {label}")
            };
            if ui.button(text).on_hover_text(help).clicked() {
                actions.push(Action::SetHistoryAllBranches(all));
                ui.close();
            }
        }
    });

    // The field.
    let field = Rect::from_min_max(
        Pos2::new(rect.left() + 12.0, center - 16.0),
        Pos2::new(scope.left() - space::S, center + 16.0),
    );
    if field.width() < 80.0 {
        return;
    }
    let searching = matches!(
        history.filter.as_ref().map(|filter| &filter.kind),
        Some(FilterKind::Search { .. })
    );
    let count = match (history.loading, history.commits.len()) {
        _ if !searching => String::new(),
        (true, 0) => "Searching…".to_owned(),
        (true, n) => format!("{n}…"),
        (false, 1) => "1 commit".to_owned(),
        (false, n) => format!("{n} commits"),
    };
    let count_width = if count.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(count.clone(), theme::small(), palette.text_tertiary)
            .size()
            .x
            + space::S
    };
    let clear_shown = searching || !search.text.is_empty();
    let inset = space::S + count_width + if clear_shown { 26.0 } else { 0.0 };
    let id = Id::unique("commit-search-field");
    let hint = match search.kind {
        SearchKind::Any => "Filter by message, author or hash".to_owned(),
        kind => format!("Search: {} — Enter", kind.label().to_lowercase()),
    };
    let response = widgets::search_field(ui, field, id, &mut search.text, &hint, inset);
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
        if enter && !search.text.trim().is_empty() {
            actions.push(Action::RunSearch);
        } else if (enter && searching) || escape {
            actions.push(Action::ClearFilter);
        }
    }
    let mut right = field.right() - space::S;
    if clear_shown {
        let clear = Rect::from_center_size(Pos2::new(right - 11.0, center), Vec2::splat(22.0));
        if widgets::icon_button_at(
            ui,
            clear,
            Id::unique("commit-search-clear"),
            icon::CLOSE,
            "Clear the filter  (Esc)",
        )
        .clicked()
        {
            search.text.clear();
            actions.push(Action::ClearFilter);
        }
        right = clear.left() - space::XS;
    }
    if !count.is_empty() {
        ui.painter().text(
            Pos2::new(right, center),
            Align2::RIGHT_CENTER,
            count,
            theme::small(),
            palette.text_tertiary,
        );
    }

    // The magnifier picks what the field searches.
    let kinds = Rect::from_min_size(field.min, Vec2::new(30.0, field.height()));
    let response = ui
        .interact(kinds, Id::unique("commit-search-kind"), Sense::click())
        .on_hover_text("Search by…");
    egui::Popup::menu(&response).show(|ui| {
        for kind in SearchKind::ALL {
            let text = if kind == search.kind {
                format!("✓  {}", kind.label())
            } else {
                format!("    {}", kind.label())
            };
            if ui.button(text).on_hover_text(kind.help()).clicked() {
                actions.push(Action::SetSearchKind(kind));
                ui.close();
            }
        }
    });
}

pub(super) fn centered_message(ui: &Ui, rect: Rect, title: &str, detail: &str) {
    let palette = Palette::of(ui.ctx());
    let center = rect.center();
    ui.painter().text(
        center - Vec2::new(0.0, 10.0),
        Align2::CENTER_CENTER,
        title,
        theme::strong(),
        palette.text_secondary,
    );
    if !detail.is_empty() {
        ui.painter().text(
            center + Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            detail,
            theme::small(),
            palette.text_tertiary,
        );
    }
}

fn list_skeleton(ui: &Ui, rect: Rect) {
    let mut y = rect.top() + space::XS;
    let mut row = 0;
    while y + commit_row_height() < rect.bottom() && row < 16 {
        let left = rect.left() + LIST_PAD;
        skeleton(
            ui,
            Rect::from_center_size(
                Pos2::new(left + GRAPH_LEFT, y + commit_row_height() / 2.0),
                Vec2::splat(NODE_RADIUS * 2.0),
            ),
        );
        let width = (rect.width() * [0.55, 0.4, 0.62, 0.35, 0.5][row % 5]).max(40.0);
        skeleton(
            ui,
            Rect::from_min_size(
                Pos2::new(left + GRAPH_MIN_WIDTH + space::XS, y + 15.0),
                Vec2::new(width, 10.0),
            ),
        );
        skeleton(
            ui,
            Rect::from_min_size(
                Pos2::new(left + GRAPH_MIN_WIDTH + space::XS, y + 31.0),
                Vec2::new(90.0, 8.0),
            ),
        );
        y += commit_row_height();
        row += 1;
    }
}

/// History-rewriting commands are offered on rows (not in a bare repository or while
/// another operation is in progress).
struct Rewrite {
    /// The checked-out branch (`None`: detached HEAD, which cannot be rebased).
    branch: Option<String>,
}

/// What every commit's menu needs.
struct RowContext<'a> {
    forge: Option<ForgeKind>,
    rewrite: Option<&'a Rewrite>,
}

fn commit_list(
    ui: &mut Ui,
    rect: Rect,
    history: &mut History,
    badges: &std::collections::HashMap<facet_git::Oid, Vec<Badge>>,
    context: &RowContext<'_>,
    actions: &mut Vec<Action>,
) {
    if history.commits.is_empty() {
        if history.loading || !history.loaded {
            list_skeleton(ui, rect);
        } else if let Some(filter) = &history.filter {
            centered_message(ui, rect, "Nothing found", &filter.kind.empty_hint());
        } else {
            centered_message(ui, rect, "No commits yet", "Commits you make appear here");
        }
        return;
    }
    let reveal = std::mem::take(&mut history.reveal)
        .then_some(history.selected)
        .flatten();
    let layout = GraphLayout::new(history.max_lanes, rect.width() - LIST_PAD * 2.0);
    let now = unix_seconds(SystemTime::now());
    let mut scroll = ScrollArea::vertical()
        .id_salt("commit-list-scroll")
        .auto_shrink(false);
    if let Some(offset) = history.scroll_to.take() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    let history = &*history;
    ui.spacing_mut().item_spacing.y = 0.0;
    scroll.show_rows(ui, commit_row_height(), history.commits.len(), |ui, range| {
        let left = ui.max_rect().left() + LIST_PAD;
        let width = ui.max_rect().width() - LIST_PAD * 2.0;
        let top = ui.max_rect().top() - range.start as f32 * commit_row_height();
        let row_rect = |row: usize| {
            Rect::from_min_size(
                Pos2::new(left, top + row as f32 * commit_row_height()),
                Vec2::new(width, commit_row_height()),
            )
        };
        if let Some(row) = reveal {
            ui.scroll_to_rect(row_rect(row), None);
        }
        for row in range {
            let (Some(commit), Some(graph)) = (history.commits.get(row), history.rows.get(row))
            else {
                continue;
            };
            let row_badges = badges.get(&commit.oid).map_or(&[][..], Vec::as_slice);
            let label = history
                .filter
                .as_ref()
                .and_then(|filter| filter.labels.get(row))
                .map(String::as_str)
                .filter(|label| !label.is_empty());
            commit_row(
                ui,
                row_rect(row),
                row,
                commit,
                graph,
                row_badges,
                label,
                history.selected == Some(row),
                layout,
                now,
                context,
                actions,
            );
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn commit_row(
    ui: &mut Ui,
    rect: Rect,
    row: usize,
    commit: &LogCommit,
    graph: &GraphRow,
    badges: &[Badge],
    label: Option<&str>,
    selected: bool,
    layout: GraphLayout,
    now: i64,
    context: &RowContext<'_>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, Id::unique(("commit-row", row)), Sense::click());
    if response.clicked() || response.secondary_clicked() {
        actions.push(Action::SelectCommit(row));
    }
    let fill = if selected {
        palette.accent_soft
    } else {
        interaction_fill(ui, &response, palette)
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::M), fill);
    }

    // Graph.
    let painter = ui.painter();
    let origin = rect.left() + GRAPH_LEFT;
    let node_radius = layout.node_radius();
    let lane_x = |lane: u16| origin + f32::from(lane.min(layout.lanes - 1)) * layout.lane;
    let mid = rect.center().y;
    for segment in &graph.segments {
        let (y0, y1) = match segment.half {
            Half::Top => (rect.top(), mid),
            Half::Bottom => (mid, rect.bottom()),
        };
        let stroke = Stroke::new(2.0, lane_color(palette, segment.color));
        painter.line_segment(
            [
                Pos2::new(lane_x(segment.from), y0),
                Pos2::new(lane_x(segment.to), y1),
            ],
            stroke,
        );
    }
    let node = Pos2::new(lane_x(graph.lane), mid);
    let color = lane_color(palette, graph.color);
    let is_head = badges
        .iter()
        .any(|badge| matches!(badge.kind, BadgeKind::Head | BadgeKind::Detached));
    let ring = palette.surface;
    if is_head {
        // Where you are: a ring.
        painter.add(Shape::circle_filled(node, node_radius + 0.5, ring));
        painter.add(Shape::circle_stroke(
            node,
            node_radius + 0.5,
            Stroke::new(2.5, color),
        ));
    } else if graph.merge {
        painter.add(Shape::circle_filled(node, node_radius, ring));
        painter.add(Shape::circle_stroke(
            node,
            node_radius - 0.5,
            Stroke::new(2.0, color),
        ));
    } else {
        painter.add(Shape::circle_filled(node, node_radius, color));
    }

    // Chips on the right: the reflog step / earlier file name, then branches and tags.
    let text_left = rect.left() + layout.width() + space::XS;
    let mut right = rect.right() - 10.0;
    let chip_limit = text_left + (right - text_left) * 0.45;
    let mut chips: Vec<Badge> = Vec::new();
    if let Some(label) = label {
        chips.push(Badge {
            label: label.to_owned(),
            kind: BadgeKind::Remote,
        });
    }
    chips.extend(badges.iter().cloned());
    for chip in &chips {
        let width = chip_width(ui, chip);
        if right - width < chip_limit {
            break;
        }
        draw_chip(ui, Pos2::new(right, mid), chip, color, palette);
        right -= width + space::XS;
    }

    // Subject over "author · time".
    let room = (right - space::XS - text_left).max(0.0);
    text_line(
        ui,
        Pos2::new(text_left, mid - 9.0),
        &commit.subject,
        if selected {
            theme::strong()
        } else {
            theme::body()
        },
        palette.text,
        room,
    );
    text_line(
        ui,
        Pos2::new(text_left, mid + 10.0),
        &format!("{} · {}", commit.author, relative_time(now, commit.time)),
        theme::small(),
        palette.text_tertiary,
        room,
    );

    let oid = commit.oid.to_string();
    let subject = commit.subject.to_string();
    response.context_menu(|ui| {
        commit_menu(ui, &oid, &subject, context, false, actions);
    });
}

/// Everything you can do with a commit: the row's context menu and the page's "⋯" menu.
fn commit_menu(
    ui: &mut Ui,
    oid: &str,
    subject: &str,
    context: &RowContext<'_>,
    from_page: bool,
    actions: &mut Vec<Action>,
) {
    let oid = oid.to_owned();
    if !from_page {
        menu_item(
            ui,
            "New Branch Here…",
            Action::NewBranch {
                at: Some(oid.clone()),
            },
            actions,
        );
    }
    menu_item(
        ui,
        "New Tag Here…",
        Action::NewTag {
            at: Some(oid.clone()),
        },
        actions,
    );
    menu_item(
        ui,
        "Check Out Commit (Detached)",
        Action::CheckoutCommit(oid.clone()),
        actions,
    );
    if let Some(rewrite) = context.rewrite {
        ui.separator();
        menu_item(
            ui,
            "Cherry-Pick onto Current Branch",
            Action::CherryPick(oid.clone()),
            actions,
        );
        if !from_page {
            menu_item(ui, "Revert Commit", Action::Revert(oid.clone()), actions);
        }
        menu_item(
            ui,
            "Merge into Current Branch",
            Action::MergeIntoCurrent(oid.clone()),
            actions,
        );
        let branch = rewrite.branch.as_deref().unwrap_or("HEAD");
        ui.menu_button(format!("Reset {branch} to Here"), |ui| {
            for (mode, label) in [
                (ResetMode::Soft, "Soft: Keep Changes Staged"),
                (ResetMode::Mixed, "Mixed: Keep Changes Unstaged"),
                (ResetMode::Hard, "Hard: Discard Changes…"),
            ] {
                menu_item(
                    ui,
                    label,
                    Action::ResetTo {
                        oid: oid.clone(),
                        mode,
                    },
                    actions,
                );
            }
        });
        if let Some(branch) = &rewrite.branch {
            menu_item(
                ui,
                &format!("Rebase {branch} onto Here…"),
                Action::RebaseOnto(oid.clone()),
                actions,
            );
            menu_item(
                ui,
                &format!("Interactive Rebase {branch} From Here…"),
                Action::InteractiveRebase {
                    base: Some(oid.clone()),
                },
                actions,
            );
        }
    }
    ui.separator();
    menu_item(
        ui,
        "Copy Commit SHA",
        Action::CopyText {
            text: oid.clone(),
            what: "commit SHA",
        },
        actions,
    );
    menu_item(
        ui,
        "Copy Subject",
        Action::CopyText {
            text: subject.to_owned(),
            what: "subject",
        },
        actions,
    );
    if let Some(kind) = context.forge {
        ui.separator();
        menu_item(
            ui,
            &format!("Open on {}", forge_name(kind)),
            Action::OpenCommitOnForge(oid),
            actions,
        );
    }
}

fn chip_width(ui: &Ui, chip: &Badge) -> f32 {
    let font = match chip.kind {
        BadgeKind::Head | BadgeKind::Local | BadgeKind::Detached => theme::caption_strong(),
        _ => theme::caption(),
    };
    let text = ui
        .painter()
        .layout_no_wrap(chip.label.clone(), font, Color32::WHITE)
        .size()
        .x
        .min(160.0);
    let icon = if chip.kind == BadgeKind::Tag {
        14.0
    } else {
        0.0
    };
    text + icon + 16.0
}

/// A ref chip with its right edge at `right_center`: branches tinted in their lane's color,
/// remote branches outlined, tags outlined with a tag icon.
fn draw_chip(ui: &Ui, right_center: Pos2, chip: &Badge, lane: Color32, palette: &Palette) {
    let width = chip_width(ui, chip);
    let rect = Rect::from_min_max(
        Pos2::new(right_center.x - width, right_center.y - 10.0),
        Pos2::new(right_center.x, right_center.y + 10.0),
    );
    let (font, text, fill, stroke, corner) = match chip.kind {
        BadgeKind::Head | BadgeKind::Local => (
            theme::caption_strong(),
            lane,
            palette.soft(lane),
            None,
            10,
        ),
        BadgeKind::Detached => (
            theme::caption_strong(),
            palette.warning,
            palette.soft(palette.warning),
            None,
            10,
        ),
        BadgeKind::Remote => (
            theme::caption(),
            palette.text_secondary,
            Color32::TRANSPARENT,
            Some(palette.hairline_stroke()),
            10,
        ),
        BadgeKind::Tag => (
            theme::caption(),
            palette.text_secondary,
            Color32::TRANSPARENT,
            Some(palette.hairline_stroke()),
            radius::S,
        ),
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(corner), fill);
    if let Some(stroke) = stroke {
        ui.painter()
            .rect_stroke(rect, CornerRadius::same(corner), stroke, StrokeKind::Inside);
    }
    let mut x = rect.left() + 8.0;
    if chip.kind == BadgeKind::Tag {
        ui.painter().text(
            Pos2::new(x + 5.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon::TAG,
            theme::icon_font(10.0),
            text,
        );
        x += 14.0;
    }
    text_line(
        ui,
        Pos2::new(x, rect.center().y),
        &chip.label,
        font,
        text,
        rect.right() - 8.0 - x,
    );
}

fn detail_pane(
    ui: &mut Ui,
    rect: Rect,
    history: &mut History,
    context: &RowContext<'_>,
    mode: crate::settings::ImageMode,
    actions: &mut Vec<Action>,
) {
    let view = &mut history.commit;
    match &view.body {
        DetailBody::None => {
            centered_message(ui, rect, "No commit selected", "Pick a commit in the list");
            return;
        }
        DetailBody::Loading => {
            let mut y = rect.top() + PAGE_PAD_Y;
            for (width, height) in [(0.6, 20.0), (0.35, 12.0), (0.8, 12.0), (0.7, 12.0)] {
                skeleton(
                    ui,
                    Rect::from_min_size(
                        Pos2::new(rect.left() + PAGE_PAD_X, y),
                        Vec2::new((rect.width() - PAGE_PAD_X * 2.0) * width, height),
                    ),
                );
                y += height + 16.0;
            }
            return;
        }
        DetailBody::Failed(message) => {
            centered_message(ui, rect, "Couldn't read this commit", message);
            return;
        }
        DetailBody::Loaded { .. } => {}
    }
    let DetailBody::Loaded { detail, files } = &view.body else {
        return;
    };

    // A file is open: its diff, with the way back to the commit.
    if let Some(selected) = view.selected_file {
        let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), BAR));
        let back = Rect::from_center_size(
            Pos2::new(bar.left() + 12.0 + 16.0, bar.center().y),
            Vec2::splat(32.0),
        );
        if widgets::icon_button_at(
            ui,
            back,
            Id::unique("commit-file-back"),
            icon::BACK,
            "Back to the commit  (Esc)",
        )
        .clicked()
        {
            actions.push(Action::CloseCommitFile);
        }
        let header = Rect::from_min_max(Pos2::new(back.right() - 8.0, bar.top()), bar.max);
        plain_diff_header(ui, header, &view.diff);
        widgets::hairline_h(ui, rect.x_range().into(), bar.bottom());
        let body = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom() + 1.0), rect.max);
        if selected < files.len() {
            // Commit diffs are read-only: no partial staging, so `area` is not used.
            diff_body(ui, body, &mut view.diff, Area::Staged, mode, actions);
        }
        return;
    }

    let rev = detail.oid.clone();
    ScrollArea::vertical()
        .id_salt(("commit-page", &detail.oid))
        .auto_shrink(false)
        .show(ui, |ui| {
            commit_page(ui, rect, detail, files, &rev, context, actions);
        });
}

/// Allocate a full-width block of `height` and return its rect.
fn block(ui: &mut Ui, height: f32) -> Rect {
    let width = ui.available_width();
    ui.allocate_exact_size(Vec2::new(width, height), Sense::hover())
        .0
}

/// Wrapped text inside the page's margins; returns its height.
fn paragraph(
    ui: &mut Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> f32 {
    let galley = ui
        .painter()
        .layout(text.to_owned(), font, color, max_width.max(40.0));
    let height = galley.size().y;
    let rect = block(ui, height);
    ui.painter()
        .galley(Pos2::new(rect.left() + PAGE_PAD_X, rect.top()), galley, color);
    height
}

fn commit_page(
    ui: &mut Ui,
    pane: Rect,
    detail: &CommitDetail,
    files: &[ChangedFile],
    rev: &str,
    context: &RowContext<'_>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let now = unix_seconds(SystemTime::now());
    ui.spacing_mut().item_spacing.y = 0.0;
    let width = (pane.width() - PAGE_PAD_X * 2.0).max(120.0);
    ui.add_space(PAGE_PAD_Y);

    // Title.
    paragraph(ui, detail.subject(), theme::heading(), palette.text, width);
    ui.add_space(10.0);

    // Author, when, and the commit id.
    let row = block(ui, 24.0);
    let center = row.center().y;
    let mut x = row.left() + PAGE_PAD_X;
    widgets::avatar(
        ui.painter(),
        Pos2::new(x + 12.0, center),
        24.0,
        &detail.author,
        palette,
    );
    x += 24.0 + 10.0;
    let limit = row.left() + PAGE_PAD_X + width;
    x += text_line(
        ui,
        Pos2::new(x, center),
        &detail.author,
        theme::strong(),
        palette.text,
        (limit - x).max(0.0),
    );
    let mut when = format!(" committed {}", relative_time(now, detail.author_time));
    if detail.committer != detail.author || detail.committer_email != detail.author_email {
        when = format!(
            " wrote this {} · {} committed it",
            relative_time(now, detail.author_time),
            detail.committer
        );
    }
    x += text_line(
        ui,
        Pos2::new(x, center),
        &when,
        theme::body(),
        palette.text_secondary,
        (limit - x - 90.0).max(0.0),
    );
    x += 10.0;
    ui.painter().text(
        Pos2::new(x, center),
        Align2::CENTER_CENTER,
        "·",
        theme::body(),
        palette.text_tertiary,
    );
    x += 10.0;
    let short = &detail.oid[..detail.oid.len().min(7)];
    let short_width = ui
        .painter()
        .layout_no_wrap(short.to_owned(), theme::mono_small(), palette.text)
        .size()
        .x;
    let chip = Rect::from_min_size(
        Pos2::new(x, center - 12.0),
        Vec2::new(8.0 + short_width + 6.0 + 12.0 + 8.0, 24.0),
    );
    let response = ui
        .interact(chip, Id::unique("commit-copy-id"), Sense::click())
        .on_hover_text(format!("Copy {}", detail.oid));
    ui.painter().rect(
        chip,
        CornerRadius::same(radius::M),
        interaction_fill(ui, &response, palette),
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(chip.left() + 8.0, center),
        Align2::LEFT_CENTER,
        short,
        theme::mono_small(),
        palette.text_secondary,
    );
    ui.painter().text(
        Pos2::new(chip.right() - 8.0 - 6.0, center),
        Align2::CENTER_CENTER,
        icon::COPY,
        theme::icon_font(11.0),
        palette.text_secondary,
    );
    if response.clicked() {
        actions.push(Action::CopyText {
            text: detail.oid.clone(),
            what: "commit SHA",
        });
    }
    ui.add_space(20.0);

    // The rest of the message.
    let body = detail.body().trim();
    if !body.is_empty() {
        paragraph(
            ui,
            body,
            theme::large(),
            palette.text_secondary,
            width.min(640.0),
        );
        ui.add_space(20.0);
    }

    // Actions.
    let row = block(ui, 32.0);
    let mut x = row.left() + PAGE_PAD_X;
    let mut button = |ui: &mut Ui, glyph: char, label: &str, tooltip: &str| {
        let width = outline_button_width(ui, Some(glyph), label);
        let rect = Rect::from_min_size(Pos2::new(x, row.top()), Vec2::new(width, 32.0));
        x += width + space::S;
        outline_button_at(
            ui,
            rect,
            Id::unique(("commit-action", label, glyph)),
            Some(glyph),
            label,
            tooltip,
        )
    };
    if button(
        ui,
        icon::BRANCH,
        "Branch from here",
        "Start a new branch at this commit",
    )
    .clicked()
    {
        actions.push(Action::NewBranch {
            at: Some(detail.oid.clone()),
        });
    }
    if context.rewrite.is_some()
        && button(
            ui,
            icon::UNDO,
            "Revert",
            "Make a new commit that undoes this one",
        )
        .clicked()
    {
        actions.push(Action::Revert(detail.oid.clone()));
    }
    let more = button(ui, icon::MORE, "", "More commit actions");
    egui::Popup::menu(&more).show(|ui| {
        commit_menu(ui, &detail.oid, detail.subject(), context, true, actions);
    });
    ui.add_space(20.0);

    // The files it changed.
    if files.is_empty() {
        paragraph(
            ui,
            "This commit changes no files (or only merges them).",
            theme::body(),
            palette.text_tertiary,
            width,
        );
    } else {
        files_card(ui, width, files, rev, actions);
    }
    ui.add_space(14.0);

    // Footer: how to open a diff, and the parents.
    let row = block(ui, 18.0);
    let center = row.center().y;
    let mut x = row.left() + PAGE_PAD_X;
    let text = if files.is_empty() {
        String::new()
    } else {
        "Select a file to open its diff".to_owned()
    };
    x += text_line(
        ui,
        Pos2::new(x, center),
        &text,
        theme::small(),
        palette.text_tertiary,
        width,
    );
    if !detail.parents.is_empty() {
        let label = match (files.is_empty(), detail.parents.len()) {
            (true, 1) => "Parent ",
            (true, _) => "Parents ",
            (false, 1) => " · Parent ",
            (false, _) => " · Parents ",
        };
        x += text_line(
            ui,
            Pos2::new(x, center),
            label,
            theme::small(),
            palette.text_tertiary,
            width,
        );
        for parent in &detail.parents {
            let short = &parent[..parent.len().min(7)];
            let galley = ui.painter().layout_no_wrap(
                short.to_owned(),
                theme::mono_small(),
                palette.accent,
            );
            let link = Rect::from_min_size(
                Pos2::new(x, center - galley.size().y / 2.0),
                galley.size(),
            );
            let response = ui
                .interact(link, Id::unique(("commit-parent", parent)), Sense::click())
                .on_hover_text("Show this commit")
                .on_hover_cursor(CursorIcon::PointingHand);
            let color = if response.hovered() {
                palette.text
            } else {
                palette.text_secondary
            };
            ui.painter().galley(link.min, galley, color);
            if response.clicked() {
                actions.push(Action::RevealCommit(parent.clone()));
            }
            x = link.right() + space::S;
        }
    }
    ui.add_space(PAGE_PAD_Y);
}

/// "N files changed +a −r" over a row per file.
fn files_card(
    ui: &mut Ui,
    width: f32,
    files: &[ChangedFile],
    rev: &str,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let height = 40.0 + 1.0 + 8.0 + files.len() as f32 * file_row();
    let block = block(ui, height);
    let card = Rect::from_min_size(
        Pos2::new(block.left() + PAGE_PAD_X, block.top()),
        Vec2::new(width, height),
    );
    let corner = CornerRadius::same(radius::L);
    let header = Rect::from_min_size(card.min, Vec2::new(card.width(), 40.0));
    ui.painter().rect_filled(
        header,
        CornerRadius {
            nw: radius::L,
            ne: radius::L,
            sw: 0,
            se: 0,
        },
        palette.surface_alt,
    );
    ui.painter()
        .hline(card.x_range(), header.bottom(), palette.hairline_stroke());
    ui.painter()
        .rect_stroke(card, corner, palette.hairline_stroke(), StrokeKind::Inside);
    let center = header.center().y;
    let count = if files.len() == 1 {
        "1 file changed".to_owned()
    } else {
        format!("{} files changed", files.len())
    };
    ui.painter().text(
        Pos2::new(header.left() + 14.0, center),
        Align2::LEFT_CENTER,
        count,
        theme::strong(),
        palette.text,
    );
    let added: u32 = files.iter().filter_map(|file| file.added).sum();
    let removed: u32 = files.iter().filter_map(|file| file.removed).sum();
    let mut right = header.right() - 14.0;
    for (count, sign, color) in [
        (removed, '−', palette.danger),
        (added, '+', palette.success),
    ] {
        let painted = ui.painter().text(
            Pos2::new(right, center),
            Align2::RIGHT_CENTER,
            format!("{sign}{count}"),
            theme::mono_small(),
            color,
        );
        right = painted.left() - 6.0;
    }

    let clip = ui.clip_rect();
    let top = header.bottom() + 1.0 + 4.0;
    for (index, file) in files.iter().enumerate() {
        let row = Rect::from_min_size(
            Pos2::new(card.left() + 4.0, top + index as f32 * file_row()),
            Vec2::new(card.width() - 8.0, file_row()),
        );
        if !clip.intersects(row) {
            continue;
        }
        let response = ui.interact(row, Id::unique(("commit-file", index)), Sense::click());
        if response.clicked() {
            actions.push(Action::SelectCommitFile(index));
        }
        ui.painter().rect_filled(
            row,
            CornerRadius::same(radius::M),
            interaction_fill(ui, &response, palette),
        );
        file_row_contents(ui, row, file, false, true);
        file_menu(&response, file, rev, &FileListKind::COMMIT, actions);
    }
}

/// Status badge, name and folder, line counts (and, with `bar`, the change bar) of one file.
fn file_row_contents(ui: &Ui, row: Rect, file: &ChangedFile, untracked: bool, bar: bool) {
    let palette = Palette::of(ui.ctx());
    let center = row.center().y;
    let status = if untracked { 'U' } else { file.status };
    status_badge(
        ui.painter(),
        Pos2::new(row.left() + 10.0 + 9.0, center),
        status,
        palette,
    );
    let mut right = row.right() - 10.0;
    if bar {
        if let (Some(added), Some(removed)) = (file.added, file.removed) {
            right -= change_bar(
                ui.painter(),
                Pos2::new(right, center),
                added as usize,
                removed as usize,
                palette,
            ) + 10.0;
        }
    }
    let counts = match (file.added, file.removed) {
        (Some(0), Some(0)) => String::new(),
        (Some(added), Some(0)) => format!("+{added}"),
        (Some(0), Some(removed)) => format!("−{removed}"),
        (Some(added), Some(removed)) => format!("+{added} −{removed}"),
        _ if file.submodule => String::new(),
        _ => "binary".to_owned(),
    };
    if !counts.is_empty() {
        ui.painter().text(
            Pos2::new(right, center),
            Align2::RIGHT_CENTER,
            counts,
            theme::mono_small(),
            palette.text_tertiary,
        );
        right -= 56.0 + space::S;
    }
    let (dir, name) = split_path(&file.path);
    let x = row.left() + 10.0 + 18.0 + 10.0;
    let room = (right - x).max(0.0);
    let painted = text_line(
        ui,
        Pos2::new(x, center),
        name,
        theme::body(),
        palette.text,
        room,
    );
    let detail = match &file.orig_path {
        Some(orig) => format!("from {orig}"),
        None => dir.to_owned(),
    };
    if !detail.is_empty() && room - painted > 24.0 {
        text_line(
            ui,
            Pos2::new(x + painted + 6.0, center),
            &detail,
            theme::body(),
            palette.text_tertiary,
            room - painted - 6.0,
        );
    }
}

/// The context menu of a read-only file row.
fn file_menu(
    response: &egui::Response,
    file: &ChangedFile,
    rev: &str,
    kind: &FileListKind,
    actions: &mut Vec<Action>,
) {
    response.context_menu(|ui| {
        menu_item(
            ui,
            "File History",
            Action::ShowFileHistory(file.path.clone()),
            actions,
        );
        if file.status != 'D' {
            menu_item(
                ui,
                kind.blame,
                Action::Blame {
                    path: file.path.clone(),
                    rev: Some(rev.to_owned()),
                },
                actions,
            );
        }
        menu_item(
            ui,
            "Open in External Diff Tool",
            Action::OpenDiffTool,
            actions,
        );
        ui.separator();
        menu_item(
            ui,
            "Copy File Path",
            Action::CopyText {
                text: file.path.clone(),
                what: "file path",
            },
            actions,
        );
    });
}

/// One row of a read-only file list.
pub(super) struct FileItem<'a> {
    pub file: &'a ChangedFile,
    /// A stashed untracked file: drawn as `U`, like in the working copy.
    pub untracked: bool,
    /// The commit the file is at (for blame).
    pub rev: &'a str,
}

/// What a read-only file list belongs to: a commit of the history, or a stash.
pub(super) struct FileListKind {
    id: &'static str,
    select: fn(usize) -> Action,
    blame: &'static str,
}

impl FileListKind {
    pub const COMMIT: Self = Self {
        id: "commit-files",
        select: Action::SelectCommitFile,
        blame: "Blame at This Commit",
    };
    pub const STASH: Self = Self {
        id: "stash-files",
        select: Action::SelectStashFile,
        blame: "Blame in This Stash",
    };
}

/// The files of a stash (or a commit) as a scrolling list: status badge, name and folder,
/// line counts.
pub(super) fn file_list<'a>(
    ui: &mut Ui,
    count: usize,
    item: impl Fn(usize) -> FileItem<'a>,
    selected: Option<usize>,
    kind: &FileListKind,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    ScrollArea::vertical()
        .id_salt((kind.id, "scroll"))
        .auto_shrink(false)
        .show_rows(ui, file_row(), count, |ui, range| {
            let left = ui.max_rect().left() + LIST_PAD;
            let width = ui.max_rect().width() - LIST_PAD * 2.0;
            let top = ui.max_rect().top() - range.start as f32 * file_row();
            for index in range {
                let FileItem {
                    file,
                    untracked,
                    rev,
                } = item(index);
                let row = Rect::from_min_size(
                    Pos2::new(left, top + index as f32 * file_row()),
                    Vec2::new(width, file_row()),
                );
                let response = ui.interact(row, Id::unique((kind.id, index)), Sense::click());
                if response.clicked() || response.secondary_clicked() {
                    actions.push((kind.select)(index));
                }
                let fill = if selected == Some(index) {
                    palette.accent_soft
                } else {
                    interaction_fill(ui, &response, palette)
                };
                ui.painter()
                    .rect_filled(row, CornerRadius::same(radius::M), fill);
                file_row_contents(ui, row, file, untracked, false);
                let response = if untracked {
                    response.on_hover_text("New file (was untracked)")
                } else {
                    response
                };
                file_menu(&response, file, rev, kind, actions);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_narrow_before_they_are_clamped() {
        // Few lanes: full width.
        let few = GraphLayout::new(3, 600.0);
        assert_eq!((few.lane, few.lanes), (LANE, 3));
        // 25 lanes in a 520 px list: narrower lanes, all visible.
        let many = GraphLayout::new(25, 520.0);
        assert_eq!(many.lanes, 25);
        assert!(many.lane >= MIN_LANE && many.lane < LANE, "{many:?}");
        // Hundreds of lanes: clamped at the minimum width.
        let huge = GraphLayout::new(400, 520.0);
        assert_eq!(huge.lane, MIN_LANE);
        assert_eq!(huge.lanes, 26);
        // An empty history still lays out one lane, in the design's 44 px column.
        assert_eq!(GraphLayout::new(0, 10.0).lanes, 1);
        assert_eq!(GraphLayout::new(1, 600.0).width(), GRAPH_MIN_WIDTH);
    }
}
