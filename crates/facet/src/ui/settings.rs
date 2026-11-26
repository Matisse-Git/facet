//! The Settings page (in the content area, like Changes and History), laid out like Windows
//! 11 Settings: a title with a "Find a setting" box, then sections of rows, each its own card
//! with an icon, a name, a one-line explanation and its control on the right. Rarely changed
//! settings are on the Advanced page. Every change goes through an [`Action`], so the palette
//! offers the same settings.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Label, Margin, Modifiers, Pos2,
    Rect, RichText, Sense, Shadow, Stroke, StrokeKind, TextEdit, Ui, Vec2,
};

use super::region;
use super::vector::{self, Icon};
use super::widgets::{self, button, button_enabled, icon_button_at};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, GitStatus};
use crate::app_settings::{GitignorePreview, SettingsPage, SettingsView, ACCOUNT_HOSTS};
use crate::gitignore::Template;
use crate::integrations::{Installed, OpenTarget, ToolPreset};
use crate::settings::{
    fetch_interval_label, CodeFont, Density, Editor, PullMode, Terminal, ThemeMode,
    CODE_FONT_SIZES, FETCH_INTERVALS, UI_SCALES,
};
use crate::theme::{self, icon, radius, space, Palette};
use facet_git::LineEndings;

/// The page's column is at most this wide, from the left edge of the content area.
const COLUMN: f32 = 820.0;
const PAD_X: f32 = 40.0;
const PAD_TOP: f32 = 32.0;
const PAD_BOTTOM: f32 = 48.0;
const ROW_HEIGHT: f32 = 64.0;
const ROW_GAP: f32 = 4.0;
const ROW_PAD: f32 = 18.0;
const SECTION_GAP: f32 = 28.0;
const CONTROL_HEIGHT: f32 = 32.0;
const SEGMENT_HEIGHT: f32 = 28.0;
const FIND_WIDTH: f32 = 260.0;

/// What the view shows from the rest of the app, copied so the view itself can be borrowed
/// mutably while drawing.
struct Snapshot {
    theme: ThemeMode,
    ui_scale: f32,
    code_font: CodeFont,
    code_font_size: f32,
    windows_accent: bool,
    density: Density,
    pull_mode: PullMode,
    auto_fetch: bool,
    fetch_minutes: u32,
    editor: Editor,
    terminal: Terminal,
    git: String,
    custom_git: bool,
    /// External tools: label and command line.
    tools: Vec<(String, String)>,
    /// Saved commit identities.
    identities: Vec<crate::settings::SavedIdentity>,
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
            code_font_size: self.settings.code_font_size,
            windows_accent: self.settings.windows_accent,
            density: self.settings.density,
            pull_mode: self.settings.pull_mode,
            auto_fetch: self.settings.auto_fetch,
            fetch_minutes: self.settings.fetch_minutes,
            editor: self.settings.editor,
            terminal: self.settings.terminal,
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
            identities: self.settings.identities.clone(),
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
        let mut actions = Vec::new();
        let Some(view) = self.settings_view.as_mut() else {
            return;
        };
        let nothing_focused = ctx.memory(|memory| memory.focused().is_none());
        if !dialog_on_top
            && nothing_focused
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            match view.page {
                SettingsPage::Advanced => view.page = SettingsPage::Main,
                SettingsPage::Main => actions.push(Action::CloseSettings),
            }
        }

        region(ui, "settings-page", rect, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("settings-scroll", view.page == SettingsPage::Advanced))
                .auto_shrink(false)
                .show(ui, |ui| {
                    let width = (ui.available_width() - PAD_X * 2.0).clamp(280.0, COLUMN);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        ui.add_space(PAD_X);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            ui.set_width(width);
                            ui.add_space(PAD_TOP);
                            match view.page {
                                SettingsPage::Main => {
                                    main_page(ui, &snapshot, view, &mut actions);
                                }
                                SettingsPage::Advanced => {
                                    advanced_page(ui, &snapshot, view, &mut actions);
                                }
                            }
                            ui.add_space(SECTION_GAP);
                            footer(ui);
                            ui.add_space(PAD_BOTTOM);
                        });
                    });
                });
        });
        self.pending.extend(actions);
    }
}

// ----- pages ------------------------------------------------------------------------------

