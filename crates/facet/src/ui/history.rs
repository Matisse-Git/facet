//! The history view: the virtualized commit graph on the left and the selected commit's
//! message, files and diff on the right.
//!
//! Every row is drawn from its own `GraphRow` (lanes, segments, node), so a frame only touches
//! the rows on screen — the list is as cheap at 100k commits as at 100.

use std::time::SystemTime;

use eframe::egui::{
    Align2, Color32, CornerRadius, CursorIcon, Id, Label, Pos2, Rect, RichText, ScrollArea, Sense,
    Shape, Stroke, Ui, Vec2,
};
use facet_git::forge::ForgeKind;
use facet_git::graph::{GraphRow, Half};
use facet_git::parse::{ChangedFile, CommitDetail, LogCommit};

use super::diff::{diff_body, plain_diff_header, HEADER_HEIGHT};
use super::widgets::{self, interaction_fill, skeleton, text_line};
use super::{region, splitter_feedback, SPLITTER_GRAB};
use facet_git::ResetMode;

use super::power::{filter_bar, FILTER_BAR};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::format::{relative_time, unix_seconds};
use crate::history::{Badge, BadgeKind, DetailBody, History};
use crate::settings::{Settings, DETAIL_WIDTH};
use crate::theme::{self, radius, space, Palette};
use crate::workspace::Area;

pub const COMMIT_ROW: f32 = 28.0;
const FILE_ROW: f32 = 26.0;
/// Width of one graph lane; lanes narrow towards `MIN_LANE` when many are open.
const LANE: f32 = 14.0;
const MIN_LANE: f32 = 8.0;
/// The graph takes at most this share of the list width. Lanes that still do not fit are
/// drawn on the last visible lane: hundreds of concurrent branches must not push the
/// subjects off screen.
const GRAPH_SHARE: f32 = 0.4;
const NODE_RADIUS: f32 = 4.0;

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
        f32::from(self.lanes) * self.lane + space::S
    }

    fn node_radius(self) -> f32 {
        NODE_RADIUS.min(self.lane * 0.34)
    }
}
const MIN_LIST_WIDTH: f32 = 360.0;

/// Lane colors, picked to stay apart from each other and readable on both themes.
const LANES_DARK: [Color32; 8] = [
    Color32::from_rgb(0x5A, 0xA9, 0xFF),
    Color32::from_rgb(0xF2, 0x9E, 0x4C),
    Color32::from_rgb(0x7B, 0xD8, 0x8F),
    Color32::from_rgb(0xE8, 0x6A, 0xA6),
    Color32::from_rgb(0xB4, 0x8C, 0xF2),
    Color32::from_rgb(0x4F, 0xD1, 0xC5),
    Color32::from_rgb(0xF2, 0xCC, 0x60),
    Color32::from_rgb(0xFF, 0x7A, 0x7A),
];
const LANES_LIGHT: [Color32; 8] = [
    Color32::from_rgb(0x1F, 0x6F, 0xD1),
    Color32::from_rgb(0xC4, 0x6A, 0x12),
    Color32::from_rgb(0x2E, 0x9A, 0x4A),
    Color32::from_rgb(0xC2, 0x39, 0x7A),
    Color32::from_rgb(0x7A, 0x4F, 0xD0),
    Color32::from_rgb(0x12, 0x8A, 0x80),
    Color32::from_rgb(0xA8, 0x83, 0x10),
    Color32::from_rgb(0xD1, 0x3B, 0x3B),
];

