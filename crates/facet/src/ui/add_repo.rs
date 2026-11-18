//! The Add repository dialog: Clone from a URL, open a folder, or create a new repository.
//! Its state and the work it starts live in `app_add.rs`.

use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, CornerRadius, Id, Key, Layout, Modifiers, Pos2, Rect, Response, Sense,
    StrokeKind, Ui, UiBuilder, Vec2,
};

use super::power::modal_area;
use super::widgets::{self, button, button_enabled, icon_button_at, text_field, ButtonKind};
use crate::actions::Action;
use crate::app::FacetApp;
use crate::app_add::{AddRepoView, AddTab, INITIAL_BRANCH};
use crate::theme::{self, icon, radius, space, Palette};

const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 404.0;
const FIELD: f32 = 34.0;
const TABS: f32 = 34.0;
const FOOTER: f32 = 64.0;

impl FacetApp {
    pub(super) fn add_repo_layer(&mut self, ui: &mut Ui, window: Rect) {
        if self.add_view.is_none() {
            return;
        }
        let parent = self.default_parent();
        let dialog_on_top = self.prompt.is_some() || self.confirm.is_some();
        let ctx = ui.ctx().clone();
        let palette = Palette::of(&ctx);
        let mut actions = Vec::new();
        let Some(view) = self.add_view.as_mut() else {
            return;
        };
        if !dialog_on_top
            && view.running.is_none()
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            actions.push(Action::CloseAddRepository);
        }

        let panel = Rect::from_center_size(
            Pos2::new(
                window.center().x,
                window.center().y + theme::TITLE_BAR_HEIGHT / 4.0,
            ),
            Vec2::new(
                WIDTH.min(window.width() - space::XL * 2.0),
                HEIGHT.min(window.height() - space::L),
            ),
        );
        modal_area(&ctx, "add-repository", window, panel, palette, |ui| {
            let inner = panel.shrink2(Vec2::new(space::L, 0.0));
            let running = view.running.is_some();

            // Title and close button.
            let title_y = panel.top() + space::L + 12.0;
            ui.painter().text(
                Pos2::new(inner.left(), title_y),
                Align2::LEFT_CENTER,
                "Add repository",
                theme::heading(),
                palette.text,
            );
            let close =
                Rect::from_center_size(Pos2::new(inner.right() - 12.0, title_y), Vec2::splat(28.0));
            if icon_button_at(
                ui,
                close,
                Id::unique("add-close"),
                icon::CLOSE,
                if running {
                    "Close and stop the clone"
                } else {
                    "Close (Esc)"
                },
            )
            .clicked()
            {
                actions.push(Action::CloseAddRepository);
            }

            // Clone · Open folder · New.
            let tabs = Rect::from_min_size(
                Pos2::new(inner.left(), title_y + 28.0),
                Vec2::new(inner.width(), TABS),
            );
            if let Some(tab) = tab_strip(ui, tabs, view.tab, !running, palette) {
                view.tab = tab;
                view.error = None;
                view.focus_first = true;
            }

            // The tab's fields.
            let body = Rect::from_min_max(
                Pos2::new(inner.left(), tabs.bottom() + space::L),
                Pos2::new(inner.right(), panel.bottom() - FOOTER),
            );
            let submit = ui
                .scope_builder(
                    UiBuilder::new()
                        .id_salt("add-body")
                        .max_rect(body)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_enabled_ui(!running, |ui| match view.tab {
                            AddTab::Clone => clone_fields(ui, view, &parent, &mut actions),
                            AddTab::Open => open_fields(ui, view, &mut actions),
                            AddTab::New => new_fields(ui, view, &mut actions),
                        })
                        .inner
                    },
                )
                .inner;

            // Footer: the error or the running job, then the buttons.
            let footer = Rect::from_min_max(
                Pos2::new(inner.left(), panel.bottom() - FOOTER),
                Pos2::new(inner.right(), panel.bottom()),
            );
            widgets::hairline_h(ui, panel.left()..=panel.right(), footer.top());
            let center_y = footer.center().y;
            let buttons = ui
                .scope_builder(
                    UiBuilder::new()
                        .id_salt("add-footer")
                        .max_rect(Rect::from_min_max(
                            Pos2::new(footer.center().x - 40.0, footer.top()),
                            footer.max,
                        ))
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        if running {
                            let stop = button(
                                ui,
                                ButtonKind::Secondary,
                                None,
                                "Stop",
                                "Stop and remove the half-finished clone",
                            );
                            if stop.clicked() {
                                actions.push(Action::CancelClone);
                            }
                            return stop.rect.left();
                        }
                        let (label, tooltip, action, ready) = primary(view, &parent);
                        let primary =
                            button_enabled(ui, ButtonKind::Primary, None, &label, tooltip, ready);
                        if primary.clicked() {
                            actions.push(action);
                        }
                        let cancel =
                            button(ui, ButtonKind::Secondary, None, "Cancel", "Close (Esc)");
                        if cancel.clicked() {
                            actions.push(Action::CloseAddRepository);
                        }
                        cancel.rect.left()
                    },
                )
                .inner;
            let status_width = buttons - space::M - footer.left();
            if let Some(job) = &view.running {
                let progress = job.progress();
                let elapsed = job.started.elapsed().as_secs();
                let text = if progress.is_empty() {
                    format!("{}… {elapsed}s", job.label)
                } else {
                    format!("{}… {progress}", job.label)
                };
                widgets::text_line(
                    ui,
                    Pos2::new(footer.left(), center_y),
                    &text,
                    theme::small(),
                    palette.text_secondary,
                    status_width,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            } else if let Some(error) = &view.error {
                let galley =
                    widgets::elided(ui, error, theme::small(), palette.danger, status_width);
                let rect = Rect::from_min_size(
                    Pos2::new(footer.left(), center_y - galley.size().y / 2.0),
                    galley.size(),
                );
                ui.painter().galley(rect.min, galley, palette.danger);
                ui.interact(rect, Id::unique("add-error"), Sense::hover())
                    .on_hover_text(error.as_str());
            }

            if submit && !running {
                let (_, _, action, ready) = primary(view, &parent);
                if ready {
                    actions.push(action);
                }
            }
        });
        self.pending.extend(actions);
    }
}