fn main_page(ui: &mut Ui, snap: &Snapshot, view: &mut SettingsView, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    header(ui, "Settings", None, Some(&mut view.find), actions);
    let find = view.find.trim().to_lowercase();
    let shown = |words: &[&str]| {
        find.is_empty()
            || words
                .iter()
                .any(|word| word.to_lowercase().contains(find.as_str()))
    };
    let mut any = false;

    // ----- appearance
    let theme_words = ["Theme", "Light", "Dark", "Match Windows", "appearance"];
    let accent_words = ["Accent color", "Use the accent color you picked in Windows"];
    let font_words = ["Code font", "Used for diffs, blame and conflicts", "size"];
    let density_words = ["Density", "Compact", "Comfortable", "rows"];
    if [&theme_words[..], &accent_words, &font_words, &density_words]
        .iter()
        .any(|words| shown(words))
    {
        any = true;
        section(ui, "Appearance");
        if shown(&theme_words) {
            theme_card(ui, snap.theme, actions);
            ui.add_space(ROW_GAP);
        }
        if shown(&accent_words) {
            setting_row(
                ui,
                &vector::ACCENT,
                accent_words[0],
                accent_words[1],
                |ui, right, y| {
                    let on = snap.windows_accent;
                    let switch_rect = Rect::from_center_size(
                        Pos2::new(right - 20.0, y),
                        Vec2::new(40.0, 20.0),
                    );
                    if switch(ui, switch_rect, Id::unique("settings-accent"), on).clicked() {
                        actions.push(Action::SetWindowsAccent(!on));
                    }
                    let label = if on { "On" } else { "Off" };
                    let galley = ui
                        .painter()
                        .layout_no_wrap(label.into(), theme::body(), palette.text_secondary);
                    let width = galley.size().x;
                    ui.painter().galley(
                        Pos2::new(switch_rect.left() - 16.0 - width, y - galley.size().y / 2.0),
                        galley,
                        palette.text_secondary,
                    );
                    40.0 + 16.0 + width
                },
            );
        }
        if shown(&font_words) {
            setting_row(
                ui,
                &vector::CODE,
                font_words[0],
                font_words[1],
                |ui, right, y| {
                    let label = format!(
                        "{} · {:.0}",
                        snap.code_font.label(),
                        snap.code_font_size
                    );
                    let mut items: Vec<(String, bool, Action)> = CodeFont::ALL
                        .into_iter()
                        .map(|font| {
                            (
                                font.label().to_owned(),
                                font == snap.code_font,
                                Action::SetCodeFont(font),
                            )
                        })
                        .collect();
                    items.extend(CODE_FONT_SIZES.into_iter().map(|size| {
                        (
                            format!("{size:.0} pt"),
                            (size - snap.code_font_size).abs() < 0.01,
                            Action::SetCodeFontSize(size),
                        )
                    }));
                    dropdown(
                        ui,
                        Id::unique("settings-code-font"),
                        right,
                        y,
                        200.0,
                        &label,
                        &items,
                        Some(CodeFont::ALL.len()),
                        actions,
                    )
                },
            );
        }
        if shown(&density_words) {
            setting_row(
                ui,
                &vector::DENSITY,
                density_words[0],
                "Compact fits more files and commits on screen",
                |ui, right, y| {
                    let options = Density::ALL.map(Density::label);
                    let current = Density::ALL
                        .iter()
                        .position(|density| *density == snap.density)
                        .unwrap_or(0);
                    let (width, chosen) =
                        segmented(ui, Id::unique("settings-density"), right, y, &options, current);
                    if let Some(index) = chosen {
                        actions.push(Action::SetDensity(Density::ALL[index]));
                    }
                    width
                },
            );
        }
    }

    // ----- accounts
    let hosts_shown: Vec<bool> = ACCOUNT_HOSTS
        .iter()
        .map(|(name, host)| shown(&[name, host, "account", "sign in", "sign out"]))
        .collect();
    if hosts_shown.iter().any(|shown| *shown) {
        any = true;
        section(ui, "Accounts");
        for (index, (name, host)) in ACCOUNT_HOSTS.iter().enumerate() {
            if !hosts_shown[index] {
                continue;
            }
            let users = view
                .accounts
                .as_ref()
                .map(|accounts| accounts.get(index).cloned().unwrap_or_default());
            let busy = view.account_busy.as_deref() == Some(*host);
            account_row(ui, name, host, users.as_deref(), busy, actions);
        }
        ui.add_space(4.0 + 2.0);
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new(
                    "Other servers sign in through Git Credential Manager the first time you push.",
                )
                .font(theme::small())
                .color(palette.text_tertiary),
            );
            if link(ui, "Manage saved credentials").clicked() {
                actions.push(Action::ManageCredentials);
            }
        });
    }

    // ----- identities
    let identity_words: Vec<&str> = ["Commit identities", "identity", "author", "email", "host"]
        .into_iter()
        .chain(snap.identities.iter().map(|saved| saved.label.as_str()))
        .collect();
    if shown(&identity_words) {
        any = true;
        section(ui, "Commit identities");
        identities_card(ui, &snap.identities, actions);
    }

    // ----- git
    let author_words = ["Commit author", "name", "email", "identity"];
    let fetch_words = [
        "Fetch in the background",
        "Keeps ahead/behind counts current. Never changes your files.",
    ];
    let pull_words = ["When pulling", "Rebase keeps history in a straight line", "merge"];
    let branch_words = ["Default branch name", "For repositories you create in Facet"];
    if [&author_words[..], &fetch_words, &pull_words, &branch_words]
        .iter()
        .any(|words| shown(words))
    {
        any = true;
        section(ui, "Git");
        if shown(&author_words) {
            let subtitle = match &view.identity {
                None => "Reading your Git config…".to_owned(),
                Some(identity) if identity.name.is_empty() && identity.email.is_empty() => {
                    "Not set: Git needs a name and email to commit".to_owned()
                }
                Some(identity) => {
                    let place = match identity.scope.as_str() {
                        "local" => "from this repository's Git config",
                        "global" => "from your global Git config",
                        "system" => "from the system Git config",
                        "worktree" => "from this worktree's Git config",
                        _ => "from your Git config",
                    };
                    format!("{} · {} — {place}", identity.name, identity.email)
                }
            };
            let can_edit = snap.gitignore_repo.is_some();
            setting_row(ui, &vector::PERSON, author_words[0], &subtitle, |ui, right, y| {
                let response = row_button(
                    ui,
                    Id::unique("settings-author"),
                    right,
                    y,
                    "Edit",
                    false,
                    can_edit,
                );
                let response = if can_edit {
                    response.on_hover_text("Set the author for this repository only")
                } else {
                    response.on_hover_text("Open a repository first")
                };
                if response.clicked() {
                    actions.push(Action::EditCommitAuthor);
                }
                response.rect.width()
            });
        }
        if shown(&fetch_words) {
            setting_row(ui, &vector::SYNC, fetch_words[0], fetch_words[1], |ui, right, y| {
                let on = snap.auto_fetch;
                let switch_rect =
                    Rect::from_center_size(Pos2::new(right - 20.0, y), Vec2::new(40.0, 20.0));
                if switch(ui, switch_rect, Id::unique("settings-fetch"), on).clicked() {
                    actions.push(Action::SetAutoFetch(!on));
                }
                let items: Vec<(String, bool, Action)> = FETCH_INTERVALS
                    .into_iter()
                    .map(|minutes| {
                        (
                            fetch_interval_label(minutes),
                            minutes == snap.fetch_minutes,
                            Action::SetFetchMinutes(minutes),
                        )
                    })
                    .collect();
                let width = dropdown(
                    ui,
                    Id::unique("settings-fetch-interval"),
                    switch_rect.left() - 16.0,
                    y,
                    160.0,
                    &fetch_interval_label(snap.fetch_minutes),
                    &items,
                    None,
                    actions,
                );
                width + 16.0 + 40.0
            });
        }
        if shown(&pull_words) {
            let subtitle = if snap.pull_mode == PullMode::FastForwardOnly {
                "Fast-forward only: Pull stops instead of merging diverged branches"
            } else {
                pull_words[1]
            };
            setting_row(ui, &vector::PULL, pull_words[0], subtitle, |ui, right, y| {
                let current = usize::from(snap.pull_mode == PullMode::Rebase);
                let (width, chosen) = segmented(
                    ui,
                    Id::unique("settings-pull"),
                    right,
                    y,
                    &["Merge", "Rebase"],
                    current,
                );
                match chosen {
                    Some(0) => actions.push(Action::SetPullMode(PullMode::Merge)),
                    Some(_) => actions.push(Action::SetPullMode(PullMode::Rebase)),
                    None => {}
                }
                width
            });
        }
        if shown(&branch_words) {
            setting_row(ui, &vector::BRANCH, branch_words[0], branch_words[1], |ui, right, y| {
                let rect = Rect::from_min_size(
                    Pos2::new(right - 160.0, y - CONTROL_HEIGHT / 2.0),
                    Vec2::new(160.0, CONTROL_HEIGHT),
                );
                let response = text_box(
                    ui,
                    rect,
                    Id::unique("settings-default-branch"),
                    &mut view.default_branch,
                    "main",
                );
                if response.lost_focus() {
                    actions.push(Action::SetDefaultBranch(view.default_branch.clone()));
                }
                160.0
            });
        }
    }

    // ----- tools
    let editor_words = ["External editor", "Opens files and unresolved conflicts"];
    let terminal_words = ["Terminal", "Opens in the repository folder"];
    let advanced_words = [
        "Advanced",
        "Git executable, LFS, line endings, diagnostics",
        "interface size",
        "diff tool",
        "merge tool",
        ".gitignore",
        "open in",
    ];
    if [&editor_words[..], &terminal_words, &advanced_words]
        .iter()
        .any(|words| shown(words))
    {
        any = true;
        section(ui, "Tools");
        if shown(&editor_words) {
            setting_row(ui, &vector::EDIT, editor_words[0], editor_words[1], |ui, right, y| {
                let items: Vec<(String, bool, Action)> = Editor::ALL
                    .into_iter()
                    .map(|editor| {
                        let found = match editor {
                            Editor::VsCode => snap.installed.vscode.is_some(),
                            Editor::Rider => snap.installed.rider.is_some(),
                            Editor::System => true,
                        };
                        let label = if found {
                            editor.label().to_owned()
                        } else {
                            format!("{} (not found)", editor.label())
                        };
                        (label, editor == snap.editor, Action::SetEditor(editor))
                    })
                    .collect();
                dropdown(
                    ui,
                    Id::unique("settings-editor"),
                    right,
                    y,
                    200.0,
                    snap.editor.label(),
                    &items,
                    None,
                    actions,
                )
            });
        }
        if shown(&terminal_words) {
            setting_row(
                ui,
                &vector::TERMINAL,
                terminal_words[0],
                terminal_words[1],
                |ui, right, y| {
                    let items: Vec<(String, bool, Action)> = Terminal::ALL
                        .into_iter()
                        .map(|terminal| {
                            let label = if terminal == Terminal::WindowsTerminal
                                && snap.installed.terminal.is_none()
                            {
                                format!("{} (not found)", terminal.label())
                            } else {
                                terminal.label().to_owned()
                            };
                            (label, terminal == snap.terminal, Action::SetTerminal(terminal))
                        })
                        .collect();
                    dropdown(
                        ui,
                        Id::unique("settings-terminal"),
                        right,
                        y,
                        200.0,
                        snap.terminal.label(),
                        &items,
                        None,
                        actions,
                    )
                },
            );
        }
        if shown(&advanced_words) && link_row(ui, &vector::SLIDERS, advanced_words[0], advanced_words[1]) {
            actions.push(Action::ShowAdvancedSettings);
        }
    }

    if !any {
        ui.add_space(space::M);
        ui.label(
            RichText::new(format!("No settings match “{}”.", view.find.trim()))
                .font(theme::body())
                .color(palette.text_secondary),
        );
    }
}