fn lane_color(palette: &Palette, color: u16) -> Color32 {
    let colors = if palette.dark {
        &LANES_DARK
    } else {
        &LANES_LIGHT
    };
    colors[usize::from(color) % colors.len()]
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

impl FacetApp {
    /// The history body below the repository header.
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
        let context = self.repo_context();
        let rewrite = context
            .as_ref()
            .filter(|cx| !cx.bare && cx.operation.is_none())
            .map(|cx| Rewrite {
                branch: cx.branch.clone(),
            });
        let tab = &mut self.tabs[index];

        ui.painter().rect_filled(rect, 0.0, palette.surface);
        let mut list_body = list;
        if tab.power.search.open || tab.history.filter.is_some() {
            let bar = Rect::from_min_size(list.min, Vec2::new(list.width(), FILTER_BAR));
            region(ui, "filter-bar", bar, |ui| {
                filter_bar(ui, bar, &mut tab.power.search, &tab.history, actions);
            });
            widgets::hairline_h(ui, list.x_range().into(), bar.bottom());
            list_body = Rect::from_min_max(Pos2::new(list.left(), bar.bottom() + 1.0), list.max);
        }
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
            detail_pane(ui, detail, &mut tab.history, forge, mode, actions);
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
    let mut y = rect.top() + space::S;
    let mut row = 0;
    while y + COMMIT_ROW < rect.bottom() && row < 24 {
        let graph = Rect::from_center_size(
            Pos2::new(rect.left() + space::S + LANE / 2.0, y + COMMIT_ROW / 2.0),
            Vec2::splat(NODE_RADIUS * 2.0),
        );
        skeleton(ui, graph);
        let width = (rect.width() * [0.55, 0.4, 0.62, 0.35, 0.5][row % 5]).max(40.0);
        skeleton(
            ui,
            Rect::from_min_size(
                Pos2::new(rect.left() + space::L + LANE, y + 9.0),
                Vec2::new(width, 10.0),
            ),
        );
        y += COMMIT_ROW;
        row += 1;
    }
}

/// History-rewriting commands are offered on rows (not in a bare repository or while
/// another operation is in progress).
struct Rewrite {
    /// The checked-out branch (`None`: detached HEAD, which cannot be rebased).
    branch: Option<String>,
}

/// What every row's context menu needs.
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
    let layout = GraphLayout::new(history.max_lanes, rect.width());
    let now = unix_seconds(SystemTime::now());
    let mut scroll = ScrollArea::vertical()
        .id_salt("commit-list-scroll")
        .auto_shrink(false);
    if let Some(offset) = history.scroll_to.take() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    let history = &*history;
    ui.spacing_mut().item_spacing.y = 0.0;
    scroll.show_rows(ui, COMMIT_ROW, history.commits.len(), |ui, range| {
        let left = ui.max_rect().left();
        let width = ui.max_rect().width();
        let top = ui.max_rect().top() - range.start as f32 * COMMIT_ROW;
        let row_rect = |row: usize| {
            Rect::from_min_size(
                Pos2::new(left, top + row as f32 * COMMIT_ROW),
                Vec2::new(width, COMMIT_ROW),
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
    let forge = context.forge;
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, Id::unique(("commit-row", row)), Sense::click());
    if response.clicked() || response.secondary_clicked() {
        actions.push(Action::SelectCommit(row));
    }
    let inner = rect.shrink2(Vec2::new(space::XS, 1.0));
    let fill = if selected {
        palette.accent_soft
    } else {
        interaction_fill(ui, &response, palette)
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(inner, CornerRadius::same(radius::M), fill);
    }

    // Graph.
    let painter = ui.painter();
    let origin = rect.left() + space::S + layout.lane / 2.0;
    let node_radius = layout.node_radius();
    let lane_x = |lane: u16| origin + f32::from(lane.min(layout.lanes - 1)) * layout.lane;
    let mid = rect.center().y;
    for segment in &graph.segments {
        let (y0, y1) = match segment.half {
            Half::Top => (rect.top(), mid),
            Half::Bottom => (mid, rect.bottom()),
        };
        let stroke = Stroke::new(1.6, lane_color(palette, segment.color));
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
    if graph.merge {
        painter.add(Shape::circle_filled(node, node_radius, palette.surface));
        painter.add(Shape::circle_stroke(
            node,
            node_radius,
            Stroke::new(1.6, color),
        ));
    } else {
        painter.add(Shape::circle_filled(node, node_radius, color));
    }
    if is_head {
        painter.add(Shape::circle_stroke(
            node,
            node_radius + 2.5,
            Stroke::new(1.2, palette.text),
        ));
    }

    // Right-hand columns: date, author, short id (dropped as the list narrows).
    let center = mid;
    let mut right = inner.right() - space::S;
    let sha_width = 60.0;
    let date_width = 104.0;
    let author_width = 132.0;
    let text_left = rect.left() + layout.width() + space::XS;
    let available = right - text_left;
    if available > 360.0 {
        text_line(
            ui,
            Pos2::new(right - sha_width, center),
            &commit.short_oid(),
            theme::mono(),
            palette.text_tertiary,
            sha_width,
        );
        right -= sha_width + space::S;
    }
    if available > 300.0 {
        text_line(
            ui,
            Pos2::new(right - date_width, center),
            &relative_time(now, commit.time),
            theme::small(),
            palette.text_tertiary,
            date_width,
        );
        right -= date_width + space::S;
    }
    if available > 520.0 {
        text_line(
            ui,
            Pos2::new(right - author_width, center),
            &commit.author,
            theme::small(),
            palette.text_secondary,
            author_width,
        );
        right -= author_width + space::S;
    }

    // Badges (or the reflog step / earlier file name of a filtered list), then the subject
    // in what is left.
    let mut x = text_left;
    let badge_limit = text_left + (right - text_left) * 0.6;
    if let Some(label) = label {
        let pill = Badge {
            label: label.to_owned(),
            kind: BadgeKind::Remote,
        };
        x += badge_pill(ui, Pos2::new(x, center), &pill, badge_limit - x, palette) + space::XS;
    }
    for badge in badges {
        if x > badge_limit - 24.0 {
            break;
        }
        x += badge_pill(ui, Pos2::new(x, center), badge, badge_limit - x, palette) + space::XS;
    }
    if right - x > 16.0 {
        text_line(
            ui,
            Pos2::new(x + space::XS, center),
            &commit.subject,
            theme::body(),
            palette.text,
            right - x - space::XS,
        );
    }

    response.context_menu(|ui| {
        let oid = commit.oid.to_string();
        menu_item(
            ui,
            "New Branch Here…",
            Action::NewBranch {
                at: Some(oid.clone()),
            },
            actions,
        );
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
            menu_item(ui, "Revert Commit", Action::Revert(oid.clone()), actions);
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
                text: commit.subject.to_string(),
                what: "subject",
            },
            actions,
        );
        if let Some(kind) = forge {
            ui.separator();
            menu_item(
                ui,
                &format!("Open on {}", forge_name(kind)),
                Action::OpenCommitOnForge(oid),
                actions,
            );
        }
    });
}

/// A ref name pill; returns its width.
fn badge_pill(ui: &Ui, left_center: Pos2, badge: &Badge, max_width: f32, palette: &Palette) -> f32 {
    let (fill, text, stroke) = match badge.kind {
        BadgeKind::Head => (palette.accent, palette.on_accent, None),
        BadgeKind::Detached => (palette.warning, palette.on_accent, None),
        BadgeKind::Local => (palette.accent_soft, palette.text, None),
        BadgeKind::Remote => (palette.hover, palette.text_secondary, None),
        BadgeKind::Tag => (
            Color32::TRANSPARENT,
            palette.text_secondary,
            Some(Stroke::new(1.0, palette.hairline)),
        ),
    };
    // Tags lead with a small diamond, painted (no UI font has a glyph for it).
    let marker = if badge.kind == BadgeKind::Tag {
        10.0
    } else {
        0.0
    };
    let galley = widgets::elided(
        ui,
        &badge.label,
        theme::small_strong(),
        text,
        (max_width - 12.0 - marker).max(0.0),
    );
    let size = Vec2::new(galley.size().x + 12.0 + marker, 18.0);
    let rect = Rect::from_min_size(Pos2::new(left_center.x, left_center.y - size.y / 2.0), size);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::S), fill);
    if let Some(stroke) = stroke {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius::S),
            stroke,
            eframe::egui::StrokeKind::Inside,
        );
    }
    if marker > 0.0 {
        let center = Pos2::new(rect.left() + 9.0, rect.center().y);
        let r = 3.0;
        ui.painter().add(Shape::convex_polygon(
            vec![
                center + Vec2::new(0.0, -r),
                center + Vec2::new(r, 0.0),
                center + Vec2::new(0.0, r),
                center + Vec2::new(-r, 0.0),
            ],
            text,
            Stroke::NONE,
        ));
    }
    ui.painter().galley(
        Pos2::new(
            rect.left() + 6.0 + marker,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text,
    );
    size.x
}

