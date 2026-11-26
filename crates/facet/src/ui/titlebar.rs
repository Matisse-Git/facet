//! The custom title bar: Facet's mark, the repository switcher, the branch picker, the
//! "Search or run a command" box (the command palette), the Sync button, and empty space that
//! drags the window. The minimize/maximize/close buttons on the right are drawn by Windows
//! (see `platform::Chrome`); this view only leaves room for them.

use eframe::egui::{
    self, Align2, CornerRadius, Id, PopupCloseBehavior, Pos2, Rect, RectAlign, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, Vec2,
};
use facet_git::parse::{GitRef, RefKind};
use facet_git::Head;
use std::time::SystemTime;

use super::sidebar;
use super::widgets::{self, icon_button_at, interaction_fill, list_row, text_line};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, RepoTab, TabState};
use crate::app_repo::sync_step;
use crate::app_work::ready_info;
use crate::format::{relative_time, unix_seconds};
use crate::platform::{Chrome, PxRect};
use crate::settings::same_path;
use crate::theme::{self, icon, radius, space, Palette};

const CONTROL_HEIGHT: f32 = 32.0;
const SEARCH_WIDTH: f32 = 400.0;
const SWITCHER_WIDTH: f32 = 440.0;
const BRANCH_PICKER_WIDTH: f32 = 380.0;
const REPO_ROW: f32 = 52.0;
/// A strip of title bar that is always left empty so the window can be grabbed.
const DRAG_STRIP: f32 = space::XL;

/// The repository switcher's button id (the sidebar and the welcome view open it too).
pub(super) fn switcher_id() -> Id {
    Id::unique("title-repo-switcher")
}

fn branch_picker_id() -> Id {
    Id::unique("title-branch-picker")
}

/// Paint a ghost button (no fill until hovered) at `rect` and return its response.
fn ghost(ui: &Ui, rect: Rect, id: Id, open: bool) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    let fill = if open {
        palette.pressed
    } else {
        interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);
    widgets::focus_ring(ui, &response, rect, radius::M, palette);
    response
}