fn advanced_page(
    ui: &mut Ui,
    snap: &Snapshot,
    view: &mut SettingsView,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    header(ui, "Advanced", Some("Settings"), None, actions);

    // ----- git
    section(ui, "Git");
    stacked_card(ui, &vector::GIT, "Git executable", &snap.git, |ui| {
        git_path_field(ui, snap, view, actions);
    });
    ui.add_space(ROW_GAP);
    let lfs = match &view.lfs {
        None => "Checking…".to_owned(),
        Some(Some(version)) => {
            // `git-lfs/3.5.1 (GitHub; windows amd64; …)`: just the number.
            let short = version
                .split_whitespace()
                .next()
                .and_then(|word| word.strip_prefix("git-lfs/"))
                .unwrap_or(version);
            format!("Git LFS {short} is installed")
        }
        Some(None) => "Not installed: large files are stored in the repository itself".to_owned(),
    };
    setting_row(ui, &vector::STORAGE, "Large files (Git LFS)", &lfs, |_, _, _| 0.0);
    let has_work = snap.gitignore_repo.is_some();
    setting_row(
        ui,
        &vector::LINE_ENDINGS,
        "Line endings",
        if has_work {
            "How this repository converts CRLF and LF (core.autocrlf, this repository only)"
        } else {
            "Open a repository to change how it converts CRLF and LF"
        },
        |ui, right, y| {
            let Some(current) = view.line_endings.filter(|_| has_work) else {
                return 0.0;
            };
            let modes = [LineEndings::AsIs, LineEndings::Windows, LineEndings::Input];
            let index = modes.iter().position(|mode| *mode == current).unwrap_or(0);
            let (width, chosen) = segmented(
                ui,
                Id::unique("settings-line-endings"),
                right,
                y,
                &["As is", "CRLF on checkout", "LF on commit"],
                index,
            );
            if let Some(chosen) = chosen {
                actions.push(Action::SetLineEndings(modes[chosen]));
            }
            width
        },
    );

    // ----- interface
    section(ui, "Interface");
    setting_row(
        ui,
        &vector::MONITOR,
        "Interface size",
        "Ctrl+= and Ctrl+- change it too; Ctrl+0 goes back to 100%",
        |ui, right, y| {
            let labels: Vec<String> = UI_SCALES
                .iter()
                .map(|scale| format!("{:.0}%", scale * 100.0))
                .collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let current = UI_SCALES
                .iter()
                .position(|scale| (scale - snap.ui_scale).abs() < 0.001)
                .unwrap_or(usize::MAX);
            let (width, chosen) =
                segmented(ui, Id::unique("settings-scale"), right, y, &labels, current);
            if let Some(index) = chosen {
                actions.push(Action::SetUiScale(UI_SCALES[index]));
            }
            width
        },
    );

    // ----- external tools
    section(ui, "Diff and merge tools");
    card(ui, |ui| {
        note(
            ui,
            "Chosen by file extension; the first matching tool wins. Without one, diffs and \
             conflicts open in Facet's own views.",
            palette.text_secondary,
        );
        ui.add_space(space::S);
        for (index, (name, command)) in snap.tools.iter().enumerate() {
            divider(ui);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::hover());
            let text_width = rect.width() - 40.0;
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
        if !snap.tools.is_empty() {
            divider(ui);
        }
        ui.add_space(space::S);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(space::S);
            if button(
                ui,
                widgets::ButtonKind::Secondary,
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
                if button(ui, widgets::ButtonKind::Secondary, None, preset.label(), &tooltip)
                    .clicked()
                {
                    actions.push(Action::AddExternalToolPreset(preset));
                }
            }
        });
    });

    // ----- open in
    section(ui, "Open in");
    for target in OpenTarget::ALL {
        let found = snap.installed.launcher(target);
        let status = found.map_or_else(|| "Not found".to_owned(), |path| path.display().to_string());
        let glyph = match target {
            OpenTarget::Terminal => &vector::TERMINAL,
            OpenTarget::VsCode | OpenTarget::Rider => &vector::EDIT,
        };
        setting_row(ui, glyph, target.label(), &status, |ui, right, y| {
            if found.is_none() {
                return 0.0;
            }
            let response = row_button(
                ui,
                Id::unique(("settings-open-in", target.label())),
                right,
                y,
                "Open",
                false,
                snap.has_repo,
            );
            if response.clicked() {
                actions.push(Action::OpenIn(target));
            }
            response.rect.width()
        });
    }
    setting_row(ui, &vector::FOLDER, "File Explorer", "Always available", |ui, right, y| {
        let response = row_button(
            ui,
            Id::unique("settings-open-explorer"),
            right,
            y,
            "Open",
            false,
            snap.has_repo,
        );
        let tooltip = widgets::tooltip_with_shortcut(
            ui,
            "Open the repository in File Explorer",
            &shortcut::REVEAL,
        );
        if response.on_hover_text(tooltip).clicked() {
            actions.push(Action::RevealInExplorer);
        }
        CONTROL_HEIGHT * 2.0
    });

    // ----- .gitignore templates
    let response = section(ui, ".gitignore templates");
    if view.reveal_gitignore {
        view.reveal_gitignore = false;
        response.scroll_to_me(Some(Align::Min));
    }
    card(ui, |ui| match &snap.gitignore_repo {
        None => note(
            ui,
            "Open a repository with a working folder to add an engine's rules to its .gitignore.",
            palette.text_secondary,
        ),
        Some(repo) => {
            note(
                ui,
                &format!(
                    "Adds the rules {repo}'s .gitignore doesn't have yet, under a comment. Undo \
                     puts the file back."
                ),
                palette.text_secondary,
            );
            ui.add_space(space::S + 2.0);
            let labels = Template::ALL.map(Template::label);
            let current = Template::ALL
                .iter()
                .position(|template| *template == view.template)
                .unwrap_or(0);
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), SEGMENT_HEIGHT + 6.0),
                Sense::hover(),
            );
            let width = segment_width(ui, &labels);
            let (_, chosen) = segmented(
                ui,
                Id::unique("settings-gitignore"),
                rect.left() + width,
                rect.center().y,
                &labels,
                current,
            );
            if let Some(index) = chosen {
                actions.push(Action::ShowGitignoreTemplate(Template::ALL[index]));
            }
            ui.add_space(space::S + 2.0);
            gitignore_preview(ui, view);
            ui.add_space(space::S + 2.0);
            let ready = matches!(&view.preview, GitignorePreview::Ready(block) if !block.is_empty());
            if button_enabled(
                ui,
                widgets::ButtonKind::Primary,
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
    });

    // ----- diagnostics
    section(ui, "Diagnostics");
    setting_row(
        ui,
        &vector::PULSE,
        "Diagnostics",
        "Versions and paths to paste into a bug report; the command log shows every git command",
        |ui, right, y| {
            let copy = row_button(
                ui,
                Id::unique("settings-copy-diagnostics"),
                right,
                y,
                "Copy",
                false,
                true,
            );
            if copy.clicked() {
                actions.push(Action::CopyDiagnostics);
            }
            let log = row_button(
                ui,
                Id::unique("settings-command-log"),
                copy.rect.left() - space::S,
                y,
                "Command Log",
                false,
                true,
            );
            if log.clicked() {
                actions.push(Action::ToggleCommandLog);
            }
            copy.rect.width() + space::S + log.rect.width()
        },
    );
}

