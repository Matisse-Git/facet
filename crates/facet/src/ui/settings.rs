//! The Settings page (in the content area, like Changes and History): appearance, git,
//! external tools, open-in integrations and `.gitignore` templates, each a card of rows with
//! the setting's name on the left and its control on the right. Every change goes through an
//! [`Action`], so the palette offers the same settings.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Label, Layout, Margin, Modifiers,
    Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2,
};

use super::region;
use super::widgets::{
    self, button, button_enabled, icon_button_at, text_button_at, text_button_width, ButtonKind,
};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, GitStatus};
use crate::app_settings::{GitignorePreview, SettingsView};
use crate::gitignore::Template;
use crate::integrations::{Installed, OpenTarget, ToolPreset};
use crate::settings::{CodeFont, PullMode, ThemeMode, UI_SCALES};
use crate::theme::{self, icon, radius, space, Palette};

const SEGMENT_HEIGHT: f32 = 28.0;
const FIELD_HEIGHT: f32 = 32.0;
/// The page's column is at most this wide, centered in the content area.
const PAGE_WIDTH: f32 = 720.0;

/// What the view shows from the rest of the app, copied so the view itself can be borrowed
/// mutably while drawing.
struct Snapshot {
    theme: ThemeMode,
    ui_scale: f32,
    code_font: CodeFont,
    pull_mode: PullMode,
    git: String,
    custom_git: bool,
    /// External tools: label and command line.
    tools: Vec<(String, String)>,
    installed: Installed,
    /// A repository tab is open.
    has_repo: bool,
    /// The repository whose `.gitignore` the templates go to (one with a working tree).
    gitignore_repo: Option<String>,
}

impl FacetApp {
    /// Settings, drawn in the content area while it is open (sidebar, Ctrl+, or the palette).
    pub(super) fn settings_page(&mut self, ui: &mut Ui, rect: Rect) {
        let snapshot = Snapshot {
            theme: self.settings.theme,
            ui_scale: self.settings.ui_scale,
            code_font: self.settings.code_font,
            pull_mode: self.settings.pull_mode,
            git: match &self.git_status {
                GitStatus::Checking => "Checking for Git…".to_owned(),
                GitStatus::Found(version) => format!(
                    "Git {version} from {}",
                    self.settings
                        .git_path
                        .as_ref()
                        .map_or_else(|| "PATH".to_owned(), |path| path.display().to_string())
                ),
                GitStatus::Missing(_) => "Git was not found".to_owned(),
            },
            custom_git: self.settings.git_path.is_some(),
            tools: self
                .settings
                .external_tools
                .iter()
                .map(|tool| {
                    let command = if tool.merge.is_empty() {
                        &tool.diff
                    } else {
                        &tool.merge
                    };
                    (tool.label(), command.clone())
                })
                .collect(),
            installed: self.installed.clone(),
            has_repo: self.active_tab().is_some_and(crate::app::RepoTab::is_ready),
            gitignore_repo: self
                .active_tab()
                .filter(|tab| crate::app_work::ready_info(tab).is_some())
                .map(crate::app::RepoTab::name),
        };
        let dialog_on_top = self.prompt.is_some()
            || self.confirm.is_some()
            || self.palette.open
            || self.add_view.is_some();
        let ctx = ui.ctx().clone();
        let palette = Palette::of(&ctx);
        let mut actions = Vec::new();
        if !dialog_on_top && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            actions.push(Action::CloseSettings);
        }
        let Some(view) = self.settings_view.as_mut() else {
            return;
        };