/// The chevron after a button's label.
fn chevron(ui: &Ui, center: Pos2, color: egui::Color32) {
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        icon::CHEVRON_DOWN,
        theme::icon_font(9.0),
        color,
    );
}

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

        // Facet's mark.
        let mark = Rect::from_center_size(
            Pos2::new(rect.left() + 14.0 + 10.0, center_y),
            Vec2::splat(20.0),
        );
        widgets::app_mark(ui.painter(), mark, palette);
        let mut x = mark.right() + space::S;

        // Right side, next to the caption buttons: Sync, then "open in" (or Settings while no
        // repository is open, as the sidebar with its Settings item is hidden then).
        let mut right = rect.right() - caption_width - space::S;
        if let Some(sync) = self.sync_button(ui, right, center_y) {
            controls.push(sync);
            right = sync.left() - space::XS;
        }
        if self.tabs.is_empty() {
            let gear = Rect::from_min_size(
                Pos2::new(right - CONTROL_HEIGHT, center_y - CONTROL_HEIGHT / 2.0),
                Vec2::splat(CONTROL_HEIGHT),
            );
            let tooltip = widgets::tooltip_with_shortcut(ui, "Settings", &shortcut::SETTINGS);
            if icon_button_at(
                ui,
                gear,
                Id::unique("title-settings"),
                icon::SETTINGS,
                &tooltip,
            )
            .clicked()
            {
                self.pending.push(Action::ShowSettings);
            }
            controls.push(gear);
            right = gear.left() - space::XS;
        }

        // Left side: repository › branch.
        if self.tabs.is_empty() {
            text_line(
                ui,
                Pos2::new(x + space::XS, center_y),
                "Facet",
                theme::small(),
                palette.text_secondary,
                200.0,
            );
        } else {
            let repo = self.repo_button(ui, x, center_y, right - x);
            controls.push(repo);
            x = repo.right() + space::XS;
            if let Some(branch) = self.branch_button(ui, x, center_y, right - x - DRAG_STRIP) {
                controls.push(branch);
                x = branch.right();
            }
        }

        // Search or run a command: centered on the window while it fits between the two sides.
        let room = right - x - DRAG_STRIP * 2.0;
        if room >= 200.0 {
            let width = SEARCH_WIDTH.min(room);
            let center_x = rect.center().x.clamp(
                x + DRAG_STRIP + width / 2.0,
                right - DRAG_STRIP - width / 2.0,
            );
            let search = Rect::from_center_size(
                Pos2::new(center_x, center_y),
                Vec2::new(width, CONTROL_HEIGHT),
            );
            self.search_box(ui, search);
            controls.push(search);
        }

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

    /// The box that opens the command palette.
    fn search_box(&mut self, ui: &Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let response = ui.interact(rect, Id::unique("title-search"), Sense::click());
        let hover = ui.ctx().animate_bool_with_time_and_easing(
            response.id.with("hover"),
            response.hovered(),
            theme::motion::FAST,
            theme::motion::ease_out,
        );
        ui.painter().rect(
            rect,
            CornerRadius::same(radius::LAYER),
            palette.surface,
            Stroke::new(
                1.0,
                widgets::mix(palette.hairline, palette.text_tertiary, hover * 0.5),
            ),
            StrokeKind::Inside,
        );
        ui.painter().text(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon::SEARCH,
            theme::icon_font(13.0),
            palette.text_tertiary,
        );
        let key = widgets::shortcut_text(ui, &shortcut::PALETTE).replace('+', " ");
        let cap = widgets::key_cap(ui, Pos2::new(rect.right() - 8.0, rect.center().y), &key);
        text_line(
            ui,
            Pos2::new(rect.left() + 34.0, rect.center().y),
            "Search or run a command",
            theme::body(),
            palette.text_tertiary,
            rect.width() - 34.0 - cap - 16.0,
        );
        widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);
        if response.clicked() {
            self.pending.push(Action::ShowPalette);
        }
    }

    /// "facet ⌄": opens the repository switcher. Returns the button's rect.
    fn repo_button(&mut self, ui: &mut Ui, x: f32, center_y: f32, max_width: f32) -> Rect {
        let palette = Palette::of(ui.ctx());
        let name = self
            .active_tab()
            .map_or_else(|| "Choose a repository".to_owned(), RepoTab::name);
        let galley = widgets::elided(
            ui,
            &name,
            theme::strong(),
            palette.text,
            (max_width * 0.4).clamp(60.0, 260.0),
        );
        let rect = Rect::from_min_size(
            Pos2::new(x, center_y - CONTROL_HEIGHT / 2.0),
            Vec2::new(galley.size().x + 8.0 + 6.0 + 12.0 + 6.0, CONTROL_HEIGHT),
        );
        let open = egui::Popup::is_id_open(ui.ctx(), switcher_id());
        let response = ghost(ui, rect, switcher_id(), open);
        ui.painter().galley(
            Pos2::new(rect.left() + 8.0, center_y - galley.size().y / 2.0),
            galley,
            palette.text,
        );
        chevron(
            ui,
            Pos2::new(rect.right() - 12.0, center_y + 1.0),
            palette.text_secondary,
        );
        let response = if open {
            response
        } else {
            response.on_hover_text("Switch repository")
        };
        if response.clicked() {
            ui.ctx()
                .data_mut(|data| data.insert_temp(switcher_id().with("focus"), true));
        }
        if self.active_tab().is_some_and(RepoTab::is_ready) {
            self.open_in_menu(&response);
        }
        self.repo_switcher(&response);
        rect
    }

    /// The flyout under the repository button: find, switch, reopen, add.
    fn repo_switcher(&mut self, button: &egui::Response) {
        let mut chosen = None;
        let open_paths: Vec<_> = self.tabs.iter().map(|tab| tab.path.clone()).collect();
        let recent: Vec<_> = self
            .settings
            .recent_repos
            .iter()
            .filter(|path| !open_paths.iter().any(|open| same_path(open, path)))
            .cloned()
            .collect();
        let active = self.active;
        let tabs: Vec<(String, String, String, bool)> = self
            .tabs
            .iter()
            .map(|tab| {
                let detail = match &tab.state {
                    TabState::Loading => "Loading…".to_owned(),
                    TabState::Ready { head, .. } => {
                        let branch = match head {
                            Head::Branch { name, .. } => name.clone(),
                            Head::Detached { commit } => format!("at {}", commit.short_oid),
                        };
                        let changed = tab.work.staged.len() + tab.work.unstaged.len();
                        if ready_info(tab).is_some() && changed > 0 {
                            format!("{branch} · {changed} changed")
                        } else {
                            branch
                        }
                    }
                };
                (
                    tab.name(),
                    tab.path.display().to_string(),
                    detail,
                    tab.is_ready(),
                )
            })
            .collect();

        egui::Popup::menu(button)
            .id(egui::Popup::default_response_id(button))
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .align(RectAlign::BOTTOM_START)
            .gap(space::XS)
            .width(SWITCHER_WIDTH)
            .show(|ui| {
                let palette = Palette::of(ui.ctx());
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(SWITCHER_WIDTH);
                let query_id = switcher_id().with("query");
                let mut query: String = ui
                    .ctx()
                    .data_mut(|data| data.get_temp::<String>(query_id).unwrap_or_default());
                let (field, _) = ui
                    .allocate_exact_size(Vec2::new(SWITCHER_WIDTH, CONTROL_HEIGHT), Sense::hover());
                let field = field.shrink2(Vec2::new(space::XS, 0.0));
                let response = widgets::text_field(
                    ui,
                    field,
                    query_id.with("field"),
                    &mut query,
                    "Find a repository",
                );
                let focus_key = switcher_id().with("focus");
                if ui
                    .ctx()
                    .data_mut(|data| data.remove_temp::<bool>(focus_key).unwrap_or(false))
                {
                    response.request_focus();
                }
                ui.ctx()
                    .data_mut(|data| data.insert_temp(query_id, query.clone()));
                let needle = query.trim().to_lowercase();
                let matches = |name: &str, path: &str| {
                    needle.is_empty()
                        || name.to_lowercase().contains(&needle)
                        || path.to_lowercase().contains(&needle)
                };
                ui.add_space(space::XS);

                let row_width = SWITCHER_WIDTH;
                let mut first_match: Option<Action> = None;
                ScrollArea::vertical()
                    .id_salt("switcher-scroll")
                    .max_height(REPO_ROW * 7.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, (name, path, detail, ready)) in tabs.iter().enumerate() {
                            if !matches(name, path) {
                                continue;
                            }
                            let current = active == Some(index);
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(row_width, REPO_ROW),
                                Sense::hover(),
                            );
                            let row = rect.shrink2(Vec2::new(space::XS, 2.0));
                            let response =
                                list_row(ui, row, Id::unique(("switch-tab", index)), current);
                            repo_row_text(ui, row, name, path, detail, *ready, palette);
                            let action = Action::SwitchTab(index);
                            first_match.get_or_insert_with(|| action.clone());
                            if response.clicked() {
                                chosen = Some(action);
                            }
                            if response.middle_clicked() {
                                chosen = Some(Action::CloseTab(index));
                            }
                            response.context_menu(|ui| {
                                if ui.button("Close").clicked() {
                                    chosen = Some(Action::CloseTab(index));
                                }
                            });
                        }
                        let shown: Vec<_> = recent
                            .iter()
                            .filter(|path| {
                                let name = repo_name(path);
                                matches(&name, &path.display().to_string())
                            })
                            .collect();
                        if !shown.is_empty() {
                            ui.add_space(space::XS);
                            let (label, _) =
                                ui.allocate_exact_size(Vec2::new(row_width, 24.0), Sense::hover());
                            ui.painter().text(
                                Pos2::new(label.left() + space::M, label.center().y),
                                Align2::LEFT_CENTER,
                                "Recent",
                                theme::small_strong(),
                                palette.text_tertiary,
                            );
                        }
                        for path in shown {
                            let name = repo_name(path);
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(row_width, REPO_ROW),
                                Sense::hover(),
                            );
                            let row = rect.shrink2(Vec2::new(space::XS, 2.0));
                            let response =
                                list_row(ui, row, Id::unique(("switch-recent", path)), false);
                            repo_row_text(
                                ui,
                                row,
                                &name,
                                &path.display().to_string(),
                                "",
                                true,
                                palette,
                            );
                            let action = Action::OpenPath(path.clone());
                            first_match.get_or_insert_with(|| action.clone());
                            if response.clicked() {
                                chosen = Some(action);
                            }
                            response.context_menu(|ui| {
                                if ui.button("Remove from Recent").clicked() {
                                    chosen = Some(Action::ForgetRecent(path.clone()));
                                }
                            });
                        }
                    });
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    chosen = chosen.take().or(first_match);
                }

                // Footer: add a repository.
                ui.add_space(space::XS);
                let (line, _) = ui.allocate_exact_size(Vec2::new(row_width, 1.0), Sense::hover());
                widgets::hairline_h(ui, line.x_range().into(), line.center().y);
                ui.add_space(space::XS);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(row_width, 40.0), Sense::hover());
                let row = rect.shrink2(Vec2::new(space::XS, 2.0));
                let response = list_row(ui, row, Id::unique("switch-add"), false);
                ui.painter().text(
                    Pos2::new(row.left() + 20.0, row.center().y),
                    Align2::CENTER_CENTER,
                    icon::ADD,
                    theme::icon_font(13.0),
                    palette.accent,
                );
                ui.painter().text(
                    Pos2::new(row.left() + 40.0, row.center().y),
                    Align2::LEFT_CENTER,
                    "Add repository",
                    theme::strong(),
                    palette.text,
                );
                let key = widgets::shortcut_text(ui, &shortcut::OPEN);
                widgets::key_cap(ui, Pos2::new(row.right() - space::S, row.center().y), &key);
                if response.clicked() {
                    chosen = Some(Action::ShowAddRepository(None));
                }
                if chosen.is_some() {
                    ui.ctx().data_mut(|data| data.remove::<String>(query_id));
                    ui.close();
                }
            });
        self.pending.extend(chosen);
    }

    /// "⑂ main ⌄": opens the branch picker. Only for a ready working copy.
    fn branch_button(
        &mut self,
        ui: &mut Ui,
        x: f32,
        center_y: f32,
        max_width: f32,
    ) -> Option<Rect> {
        let palette = Palette::of(ui.ctx());
        let tab = self.active_tab()?;
        let TabState::Ready { head, .. } = &tab.state else {
            return None;
        };
        if max_width < 80.0 {
            return None;
        }
        let (label, color) = match head {
            Head::Branch { name, .. } => (name.clone(), palette.text),
            Head::Detached { commit } => {
                (format!("Detached at {}", commit.short_oid), palette.warning)
            }
        };
        // The separator between repository and branch.
        ui.painter().text(
            Pos2::new(x + 4.0, center_y),
            Align2::CENTER_CENTER,
            "/",
            theme::body(),
            palette.text_tertiary,
        );
        let x = x + 12.0;
        let galley = widgets::elided(
            ui,
            &label,
            theme::body(),
            color,
            (max_width - 60.0).clamp(40.0, 260.0),
        );
        let rect = Rect::from_min_size(
            Pos2::new(x, center_y - CONTROL_HEIGHT / 2.0),
            Vec2::new(
                8.0 + 16.0 + 6.0 + galley.size().x + 6.0 + 12.0 + 6.0,
                CONTROL_HEIGHT,
            ),
        );
        let open = egui::Popup::is_id_open(ui.ctx(), branch_picker_id().with("popup"));
        let response = ghost(ui, rect, branch_picker_id(), open);
        ui.painter().text(
            Pos2::new(rect.left() + 16.0, center_y),
            Align2::CENTER_CENTER,
            icon::BRANCH,
            theme::icon_font(13.0),
            palette.text_secondary,
        );
        ui.painter().galley(
            Pos2::new(rect.left() + 30.0, center_y - galley.size().y / 2.0),
            galley,
            color,
        );
        chevron(
            ui,
            Pos2::new(rect.right() - 12.0, center_y + 1.0),
            palette.text_secondary,
        );
        let response = if open {
            response
        } else {
            response.on_hover_text("Switch branch")
        };
        if std::mem::take(&mut self.open_branches) && !open {
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
            ui.ctx()
                .data_mut(|data| data.insert_temp(branch_picker_id().with("focus"), true));
        }
        if response.clicked() {
            ui.ctx()
                .data_mut(|data| data.insert_temp(branch_picker_id().with("focus"), true));
        }
        self.branch_picker(&response);
        Some(rect)
    }

    /// The flyout under the branch button: "Find or create a branch", the current branch, the
    /// other branches by when they last moved, the remote branches, and New branch / Merge
    /// into this.
    #[allow(clippy::too_many_lines)]
    fn branch_picker(&mut self, button: &egui::Response) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let forge = self.forge().map(|forge| forge.kind);
        let snapshot = &tab.refs.snapshot;
        let mut locals: Vec<&GitRef> = snapshot.local().collect();
        locals.sort_by_key(|branch| std::cmp::Reverse(branch.date));
        let current = locals
            .iter()
            .position(|branch| branch.head)
            .map(|index| locals.remove(index));
        let remotes: Vec<&GitRef> = snapshot
            .remotes
            .iter()
            .flat_map(|remote| snapshot.remote_branches(&remote.name))
            .filter(|branch| branch.short_name() != "HEAD")
            .collect();
        let remote_label = match snapshot.remotes.as_slice() {
            [only] => format!("{} · {}", only.name, remotes.len()),
            _ => remotes.len().to_string(),
        };
        let default = ["main", "master", "trunk"]
            .into_iter()
            .find(|name| snapshot.local().any(|branch| branch.name == *name));
        let info = &tab.work.branch;
        let status = match (&info.upstream, info.ahead.unwrap_or(0), info.behind.unwrap_or(0)) {
            (None, ..) => "Not published yet".to_owned(),
            (Some(upstream), ..) if info.upstream_gone() => {
                format!("{upstream} isn’t on the remote yet")
            }
            (Some(upstream), 0, 0) => format!("Up to date with {upstream}"),
            (Some(upstream), ahead, 0) => format!("{ahead} ahead of {}", remote_of(upstream)),
            (Some(upstream), 0, behind) => format!("{behind} behind {}", remote_of(upstream)),
            (Some(upstream), ahead, behind) => {
                format!("{ahead} ahead, {behind} behind {}", remote_of(upstream))
            }
        };
        let now = unix_seconds(SystemTime::now());
        let mut chosen = None;
        let mut close = false;
        let frame = egui::Frame::popup(&button.ctx.style_of(button.ctx.theme()))
            .inner_margin(0)
            .corner_radius(radius::L);
        egui::Popup::menu(button)
            .id(egui::Popup::default_response_id(button))
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .align(RectAlign::BOTTOM_START)
            .gap(space::XS)
            .width(BRANCH_PICKER_WIDTH)
            .frame(frame)
            .show(|ui| {
                let palette = Palette::of(ui.ctx());
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(BRANCH_PICKER_WIDTH);
                let block = |ui: &mut Ui, height: f32| {
                    ui.allocate_exact_size(Vec2::new(BRANCH_PICKER_WIDTH, height), Sense::hover())
                        .0
                };

                // Find or create.
                let query_id = branch_picker_id().with("query");
                let mut query: String = ui
                    .ctx()
                    .data_mut(|data| data.get_temp::<String>(query_id).unwrap_or_default());
                let field = block(ui, 56.0).shrink(10.0);
                let response = widgets::search_field(
                    ui,
                    field,
                    query_id.with("field"),
                    &mut query,
                    "Find or create a branch",
                    space::S,
                );
                if ui.ctx().data_mut(|data| {
                    data.remove_temp::<bool>(branch_picker_id().with("focus"))
                        .unwrap_or(false)
                }) {
                    response.request_focus();
                }
                ui.ctx()
                    .data_mut(|data| data.insert_temp(query_id, query.clone()));
                let needle = query.trim().to_lowercase();
                let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
                let new_name: String = query
                    .trim()
                    .chars()
                    .map(|c| if c.is_whitespace() { '-' } else { c })
                    .collect();
                let exists = snapshot.local().any(|branch| branch.name == new_name);
                let remotes_id = branch_picker_id().with("remotes");
                let remotes_open = !needle.is_empty()
                    || ui
                        .ctx()
                        .data(|data| data.get_temp::<bool>(remotes_id).unwrap_or(false));
                let mut first_match = None;

                let label = |ui: &mut Ui, text: &str, top: f32| {
                    let rect = block(ui, top + 20.0);
                    ui.painter().text(
                        Pos2::new(rect.left() + 16.0, rect.bottom() - 10.0),
                        Align2::LEFT_CENTER,
                        text,
                        theme::small_strong(),
                        palette.text_tertiary,
                    );
                };
                ScrollArea::vertical()
                    .id_salt("branch-picker-scroll")
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut any = false;

                        // "Create branch x" while what is typed is not a branch yet.
                        if !new_name.is_empty() && !exists {
                            any = true;
                            let row = block(ui, 44.0).shrink2(Vec2::new(6.0, 2.0));
                            let response =
                                list_row(ui, row, Id::unique("pick-branch-create"), false);
                            ui.painter().text(
                                Pos2::new(row.left() + 18.0, row.center().y),
                                Align2::CENTER_CENTER,
                                icon::ADD,
                                theme::icon_font(13.0),
                                palette.accent,
                            );
                            text_line(
                                ui,
                                Pos2::new(row.left() + 36.0, row.center().y),
                                &format!("Create branch “{new_name}”"),
                                theme::body(),
                                palette.text,
                                row.width() - 46.0,
                            );
                            let create = Action::CreateBranch {
                                name: new_name.clone(),
                                at: "HEAD".to_owned(),
                                switch: true,
                            };
                            if response.clicked() {
                                chosen = Some(create.clone());
                            }
                            first_match.get_or_insert(create);
                        }

                        if let Some(branch) = current.filter(|branch| matches(&branch.name)) {
                            any = true;
                            label(ui, "Current", 4.0);
                            let row = block(ui, 46.0).shrink2(Vec2::new(6.0, 1.0));
                            let response =
                                list_row(ui, row, Id::unique("pick-branch-current"), true);
                            ui.painter().text(
                                Pos2::new(row.left() + 18.0, row.center().y),
                                Align2::CENTER_CENTER,
                                icon::CHECK,
                                theme::icon_font(12.0),
                                palette.accent,
                            );
                            two_lines(
                                ui,
                                row,
                                &branch.name,
                                theme::strong(),
                                palette.text,
                                &status,
                                "",
                            );
                            if response.clicked() {
                                close = true;
                            }
                            let mut menu = Vec::new();
                            response.context_menu(|ui| {
                                sidebar::branch_menu(ui, branch, forge, &mut menu);
                            });
                            if let Some(action) = menu.pop() {
                                chosen = Some(action);
                            }
                        }

                        let others: Vec<&&GitRef> =
                            locals.iter().filter(|branch| matches(&branch.name)).collect();
                        if !others.is_empty() {
                            any = true;
                            label(ui, if needle.is_empty() { "Recent" } else { "Branches" }, 10.0);
                        }
                        for branch in others {
                            let row = block(ui, 46.0).shrink2(Vec2::new(6.0, 1.0));
                            let response =
                                list_row(ui, row, Id::unique(("pick-branch", &branch.name)), false);
                            let when = relative_time(now, branch.date);
                            let detail = if branch.author.is_empty() {
                                when
                            } else {
                                format!("{} · {when}", branch.author)
                            };
                            let tag = if Some(branch.name.as_str()) == default {
                                "default"
                            } else {
                                ""
                            };
                            two_lines(
                                ui,
                                row,
                                &branch.name,
                                theme::body(),
                                palette.text,
                                &detail,
                                tag,
                            );
                            let switch = Action::SwitchBranch(branch.name.clone());
                            if response.clicked() {
                                chosen = Some(switch.clone());
                            }
                            first_match.get_or_insert(switch);
                            let mut menu = Vec::new();
                            response.context_menu(|ui| {
                                sidebar::branch_menu(ui, branch, forge, &mut menu);
                            });
                            if let Some(action) = menu.pop() {
                                chosen = Some(action);
                            }
                        }

                        // Remote branches: one row that opens the list (always open while
                        // finding).
                        let shown: Vec<&&GitRef> = remotes
                            .iter()
                            .filter(|branch| matches(&branch.name))
                            .collect();
                        if !remotes.is_empty() && (needle.is_empty() || !shown.is_empty()) {
                            any = true;
                            ui.add_space(6.0);
                            let row = block(ui, 40.0).shrink2(Vec2::new(6.0, 0.0));
                            let response =
                                list_row(ui, row, Id::unique("pick-branch-remotes"), false);
                            ui.painter().text(
                                Pos2::new(row.left() + 18.0, row.center().y),
                                Align2::CENTER_CENTER,
                                icon::CLOUD,
                                theme::icon_font(14.0),
                                palette.text_secondary,
                            );
                            ui.painter().text(
                                Pos2::new(row.left() + 36.0, row.center().y),
                                Align2::LEFT_CENTER,
                                "Remote branches",
                                theme::body(),
                                palette.text_secondary,
                            );
                            ui.painter().text(
                                Pos2::new(row.right() - 10.0, row.center().y),
                                Align2::RIGHT_CENTER,
                                &remote_label,
                                theme::small(),
                                palette.text_tertiary,
                            );
                            if response.clicked() && needle.is_empty() {
                                ui.ctx()
                                    .data_mut(|data| data.insert_temp(remotes_id, !remotes_open));
                            }
                            if remotes_open {
                                for branch in shown {
                                    let row = block(ui, 36.0).shrink2(Vec2::new(6.0, 1.0));
                                    let response = list_row(
                                        ui,
                                        row,
                                        Id::unique(("pick-remote-branch", &branch.name)),
                                        false,
                                    );
                                    let remote = match &branch.kind {
                                        RefKind::Remote { remote } => remote.as_str(),
                                        _ => "",
                                    };
                                    let right = ui
                                        .painter()
                                        .text(
                                            Pos2::new(row.right() - 10.0, row.center().y),
                                            Align2::RIGHT_CENTER,
                                            remote,
                                            theme::small(),
                                            palette.text_tertiary,
                                        )
                                        .left();
                                    text_line(
                                        ui,
                                        Pos2::new(row.left() + 36.0, row.center().y),
                                        branch.short_name(),
                                        theme::body(),
                                        palette.text,
                                        right - row.left() - 36.0 - space::S,
                                    );
                                    let checkout = Action::CheckoutRemoteBranch(branch.name.clone());
                                    if response.clicked() {
                                        chosen = Some(checkout.clone());
                                    }
                                    first_match.get_or_insert(checkout);
                                    let response = response.on_hover_text(format!(
                                        "Check out {} as a local branch",
                                        branch.name
                                    ));
                                    let mut menu = Vec::new();
                                    response.context_menu(|ui| {
                                        sidebar::remote_branch_menu(ui, branch, forge, &mut menu);
                                    });
                                    if let Some(action) = menu.pop() {
                                        chosen = Some(action);
                                    }
                                }
                            }
                        }
                        if !any {
                            let rect = block(ui, 44.0);
                            ui.painter().text(
                                Pos2::new(rect.left() + 16.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                "No branch matches",
                                theme::body(),
                                palette.text_tertiary,
                            );
                        }
                        ui.add_space(6.0);
                    });
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    chosen = chosen.take().or(first_match);
                }

                // New branch / Merge into this.
                let footer = block(ui, 55.0);
                ui.painter()
                    .rect_filled(footer, CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: radius::L,
                        se: radius::L,
                    }, palette.surface_alt);
                ui.painter()
                    .hline(footer.x_range(), footer.top() + 0.5, palette.hairline_stroke());
                let inner = Rect::from_min_max(
                    Pos2::new(footer.left() + 10.0, footer.top() + 11.0),
                    Pos2::new(footer.right() - 10.0, footer.bottom() - 10.0),
                );
                let half = (inner.width() - space::S) / 2.0;
                let new = Rect::from_min_size(inner.min, Vec2::new(half, inner.height()));
                let merge = Rect::from_min_size(
                    Pos2::new(new.right() + space::S, inner.top()),
                    Vec2::new(half, inner.height()),
                );
                let key = widgets::shortcut_text(ui, &shortcut::NEW_BRANCH);
                if widgets::primary_button_at(
                    ui,
                    new,
                    Id::unique("pick-branch-new"),
                    Some(icon::ADD),
                    "New branch",
                    true,
                )
                .on_hover_text(format!("Start a new branch here  ({key})"))
                .clicked()
                {
                    chosen = Some(Action::NewBranch { at: None });
                }
                let merge_response = widgets::outline_button_at(
                    ui,
                    merge,
                    Id::unique("pick-branch-merge"),
                    Some(icon::BRANCH),
                    "Merge into this…",
                    "Merge another branch into the current one",
                );
                let merge_popup = egui::Popup::menu(&merge_response)
                    .close_behavior(PopupCloseBehavior::CloseOnClickOutside);
                merge_popup.show(|ui| {
                    ui.set_min_width(220.0);
                    for branch in locals.iter().chain(remotes.iter()) {
                        if ui.button(&branch.name).clicked() {
                            chosen = Some(Action::MergeIntoCurrent(branch.name.clone()));
                            ui.close();
                        }
                    }
                });
                if chosen.is_some() || close {
                    ui.ctx().data_mut(|data| data.remove::<String>(query_id));
                    ui.close();
                }
            });
        self.pending.extend(chosen);
    }

    /// Sync (↑ ahead ↓ behind), or the running network operation with its progress and a
    /// Cancel button. Right-aligned at `right`; `None` when there is nothing to sync with.
    fn sync_button(&mut self, ui: &mut Ui, right: f32, center_y: f32) -> Option<Rect> {
        let palette = Palette::of(ui.ctx());
        let tab = self.active_tab().filter(|tab| ready_info(tab).is_some())?;
        if let Some(network) = &tab.network {
            let progress = network.progress();
            let elapsed = network.started.elapsed().as_secs();
            let text = if progress.is_empty() {
                format!("{}… {elapsed}s", network.label)
            } else {
                format!("{}… {progress}", network.label)
            };
            let galley = widgets::elided(ui, &text, theme::small(), palette.text_secondary, 260.0);
            let width = 12.0 + galley.size().x + 8.0 + 24.0 + 4.0;
            let rect = Rect::from_min_size(
                Pos2::new(right - width, center_y - CONTROL_HEIGHT / 2.0),
                Vec2::new(width, CONTROL_HEIGHT),
            );
            ui.painter().rect(
                rect,
                CornerRadius::same(radius::LAYER),
                palette.surface,
                palette.hairline_stroke(),
                StrokeKind::Inside,
            );
            ui.painter().galley(
                Pos2::new(rect.left() + 12.0, center_y - galley.size().y / 2.0),
                galley,
                palette.text_secondary,
            );
            let cancel = Rect::from_center_size(
                Pos2::new(rect.right() - 4.0 - 12.0, center_y),
                Vec2::splat(24.0),
            );
            if icon_button_at(
                ui,
                cancel,
                Id::unique("title-network-cancel"),
                icon::CLOSE,
                "Stop the running network operation",
            )
            .clicked()
            {
                self.pending.push(Action::CancelNetwork);
            }
            // Keep the elapsed time / progress moving.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
            return Some(rect);
        }

        let next = sync_step(tab)?;
        let branch = &tab.work.branch;
        let mut counts = Vec::new();
        if let Some(ahead) = branch.ahead.filter(|n| *n > 0) {
            counts.push(format!("↑{ahead}"));
        }
        if let Some(behind) = branch.behind.filter(|n| *n > 0) {
            counts.push(format!("↓{behind}"));
        }
        let counts = counts.join(" ");
        let upstream = branch.upstream.clone().unwrap_or_default();
        let tooltip = match next {
            Action::Pull => format!(
                "Pull from {upstream} ({}), then Sync again to push",
                self.settings.pull_mode.label()
            ),
            Action::Push if branch.unpublished() => {
                "Publish this branch to the remote".to_owned()
            }
            Action::Push => format!("Push to {upstream}"),
            _ => "Fetch every remote to see what's new".to_owned(),
        };
        let title = theme::strong();
        let title_width = ui
            .painter()
            .layout_no_wrap("Sync".to_owned(), title.clone(), palette.text)
            .size()
            .x;
        let counts_galley =
            ui.painter()
                .layout_no_wrap(counts.clone(), theme::small(), palette.text_tertiary);
        let counts_width = if counts.is_empty() {
            0.0
        } else {
            counts_galley.size().x + space::S
        };
        let width = 10.0 + 16.0 + space::S + title_width + counts_width + 12.0;
        let rect = Rect::from_min_size(
            Pos2::new(right - width, center_y - CONTROL_HEIGHT / 2.0),
            Vec2::new(width, CONTROL_HEIGHT),
        );
        let response = ui.interact(rect, Id::unique("title-sync"), Sense::click());
        let fill = widgets::mix(
            palette.surface,
            palette.hover,
            if response.hovered() { 1.0 } else { 0.0 },
        );
        ui.painter().rect(
            rect,
            CornerRadius::same(radius::LAYER),
            if response.is_pointer_button_down_on() {
                widgets::mix(palette.surface, palette.pressed, 1.0)
            } else {
                fill
            },
            palette.hairline_stroke(),
            StrokeKind::Inside,
        );
        ui.painter().text(
            Pos2::new(rect.left() + 10.0 + 8.0, center_y),
            Align2::CENTER_CENTER,
            icon::SYNC,
            theme::icon_font(14.0),
            palette.text,
        );
        let mut x = rect.left() + 10.0 + 16.0 + space::S;
        ui.painter().text(
            Pos2::new(x, center_y),
            Align2::LEFT_CENTER,
            "Sync",
            title,
            palette.text,
        );
        x += title_width + space::S;
        if !counts.is_empty() {
            ui.painter().galley(
                Pos2::new(x, center_y - counts_galley.size().y / 2.0),
                counts_galley,
                palette.text_tertiary,
            );
        }
        widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);
        if response.on_hover_text(tooltip).clicked() {
            self.pending.push(Action::Sync);
        }
        Some(rect)
    }
}