// ----- building blocks --------------------------------------------------------------------

/// The page title, with a "Find a setting" box on the main page and a way back on others.
fn header(
    ui: &mut Ui,
    title: &str,
    parent: Option<&str>,
    find: Option<&mut String>,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
    let mut x = rect.left();
    if let Some(parent) = parent {
        let back = Rect::from_min_size(
            Pos2::new(x - 6.0, rect.center().y - 16.0),
            Vec2::splat(32.0),
        );
        if icon_button_at(ui, back, Id::unique("settings-back"), icon::BACK, "Back (Esc)").clicked() {
            actions.push(Action::ShowSettings);
        }
        x = back.right() + 6.0;
        let galley =
            ui.painter()
                .layout_no_wrap(parent.to_owned(), theme::title(), palette.text_tertiary);
        let parent_width = galley.size().x;
        let parent_rect = Rect::from_min_size(
            Pos2::new(x, rect.center().y - galley.size().y / 2.0),
            galley.size(),
        );
        let response = ui.interact(parent_rect, Id::unique("settings-parent"), Sense::click());
        let color = if response.hovered() {
            palette.text_secondary
        } else {
            palette.text_tertiary
        };
        ui.painter().galley(parent_rect.min, galley, color);
        if response.clicked() {
            actions.push(Action::ShowSettings);
        }
        x += parent_width + 12.0;
        ui.painter().text(
            Pos2::new(x, rect.center().y + 2.0),
            Align2::LEFT_CENTER,
            icon::CHEVRON_RIGHT,
            theme::icon_font(16.0),
            palette.text_tertiary,
        );
        x += 16.0 + 12.0;
    }
    ui.painter().text(
        Pos2::new(x, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::title(),
        palette.text,
    );
    if let Some(find) = find {
        let field = Rect::from_min_size(
            Pos2::new(rect.right() - FIND_WIDTH, rect.center().y - 16.0),
            Vec2::new(FIND_WIDTH, 32.0),
        );
        let id = Id::unique("settings-find");
        let response = widgets::search_field(ui, field, id, find, "Find a setting", 8.0);
        if response.has_focus() && ui.input(|input| input.key_pressed(Key::Escape)) {
            find.clear();
            response.surrender_focus();
        }
        if ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::F)) {
            response.request_focus();
        }
    }
}

/// A section's title above its rows.
fn section(ui: &mut Ui, text: &str) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    ui.add_space(SECTION_GAP);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.left() + 2.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        semibold(14.0),
        palette.text,
    );
    ui.add_space(6.0);
    response
}

fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, theme::strong().family)
}

/// The card behind one row.
fn row_card(ui: &Ui, rect: Rect, hovered: bool) {
    let palette = Palette::of(ui.ctx());
    let fill = if hovered {
        widgets::mix(palette.raised, palette.text, if palette.dark { 0.03 } else { 0.015 })
    } else {
        palette.raised
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::LAYER),
        fill,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
}