        region(ui, "settings-page", rect, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("settings-scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    let width = ui.available_width();
                    let column = (width - space::XL * 2.0).clamp(240.0, PAGE_WIDTH);
                    let side = ((width - column) / 2.0).max(0.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(side);
                        ui.vertical(|ui| {
                            ui.set_width(column);
                            ui.add_space(space::XL);
                            // Title and close button.
                            let (header, _) =
                                ui.allocate_exact_size(Vec2::new(column, 36.0), Sense::hover());
                            ui.painter().text(
                                Pos2::new(header.left(), header.center().y),
                                Align2::LEFT_CENTER,
                                "Settings",
                                theme::title(),
                                palette.text,
                            );
                            let close = Rect::from_center_size(
                                Pos2::new(header.right() - 16.0, header.center().y),
                                Vec2::splat(32.0),
                            );
                            if icon_button_at(
                                ui,
                                close,
                                Id::unique("settings-close"),
                                icon::CLOSE,
                                "Close Settings (Esc)",
                            )
                            .clicked()
                            {
                                actions.push(Action::CloseSettings);
                            }
                            settings_body(ui, &snapshot, view, &mut actions);
                            ui.add_space(space::L);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(format!("Facet {}", env!("CARGO_PKG_VERSION")))
                                        .font(theme::small())
                                        .color(palette.text_tertiary),
                                );
                            });
                            ui.add_space(space::XL);
                        });
                    });
                });
        });
        self.pending.extend(actions);
    }
}