/// A repository row in the switcher: monogram, name over path, and a detail on the right.
fn repo_row_text(
    ui: &Ui,
    row: Rect,
    name: &str,
    path: &str,
    detail: &str,
    ready: bool,
    palette: &Palette,
) {
    let tile = Rect::from_min_size(
        Pos2::new(row.left() + space::S, row.center().y - 16.0),
        Vec2::splat(32.0),
    );
    widgets::monogram(ui.painter(), tile, name, palette);
    let detail_width = if detail.is_empty() {
        0.0
    } else {
        widgets::elided(ui, detail, theme::small(), palette.text_secondary, 150.0)
            .size()
            .x
            + space::M
    };
    let text_left = tile.right() + space::M - space::XS;
    let text_width = row.right() - space::S - detail_width - text_left;
    text_line(
        ui,
        Pos2::new(text_left, row.center().y - 9.0),
        name,
        theme::strong(),
        if ready {
            palette.text
        } else {
            palette.text_tertiary
        },
        text_width,
    );
    text_line(
        ui,
        Pos2::new(text_left, row.center().y + 10.0),
        path,
        theme::small(),
        palette.text_tertiary,
        text_width,
    );
    if !detail.is_empty() {
        let galley = widgets::elided(ui, detail, theme::small(), palette.text_secondary, 150.0);
        ui.painter().galley(
            Pos2::new(
                row.right() - space::S - galley.size().x,
                row.center().y - galley.size().y / 2.0,
            ),
            galley,
            palette.text_secondary,
        );
    }
}