/// The icon, name and explanation of a row; returns where the text starts.
fn row_text(ui: &Ui, rect: Rect, glyph: &Icon, title: &str, subtitle: &str, text_right: f32) {
    let palette = Palette::of(ui.ctx());
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + ROW_PAD + 10.0, rect.center().y),
        Vec2::splat(20.0),
    );
    vector::draw(ui.painter(), icon_rect, glyph, 1.6, palette.text_secondary);
    let left = icon_rect.right() + 16.0;
    let width = (text_right - left).max(40.0);
    if subtitle.is_empty() {
        widgets::text_line(
            ui,
            Pos2::new(left, rect.center().y),
            title,
            theme::large(),
            palette.text,
            width,
        );
    } else {
        widgets::text_line(
            ui,
            Pos2::new(left, rect.center().y - 9.0),
            title,
            theme::large(),
            palette.text,
            width,
        );
        widgets::text_line(
            ui,
            Pos2::new(left, rect.center().y + 10.0),
            subtitle,
            theme::small(),
            palette.text_tertiary,
            width,
        );
    }
}

/// One setting as its own card: icon, name, explanation, and `control` drawn right-aligned
/// at (`right`, `center y`), returning how wide it is.
fn setting_row(
    ui: &mut Ui,
    glyph: &Icon,
    title: &str,
    subtitle: &str,
    control: impl FnOnce(&mut Ui, f32, f32) -> f32,
) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ROW_HEIGHT),
        Sense::hover(),
    );
    row_card(ui, rect, response.hovered());
    let used = control(ui, rect.right() - ROW_PAD, rect.center().y);
    let text_right = rect.right() - ROW_PAD - if used > 0.0 { used + 16.0 } else { 0.0 };
    row_text(ui, rect, glyph, title, subtitle, text_right);
    ui.add_space(ROW_GAP);
}

/// A row that opens a page: the whole card is the button, with a chevron.
fn link_row(ui: &mut Ui, glyph: &Icon, title: &str, subtitle: &str) -> bool {
    let palette = Palette::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ROW_HEIGHT),
        Sense::click(),
    );
    row_card(ui, rect, response.hovered());
    ui.painter().text(
        Pos2::new(rect.right() - ROW_PAD - 8.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon::CHEVRON_RIGHT,
        theme::icon_font(14.0),
        palette.text_secondary,
    );
    row_text(ui, rect, glyph, title, subtitle, rect.right() - ROW_PAD - 32.0);
    widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);
    ui.add_space(ROW_GAP);
    response.clicked()
}

/// A card with a row's icon and name, and a control that needs the card's width under them.
fn stacked_card(
    ui: &mut Ui,
    glyph: &Icon,
    title: &str,
    subtitle: &str,
    add_contents: impl FnOnce(&mut Ui),
) {
    card(ui, |ui| {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
        let rect = Rect::from_min_max(
            Pos2::new(rect.left() - ROW_PAD, rect.top()),
            Pos2::new(rect.right() + ROW_PAD, rect.bottom()),
        );
        row_text(ui, rect, glyph, title, subtitle, rect.right() - ROW_PAD);
        ui.add_space(space::M - 4.0);
        add_contents(ui);
    });
}

/// A card holding free-form content (padding 16 / 18).
/// The saved commit identities: one row each with Edit and Remove, then "Add Identity…".
fn identities_card(
    ui: &mut Ui,
    identities: &[crate::settings::SavedIdentity],
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    card(ui, |ui| {
        note(
            ui,
            "Pick one per repository from the line above the commit message in Changes; it is \
             written to that repository's own Git config. Give an identity the hosts it is for, \
             and Facet warns when a repository there commits as someone else.",
            palette.text_secondary,
        );
        ui.add_space(space::S);
        for (index, saved) in identities.iter().enumerate() {
            divider(ui);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::hover());
            let text_width = rect.width() - 76.0;
            widgets::text_line(
                ui,
                Pos2::new(rect.left(), rect.top() + 16.0),
                &saved.label,
                theme::strong(),
                palette.text,
                text_width,
            );
            let mut detail = format!("{} <{}>", saved.name, saved.email);
            if !saved.hosts.is_empty() {
                detail.push_str(&format!("  ·  {}", saved.hosts.join(", ")));
            }
            widgets::text_line(
                ui,
                Pos2::new(rect.left(), rect.bottom() - 16.0),
                &detail,
                theme::small(),
                palette.text_tertiary,
                text_width,
            );
            let remove = Rect::from_center_size(
                Pos2::new(rect.right() - 14.0, rect.center().y),
                Vec2::splat(28.0),
            );
            let edit = remove.translate(Vec2::new(-32.0, 0.0));
            if icon_button_at(
                ui,
                edit,
                Id::unique(("settings-edit-identity", index)),
                icon::EDIT,
                "Edit this identity",
            )
            .clicked()
            {
                actions.push(Action::EditIdentity(Some(index)));
            }
            if icon_button_at(
                ui,
                remove,
                Id::unique(("settings-remove-identity", index)),
                icon::DELETE,
                "Remove this identity (repositories that use it keep their author)",
            )
            .clicked()
            {
                actions.push(Action::RemoveIdentity(index));
            }
        }
        if !identities.is_empty() {
            divider(ui);
        }
        ui.add_space(space::S);
        if button(
            ui,
            widgets::ButtonKind::Secondary,
            Some(icon::ADD),
            "Add Identity…",
            "Save a name and email to commit as, e.g. your GitHub profile",
        )
        .clicked()
        {
            actions.push(Action::EditIdentity(None));
        }
    });
}

fn card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    Frame::NONE
        .fill(palette.raised)
        .stroke(palette.hairline_stroke())
        .corner_radius(CornerRadius::same(radius::LAYER))
        .inner_margin(Margin {
            left: ROW_PAD as i8,
            right: ROW_PAD as i8,
            top: 16,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add_contents(ui);
        });
    ui.add_space(ROW_GAP);
}

/// A hairline between two parts of a card.
fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    widgets::hairline_h(ui, rect.left()..=rect.right(), rect.center().y);
}

fn note(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(Label::new(RichText::new(text).font(theme::small()).color(color)).wrap());
}

fn link(ui: &mut Ui, text: &str) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.add(
        Label::new(
            RichText::new(text)
                .font(theme::small())
                .color(palette.accent),
        )
        .sense(Sense::click()),
    );
    if response.hovered() {
        let rect = response.rect;
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.bottom() - 1.0,
            Stroke::new(1.0, palette.accent),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The Windows toggle switch: 40×20, accent when on, an outline with a dot when off.
fn switch(ui: &Ui, rect: Rect, id: Id, on: bool) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, id, Sense::click());
    switch_visual(ui, rect, id, on, response.hovered(), true);
    widgets::focus_ring(ui, &response, rect.expand(2.0), 12, palette);
    response
}