fn settings_body(ui: &mut Ui, snap: &Snapshot, view: &mut SettingsView, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());

    // ----- appearance
    section(ui, "Appearance");
    card(ui, |ui| {
        stacked_row(
            ui,
            "Theme",
            Some("System follows the Windows setting."),
            |ui| {
                if let Some(mode) = theme_cards(ui, snap.theme) {
                    actions.push(Action::SetTheme(mode));
                }
            },
        );
        divider(ui);
        row(
            ui,
            "Interface size",
            Some("Ctrl+= and Ctrl+- change it too; Ctrl+0 goes back to 100%."),
            |ui| {
                let scales = UI_SCALES.map(|scale| (scale, format!("{:.0}%", scale * 100.0)));
                let current = UI_SCALES
                    .into_iter()
                    .find(|scale| (scale - snap.ui_scale).abs() < 0.001)
                    .unwrap_or(f32::NAN);
                if let Some(scale) = segmented(ui, "scale", &scales, current) {
                    actions.push(Action::SetUiScale(scale));
                }
            },
        );
        divider(ui);
        row(
            ui,
            "Code font",
            Some("Diffs, commit ids and the command log."),
            |ui| {
                if let Some(font) = segmented(
                    ui,
                    "code-font",
                    &CodeFont::ALL.map(|font| (font, font.label().to_owned())),
                    snap.code_font,
                ) {
                    actions.push(Action::SetCodeFont(font));
                }
            },
        );
        ui.add_space(space::XS);
        Frame::NONE
            .fill(palette.surface)
            .stroke(palette.hairline_stroke())
            .corner_radius(CornerRadius::same(radius::M))
            .inner_margin(Margin::symmetric(space::S as i8, space::XS as i8 + 2))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("let commit = \"4659738\";  // 0O 1lI {}[] => !=")
                        .font(theme::mono())
                        .color(palette.text_secondary),
                );
            });
        ui.add_space(space::M);
    });

    // ----- git
    section(ui, "Git");
    card(ui, |ui| {
        stacked_row(ui, "Git executable", Some(&snap.git), |ui| {
            git_path_field(ui, snap, view, actions);
        });
        divider(ui);
        row(
            ui,
            "When pulling",
            Some("How Pull (and Sync) bring in the upstream's commits."),
            |ui| {
                if let Some(mode) = segmented(
                    ui,
                    "pull-mode",
                    &PullMode::ALL.map(|mode| (mode, mode.label().to_owned())),
                    snap.pull_mode,
                ) {
                    actions.push(Action::SetPullMode(mode));
                }
            },
        );
    });

    // ----- external tools
    section(ui, "Diff and merge tools");
    card(ui, |ui| {
        ui.add_space(space::M);
        note(
            ui,
            "Chosen by file extension; the first matching tool wins. Without one, diffs and \
             conflicts open in Facet's own views.",
            palette.text_secondary,
        );
        ui.add_space(space::S);
        for (index, (name, command)) in snap.tools.iter().enumerate() {
            divider(ui);
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), theme::ROW_HEIGHT * 1.6),
                Sense::hover(),
            );
            let text_width = rect.width() - 40.0;
            widgets::text_line(
                ui,
                Pos2::new(rect.left(), rect.top() + 14.0),
                name,
                theme::body(),
                palette.text,
                text_width,
            );
            widgets::text_line(
                ui,
                Pos2::new(rect.left(), rect.bottom() - 14.0),
                command,
                theme::small(),
                palette.text_tertiary,
                text_width,
            );
            let remove = Rect::from_center_size(
                Pos2::new(rect.right() - 14.0, rect.center().y),
                Vec2::splat(28.0),
            );
            if icon_button_at(
                ui,
                remove,
                Id::unique(("settings-remove-tool", index)),
                icon::DELETE,
                "Remove this tool",
            )
            .clicked()
            {
                actions.push(Action::RemoveExternalTool(index));
            }
        }
        divider(ui);
        ui.add_space(space::S + 2.0);
        ui.horizontal_wrapped(|ui| {
            if button(
                ui,
                ButtonKind::Secondary,
                Some(icon::ADD),
                "Add Tool…",
                "Set up any diff or merge program",
            )
            .clicked()
            {
                actions.push(Action::AddExternalTool);
            }
            for preset in ToolPreset::ALL {
                let tooltip = snap.installed.preset(preset).map_or_else(
                    || "Not found in the usual folders; you can enter its path".to_owned(),
                    |path| format!("Found at {}", path.display()),
                );
                if button(ui, ButtonKind::Secondary, None, preset.label(), &tooltip).clicked() {
                    actions.push(Action::AddExternalToolPreset(preset));
                }
            }
        });
        ui.add_space(space::S + 2.0);
    });

    // ----- open in
    section(ui, "Open in");
    card(ui, |ui| {
        for (index, target) in OpenTarget::ALL.into_iter().enumerate() {
            if index > 0 {
                divider(ui);
            }
            let found = snap.installed.launcher(target);
            let status =
                found.map_or_else(|| "Not found".to_owned(), |path| path.display().to_string());
            let clicked = integration_row(
                ui,
                target.label(),
                &status,
                found.is_some(),
                snap.has_repo,
                &format!("Open the repository in {}", target.label()),
            );
            if clicked {
                actions.push(Action::OpenIn(target));
            }
        }
        divider(ui);
        let tooltip = widgets::tooltip_with_shortcut(
            ui,
            "Open the repository in File Explorer",
            &shortcut::REVEAL,
        );
        if integration_row(
            ui,
            "File Explorer",
            "Always available",
            true,
            snap.has_repo,
            &tooltip,
        ) {
            actions.push(Action::RevealInExplorer);
        }
    });

    // ----- .gitignore templates
    let response = section(ui, ".gitignore templates");
    if view.reveal_gitignore {
        view.reveal_gitignore = false;
        response.scroll_to_me(Some(Align::Min));
    }
    card(ui, |ui| {
        ui.add_space(space::M);
        match &snap.gitignore_repo {
            None => note(
                ui,
                "Open a repository with a working folder to add an engine's rules to its \
                 .gitignore.",
                palette.text_secondary,
            ),
            Some(repo) => {
                note(
                    ui,
                    &format!(
                        "Adds the rules {repo}'s .gitignore doesn't have yet, under a comment. \
                         Undo puts the file back."
                    ),
                    palette.text_secondary,
                );
                ui.add_space(space::S);
                if let Some(template) = segmented(
                    ui,
                    "gitignore-template",
                    &Template::ALL.map(|template| (template, template.label().to_owned())),
                    view.template,
                ) {
                    actions.push(Action::ShowGitignoreTemplate(template));
                }
                ui.add_space(space::S);
                gitignore_preview(ui, view);
                ui.add_space(space::S);
                let ready =
                    matches!(&view.preview, GitignorePreview::Ready(block) if !block.is_empty());
                if button_enabled(
                    ui,
                    ButtonKind::Primary,
                    None,
                    "Append to .gitignore",
                    "Add the rules shown above to the end of .gitignore",
                    ready,
                )
                .clicked()
                {
                    actions.push(Action::AddGitignoreTemplate(view.template));
                }
            }
        }
        ui.add_space(space::M);
    });
}