fn detail_pane(
    ui: &mut Ui,
    rect: Rect,
    history: &mut History,
    forge: Option<ForgeKind>,
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
            let mut y = rect.top() + space::M;
            for width in [0.7, 0.45, 0.3, 0.55] {
                skeleton(
                    ui,
                    Rect::from_min_size(
                        Pos2::new(rect.left() + space::M, y),
                        Vec2::new((rect.width() - space::XL) * width, 12.0),
                    ),
                );
                y += 24.0;
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

    // Message and metadata take up to 35 % of the pane, the file list up to 30 %, the diff
    // the rest.
    let meta_height = (rect.height() * 0.35).clamp(120.0, 360.0);
    let meta = Rect::from_min_size(rect.min, Vec2::new(rect.width(), meta_height));
    let list_height = (files.len().max(1) as f32 * FILE_ROW + space::S)
        .min((rect.height() * 0.3).max(FILE_ROW * 3.0));
    let list = Rect::from_min_size(
        Pos2::new(rect.left(), meta.bottom() + 1.0),
        Vec2::new(rect.width(), list_height),
    );
    let diff_header = Rect::from_min_size(
        Pos2::new(rect.left(), list.bottom() + 1.0),
        Vec2::new(rect.width(), HEADER_HEIGHT),
    );
    let diff = Rect::from_min_max(Pos2::new(rect.left(), diff_header.bottom() + 1.0), rect.max);

    region(ui, "commit-meta", meta, |ui| {
        commit_meta(ui, meta, detail, forge, actions);
    });
    widgets::hairline_h(ui, rect.x_range().into(), meta.bottom());
    let selected_file = view.selected_file;
    region(ui, "commit-files", list, |ui| {
        file_list(
            ui,
            files.len(),
            |index| FileItem {
                file: &files[index],
                untracked: false,
                rev: &detail.oid,
            },
            selected_file,
            &FileListKind::COMMIT,
            actions,
        );
    });
    widgets::hairline_h(ui, rect.x_range().into(), list.bottom());
    if files.is_empty() {
        centered_message(
            ui,
            Rect::from_min_max(diff_header.min, rect.max),
            "No file changes",
            "This commit changes no files (or only merges them)",
        );
        return;
    }
    if diff.height() < 40.0 {
        return;
    }
    plain_diff_header(ui, diff_header, &view.diff);
    widgets::hairline_h(ui, rect.x_range().into(), diff_header.bottom());
    // Commit diffs are read-only: no partial staging, so `area` is not used for actions.
    diff_body(ui, diff, &mut view.diff, Area::Staged, mode, actions);
}

fn commit_meta(
    ui: &mut Ui,
    rect: Rect,
    detail: &CommitDetail,
    forge: Option<ForgeKind>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let now = unix_seconds(SystemTime::now());
    let inner = rect.shrink2(Vec2::new(space::M, space::S));
    ScrollArea::vertical()
        .id_salt("commit-meta-scroll")
        .auto_shrink(false)
        .max_height(inner.height())
        .show(ui, |ui| {
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                ui.add_space(space::M);
                ui.vertical(|ui| {
                    ui.set_max_width(inner.width());
                    ui.spacing_mut().item_spacing.y = space::XS;
                    ui.add(
                        Label::new(
                            RichText::new(detail.subject())
                                .font(theme::strong())
                                .color(palette.text),
                        )
                        .wrap(),
                    );
                    let body = detail.body().trim_end();
                    if !body.is_empty() {
                        ui.add(
                            Label::new(
                                RichText::new(body)
                                    .font(theme::body())
                                    .color(palette.text_secondary),
                            )
                            .wrap(),
                        );
                    }
                    ui.add_space(space::XS);
                    let line = |ui: &mut Ui, text: String| {
                        ui.add(
                            Label::new(
                                RichText::new(text)
                                    .font(theme::small())
                                    .color(palette.text_tertiary),
                            )
                            .wrap(),
                        );
                    };
                    line(
                        ui,
                        format!(
                            "{} <{}> · {}",
                            detail.author,
                            detail.author_email,
                            relative_time(now, detail.author_time)
                        ),
                    );
                    if detail.committer != detail.author
                        || detail.committer_email != detail.author_email
                    {
                        line(
                            ui,
                            format!(
                                "Committed by {} · {}",
                                detail.committer,
                                relative_time(now, detail.commit_time)
                            ),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        ui.label(
                            RichText::new(&detail.oid)
                                .font(theme::mono())
                                .color(palette.text_tertiary),
                        );
                        if ui.small_button("Copy").clicked() {
                            actions.push(Action::CopyText {
                                text: detail.oid.clone(),
                                what: "commit SHA",
                            });
                        }
                        if let Some(kind) = forge {
                            if ui
                                .small_button(format!("Open on {}", forge_name(kind)))
                                .clicked()
                            {
                                actions.push(Action::OpenCommitOnForge(detail.oid.clone()));
                            }
                        }
                    });
                    if !detail.parents.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            ui.label(
                                RichText::new(if detail.parents.len() > 1 {
                                    "Parents"
                                } else {
                                    "Parent"
                                })
                                .font(theme::small())
                                .color(palette.text_tertiary),
                            );
                            for parent in &detail.parents {
                                let short = &parent[..parent.len().min(7)];
                                if ui
                                    .link(RichText::new(short).font(theme::mono()))
                                    .on_hover_text("Show this commit")
                                    .clicked()
                                {
                                    actions.push(Action::RevealCommit(parent.clone()));
                                }
                            }
                        });
                    }
                });
            });
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