/// Paint a toggle switch of any size (the knob scales with its height).
pub(super) fn switch_visual(ui: &Ui, rect: Rect, id: Id, on: bool, hovered: bool, enabled: bool) {
    let palette = Palette::of(ui.ctx());
    let hovered = hovered && enabled;
    let t = ui.ctx().animate_bool_with_time_and_easing(
        id.with("anim"),
        on,
        theme::motion::FAST,
        theme::motion::ease_out,
    );
    let half = rect.height() / 2.0;
    let corner = CornerRadius::same(half.round() as u8);
    let off_color = if enabled {
        palette.text_secondary
    } else {
        palette.text_tertiary
    };
    if t > 0.0 {
        let fill = if hovered {
            widgets::mix(palette.accent, Color32::WHITE, 0.1)
        } else {
            palette.accent
        };
        ui.painter()
            .rect_filled(rect, corner, theme::fade(fill, t));
    }
    if t < 1.0 {
        let fill = if hovered {
            palette.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect(
            rect,
            corner,
            fill,
            Stroke::new(
                if half < 9.0 { 1.5 } else { 1.0 },
                theme::fade(off_color, 1.0 - t),
            ),
            StrokeKind::Inside,
        );
    }
    let knob_x = egui::lerp((rect.left() + half)..=(rect.right() - half), t);
    let knob = widgets::mix(off_color, palette.on_accent, t);
    let size = half * if hovered { 0.7 } else { 0.6 };
    ui.painter()
        .circle_filled(Pos2::new(knob_x, rect.center().y), size, knob);
}

/// Width of [`segmented`] for these labels.
pub(super) fn segment_width(ui: &Ui, labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap((*label).to_owned(), semibold(12.0), Color32::WHITE)
                .size()
                .x
                + 28.0
        })
        .sum::<f32>()
        + 4.0
}

/// The design's segmented control, right-aligned at `right`: the chosen segment is raised.
/// Returns its width and the segment clicked (if not already chosen).
pub(super) fn segmented(
    ui: &Ui,
    id: Id,
    right: f32,
    center_y: f32,
    labels: &[&str],
    current: usize,
) -> (f32, Option<usize>) {
    let palette = Palette::of(ui.ctx());
    let total = segment_width(ui, labels);
    let rect = Rect::from_min_size(
        Pos2::new(right - total, center_y - SEGMENT_HEIGHT / 2.0 - 2.0),
        Vec2::new(total, SEGMENT_HEIGHT + 4.0),
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(radius::LAYER),
        palette.chrome,
        palette.hairline_stroke(),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 2.0;
    let mut chosen = None;
    for (index, label) in labels.iter().enumerate() {
        let active = index == current;
        let font = if active {
            semibold(12.0)
        } else {
            theme::small()
        };
        let width = ui
            .painter()
            .layout_no_wrap((*label).to_owned(), semibold(12.0), Color32::WHITE)
            .size()
            .x
            + 28.0;
        let segment = Rect::from_min_size(
            Pos2::new(x, rect.top() + 2.0),
            Vec2::new(width, SEGMENT_HEIGHT),
        );
        let response = ui.interact(segment, id.with(index), Sense::click());
        let corner = CornerRadius::same(radius::M);
        if active {
            ui.painter().add(Shadow {
                offset: [0, 1],
                blur: 2,
                spread: 0,
                color: Color32::from_black_alpha(if palette.dark { 60 } else { 30 }),
            }
            .as_shape(segment, corner));
            ui.painter().rect_filled(segment, corner, palette.control);
        } else if response.hovered() {
            ui.painter().rect_filled(segment, corner, palette.hover);
        }
        ui.painter().text(
            segment.center(),
            Align2::CENTER_CENTER,
            *label,
            font,
            if active {
                palette.text
            } else {
                palette.text_secondary
            },
        );
        widgets::focus_ring(ui, &response, segment, radius::M, palette);
        if response.clicked() && !active {
            chosen = Some(index);
        }
        x += width;
    }
    (total, chosen)
}

/// A button in a row, right-aligned at `right`: outline (`Sign out`, `Edit`) or accent.
fn row_button(
    ui: &Ui,
    id: Id,
    right: f32,
    center_y: f32,
    label: &str,
    primary: bool,
    enabled: bool,
) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let font = if primary {
        theme::strong()
    } else {
        theme::body()
    };
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let rect = Rect::from_min_size(
        Pos2::new(right - text_width - 28.0, center_y - CONTROL_HEIGHT / 2.0),
        Vec2::new(text_width + 28.0, CONTROL_HEIGHT),
    );
    if primary {
        return widgets::primary_button_at(ui, rect, id, None, label, enabled);
    }
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let corner = CornerRadius::same(radius::LAYER);
    let fill = if enabled && response.hovered() {
        widgets::mix(palette.control, palette.text, 0.05)
    } else {
        palette.control
    };
    ui.painter()
        .rect(rect, corner, fill, palette.hairline_stroke(), StrokeKind::Inside);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        if enabled {
            palette.text
        } else {
            palette.text_tertiary
        },
    );
    widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);
    response
}

/// A dropdown button, right-aligned at `right`, listing `items` (label, chosen, action); a
/// separator goes before item `separator`. Returns its width.
#[allow(clippy::too_many_arguments)]
fn dropdown(
    ui: &Ui,
    id: Id,
    right: f32,
    center_y: f32,
    min_width: f32,
    label: &str,
    items: &[(String, bool, Action)],
    separator: Option<usize>,
    actions: &mut Vec<Action>,
) -> f32 {
    let palette = Palette::of(ui.ctx());
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::body(), Color32::WHITE)
        .size()
        .x;
    let width = (12.0 + text_width + 10.0 + 14.0 + 10.0).max(min_width);
    let rect = Rect::from_min_size(
        Pos2::new(right - width, center_y - CONTROL_HEIGHT / 2.0),
        Vec2::new(width, CONTROL_HEIGHT),
    );
    let response = ui.interact(rect, id, Sense::click());
    let corner = CornerRadius::same(radius::LAYER);
    let fill = if response.hovered() {
        widgets::mix(palette.control, palette.text, 0.05)
    } else {
        palette.control
    };
    ui.painter()
        .rect(rect, corner, fill, palette.hairline_stroke(), StrokeKind::Inside);
    widgets::text_line(
        ui,
        Pos2::new(rect.left() + 12.0, rect.center().y),
        label,
        theme::body(),
        palette.text,
        width - 12.0 - 34.0,
    );
    ui.painter().text(
        Pos2::new(rect.right() - 17.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon::CHEVRON_DOWN,
        theme::icon_font(11.0),
        palette.text_secondary,
    );
    widgets::focus_ring(ui, &response, rect, radius::LAYER, palette);

    let menu_width = width.max(200.0);
    egui::Popup::menu(&response)
        .id(egui::Popup::default_response_id(&response))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(space::XS)
        .width(menu_width)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(menu_width - 12.0);
            for (index, (label, chosen, action)) in items.iter().enumerate() {
                if separator == Some(index) {
                    ui.add_space(4.0);
                    divider(ui);
                    ui.add_space(4.0);
                }
                let (rect, item) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 32.0),
                    Sense::click(),
                );
                let fill = widgets::interaction_fill(ui, &item, palette);
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(radius::M), fill);
                if *chosen {
                    ui.painter().rect_filled(
                        Rect::from_center_size(
                            Pos2::new(rect.left() + 1.5, rect.center().y),
                            Vec2::new(3.0, 16.0),
                        ),
                        CornerRadius::same(2),
                        palette.accent,
                    );
                }
                widgets::text_line(
                    ui,
                    Pos2::new(rect.left() + 12.0, rect.center().y),
                    label,
                    if *chosen {
                        theme::strong()
                    } else {
                        theme::body()
                    },
                    palette.text,
                    rect.width() - 40.0,
                );
                if *chosen {
                    ui.painter().text(
                        Pos2::new(rect.right() - 14.0, rect.center().y),
                        Align2::CENTER_CENTER,
                        icon::CHECK,
                        theme::icon_font(12.0),
                        palette.accent,
                    );
                }
                if item.clicked() {
                    if !*chosen {
                        actions.push(action.clone());
                    }
                    ui.close();
                }
            }
        });
    width
}