/// The git executable: a path field, Browse…, Check and Use, and the way back to PATH.
fn git_path_field(
    ui: &mut Ui,
    snap: &Snapshot,
    view: &mut SettingsView,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let submitted = ui
        .horizontal(|ui| {
            let buttons = 230.0;
            let width = (ui.available_width() - buttons).max(160.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, FIELD_HEIGHT), Sense::hover());
            let id = Id::unique("settings-git-path");
            let focused = ui.ctx().memory(|memory| memory.has_focus(id));
            ui.painter().rect(
                rect,
                CornerRadius::same(radius::M),
                palette.surface,
                if focused {
                    Stroke::new(1.5, palette.accent)
                } else {
                    palette.hairline_stroke()
                },
                StrokeKind::Inside,
            );
            let field = ui.put(
                rect,
                TextEdit::singleline(&mut view.git_path)
                    .id(id)
                    .hint_text("git  (found on PATH)")
                    .font(theme::body())
                    .frame(Frame::NONE.inner_margin(Margin::symmetric(space::S as i8, 0)))
                    .align(Align2::LEFT_CENTER),
            );
            let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            if button(
                ui,
                ButtonKind::Secondary,
                None,
                "Browse…",
                "Choose git.exe with a file dialog",
            )
            .clicked()
            {
                actions.push(Action::BrowseGitPath);
            }
            let check = button_enabled(
                ui,
                ButtonKind::Primary,
                None,
                "Check and Use",
                "Run `git version` with this executable, and use it if that works",
                !view.checking_git,
            )
            .clicked();
            entered || check
        })
        .inner;
    if submitted && !view.checking_git {
        let text = view.git_path.trim().trim_matches('"');
        actions.push(Action::SetGitPath(
            (!text.is_empty()).then(|| std::path::PathBuf::from(text)),
        ));
    }
    if view.checking_git {
        ui.add_space(space::XS);
        note(ui, "Checking…", palette.text_secondary);
    } else if let Some(error) = &view.git_error {
        ui.add_space(space::XS);
        note(ui, error, palette.danger);
    }
    if snap.custom_git {
        ui.add_space(space::S);
        if button(
            ui,
            ButtonKind::Secondary,
            None,
            "Use Git from PATH",
            "Forget the chosen executable and run the git found on PATH",
        )
        .clicked()
        {
            actions.push(Action::SetGitPath(None));
        }
    }
}