/// The primary button for the tab: label, tooltip, action, and whether it can run yet.
fn primary(view: &AddRepoView, parent: &std::path::Path) -> (String, &'static str, Action, bool) {
    match view.tab {
        AddTab::Clone => {
            let name = facet_git::clone_folder_name(&view.url);
            let dest = if view.clone_dest_edited {
                view.clone_dest.trim().to_owned()
            } else {
                view.suggested_dest(parent)
            };
            (
                name.as_ref()
                    .map_or_else(|| "Clone".to_owned(), |name| format!("Clone {name}")),
                "Download the repository and open it",
                Action::StartClone,
                !view.url.trim().is_empty() && !dest.is_empty(),
            )
        }
        AddTab::Open => (
            "Open".to_owned(),
            "Open the repository in this folder",
            Action::OpenTypedPath,
            !view.open_path.trim().is_empty(),
        ),
        AddTab::New => (
            "Create".to_owned(),
            "Create the folder with an empty repository, and open it",
            Action::CreateRepository,
            view.new_dest().is_some(),
        ),
    }
}

/// A segmented control spanning `rect`; returns the tab clicked (if it isn't the current one).
fn tab_strip(
    ui: &Ui,
    rect: Rect,
    current: AddTab,
    enabled: bool,
    palette: &Palette,
) -> Option<AddTab> {
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::LAYER),
        palette.hover,
        egui::Stroke::NONE,
        StrokeKind::Inside,
    );
    let width = (rect.width() - 4.0) / AddTab::ALL.len() as f32;
    let mut chosen = None;
    for (index, tab) in AddTab::ALL.into_iter().enumerate() {
        let segment = Rect::from_min_size(
            Pos2::new(rect.left() + 2.0 + index as f32 * width, rect.top() + 2.0),
            Vec2::new(width, rect.height() - 4.0),
        );
        let active = tab == current;
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(segment, Id::unique(("add-tab", index)), sense);
        if active {
            ui.painter().rect(
                segment,
                CornerRadius::same(radius::M),
                palette.raised,
                palette.hairline_stroke(),
                StrokeKind::Inside,
            );
        } else if response.hovered() && enabled {
            ui.painter()
                .rect_filled(segment, CornerRadius::same(radius::M), palette.hover);
        }
        widgets::focus_ring(ui, &response, segment, radius::M, palette);
        ui.painter().text(
            segment.center(),
            Align2::CENTER_CENTER,
            tab.label(),
            if active {
                theme::strong()
            } else {
                theme::body()
            },
            if active {
                palette.text
            } else {
                palette.text_secondary
            },
        );
        if response.clicked() && !active {
            chosen = Some(tab);
        }
    }
    chosen
}