/// The files of a commit or a stash: status letter, path, line counts.
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
        .show_rows(ui, FILE_ROW, count, |ui, range| {
            let left = ui.max_rect().left();
            let width = ui.max_rect().width();
            let top = ui.max_rect().top() - range.start as f32 * FILE_ROW;
            for index in range {
                let FileItem {
                    file,
                    untracked,
                    rev,
                } = item(index);
                let row = Rect::from_min_size(
                    Pos2::new(left, top + index as f32 * FILE_ROW),
                    Vec2::new(width, FILE_ROW),
                );
                let response = ui.interact(row, Id::unique((kind.id, index)), Sense::click());
                if response.clicked() {
                    actions.push((kind.select)(index));
                }
                let inner = row.shrink2(Vec2::new(space::XS, 1.0));
                let fill = if selected == Some(index) {
                    palette.accent_soft
                } else {
                    interaction_fill(ui, &response, palette)
                };
                ui.painter()
                    .rect_filled(inner, CornerRadius::same(radius::M), fill);
                let center = row.center().y;
                let (letter, color) = match file.status {
                    _ if untracked => ('U', palette.success),
                    'A' => ('A', palette.success),
                    'D' => ('D', palette.danger),
                    'R' => ('R', palette.accent),
                    'C' => ('C', palette.accent),
                    other => (other, palette.warning),
                };
                ui.painter().text(
                    Pos2::new(inner.left() + space::S + 6.0, center),
                    Align2::CENTER_CENTER,
                    letter,
                    theme::small_strong(),
                    color,
                );
                let mut right = inner.right() - space::S;
                let counts = match (file.added, file.removed) {
                    (Some(added), Some(removed)) => Some((added, removed)),
                    _ => None,
                };
                if let Some((added, removed)) = counts {
                    for (count, sign, color) in [
                        (removed, '−', palette.danger),
                        (added, '+', palette.success),
                    ] {
                        if count > 0 {
                            let painted = ui.painter().text(
                                Pos2::new(right, center),
                                Align2::RIGHT_CENTER,
                                format!("{sign}{count}"),
                                theme::small(),
                                color,
                            );
                            right = painted.left() - space::XS;
                        }
                    }
                } else if !file.submodule {
                    let painted = ui.painter().text(
                        Pos2::new(right, center),
                        Align2::RIGHT_CENTER,
                        "binary",
                        theme::small(),
                        palette.text_tertiary,
                    );
                    right = painted.left() - space::XS;
                }
                let label = match &file.orig_path {
                    Some(orig) => format!("{orig} → {}", file.path),
                    None => file.path.clone(),
                };
                let x = inner.left() + space::S + 12.0 + space::S;
                text_line(
                    ui,
                    Pos2::new(x, center),
                    &label,
                    theme::body(),
                    palette.text,
                    (right - x - space::XS).max(0.0),
                );
                let response = if untracked {
                    response.on_hover_text("New file (was untracked)")
                } else {
                    response
                };
                if response.secondary_clicked() {
                    actions.push((kind.select)(index));
                }
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
        assert!(many.width() <= 520.0 * GRAPH_SHARE + space::S + 0.01);
        // Hundreds of lanes: clamped at the minimum width.
        let huge = GraphLayout::new(400, 520.0);
        assert_eq!(huge.lane, MIN_LANE);
        assert_eq!(huge.lanes, 26);
        // An empty history still lays out one lane.
        assert_eq!(GraphLayout::new(0, 10.0).lanes, 1);
    }
}