/// System / Light / Dark as three small window previews.
fn theme_cards(ui: &mut Ui, current: ThemeMode) -> Option<ThemeMode> {
    let palette = Palette::of(ui.ctx());
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        for mode in ThemeMode::ALL {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(132.0, 112.0), Sense::click());
            let preview = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 84.0));
            let selected = mode == current;
            let painter = ui.painter();
            let light = (
                Color32::from_rgb(0xEE, 0xF1, 0xF5),
                Color32::WHITE,
                Color32::from_rgb(0xE5, 0xE5, 0xE5),
                Color32::from_rgb(0x00, 0x67, 0xC0),
            );
            let dark = (
                Color32::from_rgb(0x20, 0x20, 0x20),
                Color32::from_rgb(0x1C, 0x1C, 0x1C),
                Color32::from_rgb(0x32, 0x32, 0x32),
                Color32::from_rgb(0x60, 0xCD, 0xFF),
            );
            let draw = |clip: Rect, (backdrop, surface, line, accent): (Color32, Color32, Color32, Color32)| {
                let painter = painter.with_clip_rect(clip.intersect(ui.clip_rect()));
                painter.rect_filled(preview, CornerRadius::same(radius::LAYER), backdrop);
                let content = Rect::from_min_max(
                    preview.min + Vec2::new(34.0, 16.0),
                    preview.max,
                );
                painter.rect_filled(
                    content,
                    CornerRadius {
                        nw: 6,
                        se: radius::LAYER,
                        ..CornerRadius::ZERO
                    },
                    surface,
                );
                for (index, width) in [18.0, 14.0, 16.0].into_iter().enumerate() {
                    painter.rect_filled(
                        Rect::from_min_size(
                            preview.min + Vec2::new(8.0, 22.0 + index as f32 * 9.0),
                            Vec2::new(width, 4.0),
                        ),
                        CornerRadius::same(2),
                        if index == 0 { accent } else { line },
                    );
                }
                for (index, width) in [52.0, 70.0, 40.0].into_iter().enumerate() {
                    painter.rect_filled(
                        Rect::from_min_size(
                            content.min + Vec2::new(10.0, 12.0 + index as f32 * 12.0),
                            Vec2::new(width, 5.0),
                        ),
                        CornerRadius::same(2),
                        line,
                    );
                }
            };
            match mode {
                ThemeMode::Light => draw(preview, light),
                ThemeMode::Dark => draw(preview, dark),
                ThemeMode::System => {
                    let half = preview.center().x;
                    draw(
                        Rect::from_min_max(preview.min, Pos2::new(half, preview.bottom())),
                        light,
                    );
                    draw(
                        Rect::from_min_max(Pos2::new(half, preview.top()), preview.max),
                        dark,
                    );
                }
            }
            let hover = response.hovered() && !selected;
            ui.painter().rect_stroke(
                preview,
                CornerRadius::same(radius::LAYER),
                if selected {
                    Stroke::new(2.0, palette.accent)
                } else if hover {
                    Stroke::new(1.0, palette.text_tertiary)
                } else {
                    palette.hairline_stroke()
                },
                StrokeKind::Outside,
            );
            widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);
            ui.painter().text(
                Pos2::new(rect.center().x, rect.bottom() - 10.0),
                Align2::CENTER_CENTER,
                mode.label(),
                if selected {
                    theme::strong()
                } else {
                    theme::body()
                },
                if selected {
                    palette.text
                } else {
                    palette.text_secondary
                },
            );
            if response.clicked() && !selected {
                chosen = Some(mode);
            }
        }
    });
    chosen
}

fn gitignore_preview(ui: &mut Ui, view: &SettingsView) {
    let palette = Palette::of(ui.ctx());
    Frame::NONE
        .fill(palette.surface)
        .stroke(palette.hairline_stroke())
        .corner_radius(CornerRadius::same(radius::M))
        .inner_margin(Margin::same(space::S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &view.preview {
                GitignorePreview::None => {}
                GitignorePreview::Loading => {
                    let (rect, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 80.0), Sense::hover());
                    widgets::skeleton(ui, rect);
                }
                GitignorePreview::Ready(block) if block.is_empty() => {
                    ui.label(
                        RichText::new(format!(
                            "Every {} rule is already in .gitignore.",
                            view.template.label()
                        ))
                        .font(theme::body())
                        .color(palette.text_secondary),
                    );
                }
                GitignorePreview::Ready(block) => {
                    egui::ScrollArea::vertical()
                        .id_salt("gitignore-preview")
                        .min_scrolled_height(200.0)
                        .max_height(200.0)
                        .show(ui, |ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(block.trim_end())
                                        .font(theme::mono())
                                        .color(palette.success),
                                )
                                .extend(),
                            );
                        });
                }
                GitignorePreview::Failed(detail) => {
                    ui.label(
                        RichText::new(format!("Couldn't read .gitignore: {detail}"))
                            .font(theme::body())
                            .color(palette.danger),
                    );
                }
            }
        });
}