/// A field's name above it.
fn field_label(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.left(), rect.top() + 8.0),
        Align2::LEFT_CENTER,
        text,
        theme::small_strong(),
        palette.text_secondary,
    );
}

/// A full-width text field; `true` when Enter was pressed in it.
fn field(ui: &mut Ui, id: &'static str, text: &mut String, hint: &str) -> (Response, bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), FIELD), Sense::hover());
    let response = text_field(ui, rect, Id::unique(("add-field", id)), text, hint);
    let entered = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    (response, entered)
}

/// A text field with Browse… after it; `true` when Enter was pressed in it.
fn folder_field(
    ui: &mut Ui,
    id: &'static str,
    text: &mut String,
    hint: &str,
    actions: &mut Vec<Action>,
) -> (Response, bool) {
    let browse_width = 92.0;
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), FIELD), Sense::hover());
    let field = Rect::from_min_max(
        row.min,
        Pos2::new(row.right() - browse_width - space::S, row.bottom()),
    );
    let response = text_field(ui, field, Id::unique(("add-field", id)), text, hint);
    let entered = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    let browse = Rect::from_min_max(Pos2::new(field.right() + space::S, row.top()), row.max);
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(("add-browse", id))
            .max_rect(browse)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            if button(
                ui,
                ButtonKind::Secondary,
                Some(icon::FOLDER_OPEN),
                "Browse…",
                "Choose a folder",
            )
            .clicked()
            {
                actions.push(Action::BrowseAddFolder);
            }
        },
    );
    (response, entered)
}

fn hint(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    ui.add_space(space::S);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::small())
                .color(palette.text_tertiary),
        )
        .wrap(),
    );
}

fn focus_first(view: &mut AddRepoView, response: &Response) {
    if std::mem::take(&mut view.focus_first) {
        response.request_focus();
    }
}

fn clone_fields(
    ui: &mut Ui,
    view: &mut AddRepoView,
    parent: &std::path::Path,
    actions: &mut Vec<Action>,
) -> bool {
    field_label(ui, "Repository URL");
    let (url, mut submit) = field(
        ui,
        "url",
        &mut view.url,
        "https://github.com/owner/repository.git",
    );
    focus_first(view, &url);
    hint(
        ui,
        "An HTTPS or SSH address from GitHub, GitLab, Azure DevOps or any Git server, or the \
         path of another repository.",
    );
    ui.add_space(space::M);
    field_label(ui, "Save to");
    if !view.clone_dest_edited {
        view.clone_dest = view.suggested_dest(parent);
    }
    let parent_hint = parent.join("repository").display().to_string();
    let (dest, entered) = folder_field(ui, "dest", &mut view.clone_dest, &parent_hint, actions);
    if dest.changed() {
        view.clone_dest_edited = !view.clone_dest.trim().is_empty();
    }
    submit |= entered;
    submit
}

fn open_fields(ui: &mut Ui, view: &mut AddRepoView, actions: &mut Vec<Action>) -> bool {
    field_label(ui, "Folder");
    let (path, submit) = folder_field(
        ui,
        "open",
        &mut view.open_path,
        r"D:\Projects\my-repository",
        actions,
    );
    focus_first(view, &path);
    hint(
        ui,
        "A folder with a Git repository, or any folder inside one. You can also drop a folder \
         onto the window.",
    );
    submit
}

fn new_fields(ui: &mut Ui, view: &mut AddRepoView, actions: &mut Vec<Action>) -> bool {
    field_label(ui, "Name");
    let (name, mut submit) = field(ui, "new-name", &mut view.new_name, "my-project");
    focus_first(view, &name);
    ui.add_space(space::M);
    field_label(ui, "Location");
    let (_, entered) = folder_field(ui, "new-parent", &mut view.new_parent, "", actions);
    submit |= entered;
    let text = view.new_dest().map_or_else(
        || format!("Creates a folder with an empty repository on branch {INITIAL_BRANCH}."),
        |dest| {
            format!(
                "Creates {} with an empty repository on branch {INITIAL_BRANCH}.",
                dest.display()
            )
        },
    );
    hint(ui, &text);
    submit
}