/// A one-line text box in a row (`Default branch name`).
fn text_box(ui: &mut Ui, rect: Rect, id: Id, text: &mut String, hint: &str) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let focused = ui.ctx().memory(|memory| memory.has_focus(id));
    let corner = CornerRadius::same(radius::LAYER);
    ui.painter()
        .rect(rect, corner, palette.control, palette.hairline_stroke(), StrokeKind::Inside);
    if focused {
        ui.painter().rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left() + 1.0, rect.bottom() - 2.0),
                Pos2::new(rect.right() - 1.0, rect.bottom()),
            ),
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius::LAYER,
                se: radius::LAYER,
            },
            palette.accent,
        );
    }
    ui.put(
        rect,
        TextEdit::singleline(text)
            .id(id)
            .hint_text(hint)
            .font(theme::body())
            .text_color(palette.text)
            .frame(Frame::NONE.inner_margin(Margin::symmetric(12, 0)))
            .desired_width(rect.width())
            .align(Align2::LEFT_CENTER),
    )
}

/// GitHub / GitLab: signed in (with the account) and Sign out, or Sign in.
fn account_row(
    ui: &mut Ui,
    name: &str,
    host: &str,
    users: Option<&[String]>,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let signed_in = users.is_some_and(|users| !users.is_empty());
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ROW_HEIGHT),
        Sense::hover(),
    );
    row_card(ui, rect, response.hovered());
    // The service's mark in a 32 pt circle: solid when signed in.
    let center = Pos2::new(rect.left() + ROW_PAD + 16.0, rect.center().y);
    let (fill, mark) = if signed_in {
        (palette.text, palette.chrome)
    } else {
        (palette.control, palette.text_secondary)
    };
    ui.painter().circle_filled(center, 16.0, fill);
    if !signed_in {
        ui.painter()
            .circle_stroke(center, 15.5, palette.hairline_stroke());
    }
    let (glyph, size) = if name == "GitHub" {
        (&vector::GITHUB, 18.0)
    } else {
        (&vector::GITLAB, 17.0)
    };
    vector::draw(
        ui.painter(),
        Rect::from_center_size(center, Vec2::splat(size)),
        glyph,
        1.8,
        mark,
    );
    let subtitle = match users {
        None => "Checking…".to_owned(),
        Some([]) => "Not connected".to_owned(),
        Some([user]) => format!("Signed in as {user} · used for cloning, pushing and pull requests"),
        Some([user, rest @ ..]) => format!(
            "Signed in as {user} and {} more · used for cloning, pushing and pull requests",
            rest.len()
        ),
    };
    let right = rect.right() - ROW_PAD;
    let button = if busy {
        let label = if signed_in { "Signing out…" } else { "Signing in…" };
        row_button(ui, Id::unique(("settings-account", host)), right, rect.center().y, label, false, false)
    } else if signed_in {
        let response = row_button(
            ui,
            Id::unique(("settings-account", host)),
            right,
            rect.center().y,
            "Sign out",
            false,
            true,
        )
        .on_hover_text(format!("Make Git Credential Manager forget your {name} sign-in"));
        if response.clicked() {
            actions.push(Action::SignOut(host.to_owned()));
        }
        response
    } else {
        let response = row_button(
            ui,
            Id::unique(("settings-account", host)),
            right,
            rect.center().y,
            "Sign in",
            true,
            users.is_some(),
        )
        .on_hover_text(format!("Sign in to {host} with Git Credential Manager"));
        if response.clicked() {
            actions.push(Action::SignIn(host.to_owned()));
        }
        response
    };
    // Text after the mark (same column as the icons' text).
    let left = center.x + 16.0 + 16.0 - 2.0;
    let width = button.rect.left() - 16.0 - left;
    widgets::text_line(
        ui,
        Pos2::new(left, rect.center().y - 9.0),
        name,
        theme::large(),
        palette.text,
        width,
    );
    widgets::text_line(
        ui,
        Pos2::new(left, rect.center().y + 10.0),
        &subtitle,
        theme::small(),
        palette.text_tertiary,
        width,
    );
    ui.add_space(ROW_GAP);
}

/// Light / Dark / Match Windows as three small window previews, in a card with its title.
fn theme_card(ui: &mut Ui, current: ThemeMode, actions: &mut Vec<Action>) {
    let palette = Palette::of(ui.ctx());
    card(ui, |ui| {
        ui.label(
            RichText::new("Theme")
                .font(theme::large())
                .color(palette.text),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new("Facet can follow Windows, or stay light or dark.")
                .font(theme::small())
                .color(palette.text_tertiary),
        );
        ui.add_space(14.0);
        let gap = 12.0;
        let width = ui.available_width();
        let tile = (width - gap * 2.0) / 3.0;
        let (row, _) = ui.allocate_exact_size(Vec2::new(width, 8.0 + 84.0 + 10.0 + 20.0 + 10.0), Sense::hover());
        for (index, mode) in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System]
            .into_iter()
            .enumerate()
        {
            let rect = Rect::from_min_size(
                Pos2::new(row.left() + index as f32 * (tile + gap), row.top()),
                Vec2::new(tile, row.height()),
            );
            let response = ui.interact(rect, Id::unique(("settings-theme", index)), Sense::click());
            let selected = mode == current;
            let corner = CornerRadius::same(radius::L);
            if selected {
                ui.painter().rect(
                    rect,
                    corner,
                    palette.accent_soft,
                    Stroke::new(2.0, palette.accent),
                    StrokeKind::Inside,
                );
            } else {
                let fill = if response.hovered() {
                    palette.hover
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter()
                    .rect(rect, corner, fill, palette.hairline_stroke(), StrokeKind::Inside);
            }
            let preview = Rect::from_min_size(
                rect.min + Vec2::new(8.0, 8.0),
                Vec2::new(rect.width() - 16.0, 84.0),
            );
            theme_preview(ui, preview, mode);
            // Radio and label.
            let y = preview.bottom() + 10.0 + 10.0;
            let dot = Pos2::new(preview.left() + 4.0 + 8.0, y);
            if selected {
                ui.painter().circle_filled(dot, 8.0, palette.accent);
                ui.painter().circle_filled(dot, 3.0, palette.on_accent);
            } else {
                ui.painter()
                    .circle_stroke(dot, 7.25, Stroke::new(1.5, palette.text_tertiary));
            }
            let label = match mode {
                ThemeMode::Light => "Light",
                ThemeMode::Dark => "Dark",
                ThemeMode::System => "Match Windows",
            };
            ui.painter().text(
                Pos2::new(dot.x + 8.0 + 8.0, y),
                Align2::LEFT_CENTER,
                label,
                if selected {
                    theme::strong()
                } else {
                    theme::body()
                },
                palette.text,
            );
            widgets::focus_ring(ui, &response, rect, radius::L, palette);
            if response.clicked() && !selected {
                actions.push(Action::SetTheme(mode));
            }
        }
    });
}