/// A program row: name, where it was found, and an "Open" button.
fn integration_row(
    ui: &mut Ui,
    name: &str,
    status: &str,
    found: bool,
    has_repo: bool,
    tooltip: &str,
) -> bool {
    let palette = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::ROW_HEIGHT * 1.75),
        Sense::hover(),
    );
    let label = "Open";
    let button_width = text_button_width(ui, label) + space::M;
    let text_width = rect.width() - button_width - space::M;
    widgets::text_line(
        ui,
        Pos2::new(rect.left(), rect.top() + 16.0),
        name,
        theme::body(),
        palette.text,
        text_width,
    );
    widgets::text_line(
        ui,
        Pos2::new(rect.left(), rect.bottom() - 16.0),
        status,
        theme::small(),
        if found {
            palette.text_secondary
        } else {
            palette.text_tertiary
        },
        text_width,
    );
    if !found {
        return false;
    }
    let button = Rect::from_min_size(
        Pos2::new(
            rect.right() - button_width,
            rect.center().y - SEGMENT_HEIGHT / 2.0,
        ),
        Vec2::new(button_width, SEGMENT_HEIGHT),
    );
    let color = if has_repo {
        palette.accent
    } else {
        palette.text_tertiary
    };
    let tooltip = if has_repo {
        tooltip
    } else {
        "Open a repository first"
    };
    let response = text_button_at(
        ui,
        button,
        Id::unique(("settings-open-in", name)),
        label,
        tooltip,
        color,
        false,
    );
    has_repo && response.clicked()
}

/// A section's title above its card.
fn section(ui: &mut Ui, text: &str) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    ui.add_space(space::L);
    let response = ui.label(
        RichText::new(text)
            .font(theme::strong())
            .color(palette.text),
    );
    ui.add_space(space::S);
    response
}

/// A rounded card holding a section's rows.
fn card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    Frame::NONE
        .fill(palette.surface_alt)
        .stroke(palette.hairline_stroke())
        .corner_radius(CornerRadius::same(radius::LAYER))
        .inner_margin(Margin::symmetric(space::M as i8, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add_contents(ui);
        });
}

/// A hairline between two rows of a card.
fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    widgets::hairline_h(ui, rect.left()..=rect.right(), rect.center().y);
}

/// The setting's name and an optional explanation under it.
fn row_label(ui: &mut Ui, title: &str, detail: Option<&str>) {
    let palette = Palette::of(ui.ctx());
    ui.label(RichText::new(title).font(theme::body()).color(palette.text));
    if let Some(detail) = detail {
        ui.add_space(2.0);
        note(ui, detail, palette.text_tertiary);
    }
}

/// A setting with its control on the right.
fn row(ui: &mut Ui, title: &str, detail: Option<&str>, control: impl FnOnce(&mut Ui)) {
    ui.add_space(space::M - 2.0);
    ui.horizontal(|ui| {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new((width * 0.42).max(160.0), 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                row_label(ui, title, detail);
            },
        );
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.add_space(space::M - 2.0);
}

/// A setting whose control needs the card's whole width: under its name.
fn stacked_row(ui: &mut Ui, title: &str, detail: Option<&str>, control: impl FnOnce(&mut Ui)) {
    ui.add_space(space::M - 2.0);
    row_label(ui, title, detail);
    ui.add_space(space::S + 2.0);
    control(ui);
    ui.add_space(space::M);
}

fn note(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.add(Label::new(RichText::new(text).font(theme::small()).color(color)).wrap());
}

/// A row of mutually exclusive choices; returns the one clicked (if not already chosen).
fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: &'static str,
    options: &[(T, String)],
    current: T,
) -> Option<T> {
    let palette = Palette::of(ui.ctx());
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| text_button_width(ui, label) + space::M)
        .collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, SEGMENT_HEIGHT + 4.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::M),
        palette.surface,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 2.0;
    let mut chosen = None;
    for ((value, label), width) in options.iter().zip(widths) {
        let segment = Rect::from_min_size(
            Pos2::new(x, rect.top() + 2.0),
            Vec2::new(width, SEGMENT_HEIGHT),
        );
        let active = *value == current;
        let color = if active {
            palette.text
        } else {
            palette.text_secondary
        };
        if text_button_at(
            ui,
            segment,
            Id::unique((salt, label.as_str())),
            label,
            "",
            color,
            active,
        )
        .clicked()
            && !active
        {
            chosen = Some(*value);
        }
        x += width;
    }
    chosen
}