/// `origin` of `origin/main`.
fn remote_of(upstream: &str) -> &str {
    upstream.split('/').next().unwrap_or(upstream)
}

/// A flyout row's name over its detail (from 36 px in), and an optional note on the right.
fn two_lines(
    ui: &Ui,
    row: Rect,
    name: &str,
    font: egui::FontId,
    color: egui::Color32,
    detail: &str,
    note: &str,
) {
    let palette = Palette::of(ui.ctx());
    let mut right = row.right() - 10.0;
    if !note.is_empty() {
        right = ui
            .painter()
            .text(
                Pos2::new(right, row.center().y),
                Align2::RIGHT_CENTER,
                note,
                theme::caption(),
                palette.text_tertiary,
            )
            .left()
            - space::S;
    }
    let left = row.left() + 36.0;
    text_line(ui, Pos2::new(left, row.center().y - 8.0), name, font, color, right - left);
    text_line(
        ui,
        Pos2::new(left, row.center().y + 9.0),
        detail,
        theme::small(),
        palette.text_tertiary,
        right - left,
    );
}

/// A repository's display name: its folder name.
fn repo_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl FacetApp {
    /// The repository button's context menu: open the repository in File Explorer or one of
    /// the programs found installed (also in the palette).
    fn open_in_menu(&mut self, response: &egui::Response) {
        let targets = self.installed.open_targets();
        let mut chosen = None;
        response.context_menu(|ui| {
            let explorer = widgets::tooltip_with_shortcut(ui, "Show in Explorer", &shortcut::REVEAL);
            if ui.button(explorer).clicked() {
                chosen = Some(Action::RevealInExplorer);
            }
            for target in targets {
                if ui.button(format!("Open in {}", target.label())).clicked() {
                    chosen = Some(Action::OpenIn(target));
                }
            }
            ui.separator();
            if ui.button("Copy Path").clicked() {
                chosen = Some(Action::CopyRepositoryPath);
            }
        });
        self.pending.extend(chosen);
    }
}