/// The little window drawn on a theme tile (the design's exact shapes and colors).
fn theme_preview(ui: &Ui, preview: Rect, mode: ThemeMode) {
    struct Look {
        backdrop: Color32,
        border: Color32,
        accent: Color32,
        bar: Color32,
        panel: Color32,
        panel_line: Color32,
        added: Color32,
        removed: Color32,
    }
    let light = Look {
        backdrop: Color32::from_rgb(0xF3, 0xF3, 0xF3),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        accent: Color32::from_rgb(0x00, 0x67, 0xC0),
        bar: Color32::from_rgb(0xD0, 0xD0, 0xD0),
        panel: Color32::WHITE,
        panel_line: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        added: Color32::from_rgb(0xDC, 0xF0, 0xE3),
        removed: Color32::from_rgb(0xFA, 0xDF, 0xDB),
    };
    let dark = Look {
        backdrop: Color32::from_rgb(0x20, 0x20, 0x20),
        border: Color32::from_rgb(0x3A, 0x3A, 0x3A),
        accent: Color32::from_rgb(0x60, 0xCD, 0xFF),
        bar: Color32::from_rgb(0x44, 0x44, 0x44),
        panel: Color32::from_rgb(0x1C, 0x1C, 0x1C),
        panel_line: Color32::from_rgb(0x32, 0x32, 0x32),
        added: Color32::from_rgb(0x1E, 0x4A, 0x2B),
        removed: Color32::from_rgb(0x52, 0x25, 0x23),
    };
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter_at(preview);
    let corner = CornerRadius::same(radius::M);
    let bar = |painter: &egui::Painter, x: f32, y: f32, w: f32, h: f32, color: Color32| {
        painter.rect_filled(
            Rect::from_min_size(preview.min + Vec2::new(x, y), Vec2::new(w, h)),
            CornerRadius::same(if h > 4.5 { 3 } else { 2 }),
            color,
        );
    };
    let window = |painter: &egui::Painter, look: &Look, full: bool| {
        painter.rect_filled(preview, corner, look.backdrop);
        bar(painter, 8.0, 8.0, 22.0, 5.0, look.accent);
        bar(painter, 8.0, 20.0, 26.0, 4.0, look.bar);
        if full {
            bar(painter, 8.0, 29.0, 22.0, 4.0, look.bar);
        }
        let panel = Rect::from_min_max(preview.min + Vec2::new(42.0, 16.0), preview.max);
        painter.rect(
            panel,
            CornerRadius {
                nw: 5,
                ..CornerRadius::ZERO
            },
            look.panel,
            Stroke::new(1.0, look.panel_line),
            StrokeKind::Outside,
        );
    };
    let lines = |painter: &egui::Painter, look: &Look, left: f32, width: f32, a: f32, b: f32| {
        bar(painter, left, 28.0, width * a, 4.0, look.added);
        bar(painter, left, 37.0, width * b, 4.0, look.removed);
    };
    match mode {
        ThemeMode::Light | ThemeMode::Dark => {
            let look = if mode == ThemeMode::Light { &light } else { &dark };
            window(&painter, look, true);
            let panel_width = preview.width() - 42.0;
            lines(&painter, look, 52.0, panel_width, 0.5, 0.4);
            painter.rect_stroke(preview, corner, Stroke::new(1.0, look.border), StrokeKind::Inside);
        }
        ThemeMode::System => {
            let half = preview.width() / 2.0;
            let left = painter.with_clip_rect(Rect::from_min_size(
                preview.min,
                Vec2::new(half, preview.height()),
            ));
            window(&left, &light, false);
            let right_rect = Rect::from_min_max(
                Pos2::new(preview.left() + half, preview.top()),
                preview.max,
            );
            let right = painter.with_clip_rect(right_rect);
            right.rect_filled(right_rect, corner, dark.panel);
            right.hline(
                right_rect.left()..=right_rect.right(),
                preview.top() + 16.5,
                Stroke::new(1.0, dark.panel_line),
            );
            lines(&right, &dark, half + 10.0, half, 0.6, 0.45);
            painter.rect_stroke(
                preview,
                corner,
                palette.hairline_stroke(),
                StrokeKind::Inside,
            );
        }
    }
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
            ui.spacing_mut().item_spacing.x = space::S;
            let buttons = 230.0;
            let width = (ui.available_width() - buttons).max(160.0);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(width, CONTROL_HEIGHT), Sense::hover());
            let field = text_box(
                ui,
                rect,
                Id::unique("settings-git-path"),
                &mut view.git_path,
                "git  (found on PATH)",
            );
            let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            if button(
                ui,
                widgets::ButtonKind::Secondary,
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
                widgets::ButtonKind::Primary,
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
            widgets::ButtonKind::Secondary,
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

fn gitignore_preview(ui: &mut Ui, view: &SettingsView) {
    let palette = Palette::of(ui.ctx());
    Frame::NONE
        .fill(palette.surface_alt)
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

/// `Facet 0.1.0 · build 1a2b3c4`, the build an installer was made from (`FACET_BUILD_ID`,
/// set by `scripts/build-installer.ps1`).
fn footer(ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(
            RichText::new(version_label())
                .font(theme::small())
                .color(palette.text_tertiary),
        );
    });
}

pub(crate) fn version_label() -> String {
    match option_env!("FACET_BUILD_ID") {
        Some(build) if !build.is_empty() => {
            format!("Facet {} · build {build}", env!("CARGO_PKG_VERSION"))
        }
        _ => format!("Facet {} · development build", env!("CARGO_PKG_VERSION")),
    }
}
