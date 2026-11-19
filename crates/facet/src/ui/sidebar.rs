//! The sidebar: the active repository's branches, remotes, tags and stashes, then the recent
//! repositories, and which git Facet drives. Sections collapse (remembered in the settings).
//!
//! The rows are one flat, fixed-height list drawn with `show_rows`, so a repository with
//! thousands of branches or tags costs only the visible rows.
//!
//! It paints no background of its own: the Mica backdrop shows through.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Pos2, Rect, ScrollArea, Sense, Ui, Vec2,
};
use facet_git::forge::ForgeKind;
use facet_git::parse::{GitRef, LfsLock, RefKind, Remote, StashEntry, Submodule, Track, Worktree};
use facet_git::same_dir;

use super::history::{forge_name, menu_item};
use super::region;
use super::widgets::{self, interaction_fill, text_line};
use crate::actions::{shortcut, Action};
use crate::app::{FacetApp, GitStatus, TabState};
use crate::app_trees::native_path;
use crate::app_work::ready_info;
use crate::history::View;
use crate::settings::same_path;
use crate::theme::{self, icon, radius, space, Palette};

const ROW: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 48.0;
/// The Changes / History / Settings items.
const NAV_ROW: f32 = 36.0;
/// Indent of rows under a section header (and of remote branches under their remote).
const INDENT: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Branches,
    Remotes,
    Tags,
    Stashes,
    Worktrees,
    Submodules,
    Locks,
    Recent,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::Branches => "Branches",
            Self::Remotes => "Remotes",
            Self::Tags => "Tags",
            Self::Stashes => "Stashes",
            Self::Worktrees => "Worktrees",
            Self::Submodules => "Submodules",
            Self::Locks => "Locks",
            Self::Recent => "Recent",
        }
    }

    /// The header button: glyph, tooltip, action.
    fn button(self) -> Option<(char, &'static str, Action)> {
        match self {
            Self::Branches => Some((icon::ADD, "New branch…", Action::NewBranch { at: None })),
            Self::Remotes => Some((icon::ADD, "Add remote…", Action::AddRemote)),
            Self::Tags => Some((icon::ADD, "New tag…", Action::NewTag { at: None })),
            Self::Stashes => Some((icon::ADD, "Stash changes…", Action::Stash)),
            Self::Worktrees => Some((
                icon::ADD,
                "Add worktree…",
                Action::AddWorktree { branch: None },
            )),
            Self::Submodules => Some((
                icon::REFRESH,
                "Update all submodules",
                Action::UpdateSubmodules(None),
            )),
            Self::Locks => Some((icon::REFRESH, "Refresh locks", Action::RefreshLocks)),
            Self::Recent => None,
        }
    }
}

enum Row<'a> {
    Header(Section, usize, bool),
    Branch(&'a GitRef),
    Remote(&'a Remote),
    RemoteBranch(&'a GitRef),
    Tag(&'a GitRef),
    Stash(usize, &'a StashEntry),
    /// A worktree, and whether it is the one this tab shows.
    Worktree(&'a Worktree, bool),
    Submodule(&'a Submodule),
    Lock(&'a LfsLock),
    Recent(&'a PathBuf),
    Hint(&'static str),
    Note(String),
}

impl FacetApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, rect: Rect) {
        let palette = Palette::of(ui.ctx());
        let mut actions = Vec::new();

        // Navigation: Changes and History of a working copy.
        let mut top = rect.top() + space::XS;
        let settings_open = self.settings_view.is_some();
        let work = self.active_tab().and_then(|tab| {
            ready_info(tab).map(|_| {
                let changed = tab.work.staged.len() + tab.work.unstaged.len();
                (tab.view, changed)
            })
        });
        if let Some((view, changed)) = work {
            for (target, glyph, label, tooltip, key, action) in [
                (
                    View::Changes,
                    icon::CHANGES,
                    "Changes",
                    "Show the working copy",
                    &shortcut::SHOW_CHANGES,
                    Action::ShowChanges,
                ),
                (
                    View::History,
                    icon::HISTORY,
                    "History",
                    "Show the commit history",
                    &shortcut::SHOW_HISTORY,
                    Action::ShowHistory,
                ),
            ] {
                let row = Rect::from_min_size(
                    Pos2::new(rect.left() + space::S, top),
                    Vec2::new(rect.width() - space::S * 2.0, NAV_ROW),
                );
                let badge = (target == View::Changes && changed > 0).then(|| changed.to_string());
                let tooltip = widgets::tooltip_with_shortcut(ui, tooltip, key);
                if nav_item(
                    ui,
                    row,
                    glyph,
                    label,
                    badge.as_deref(),
                    view == target && !settings_open,
                    palette,
                )
                .on_hover_text(tooltip)
                .clicked()
                {
                    actions.push(action);
                }
                top = row.bottom() + 2.0;
            }
            top += space::XS;
            widgets::hairline_h(ui, rect.left() + space::M..=rect.right() - space::M, top);
            top += space::XS;
        }

        let list = Rect::from_min_max(
            Pos2::new(rect.left() + space::S, top),
            Pos2::new(rect.right() - space::S, rect.bottom() - FOOTER_HEIGHT),
        );
        let rows = self.sidebar_rows();
        let forge = self.forge().map(|forge| forge.kind);
        let active_path = self.active_tab().map(|tab| tab.path.clone());
        // The stash shown in the stash view is highlighted like the current page.
        let shown_stash = self
            .active_tab()
            .filter(|tab| tab.view == View::Stash && !settings_open)
            .and_then(|tab| tab.stash.entry.as_ref())
            .map(|entry| entry.oid.clone());

        region(ui, "sidebar-list", list, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ScrollArea::vertical()
                .id_salt("sidebar-scroll")
                .auto_shrink(false)
                .show_rows(ui, ROW, rows.len(), |ui, range| {
                    let left = ui.max_rect().left();
                    let width = ui.max_rect().width();
                    let top = ui.max_rect().top() - range.start as f32 * ROW;
                    for index in range {
                        let rect = Rect::from_min_size(
                            Pos2::new(left, top + index as f32 * ROW),
                            Vec2::new(width, ROW),
                        );
                        draw_row(
                            ui,
                            rect,
                            index,
                            &rows[index],
                            active_path.as_deref(),
                            shown_stash.as_deref(),
                            forge,
                            palette,
                            &mut actions,
                        );
                    }
                });
        });

        // Footer: Settings, and which git Facet is driving.
        let row = Rect::from_min_size(
            Pos2::new(
                rect.left() + space::S,
                rect.bottom() - FOOTER_HEIGHT + (FOOTER_HEIGHT - NAV_ROW) / 2.0,
            ),
            Vec2::new(rect.width() - space::S * 2.0, NAV_ROW),
        );
        let tooltip = widgets::tooltip_with_shortcut(
            ui,
            if settings_open {
                "Close Settings"
            } else {
                "Settings"
            },
            &shortcut::SETTINGS,
        );
        if nav_item(
            ui,
            row,
            icon::SETTINGS,
            "Settings",
            None,
            settings_open,
            palette,
        )
        .on_hover_text(tooltip)
        .clicked()
        {
            actions.push(if settings_open {
                Action::CloseSettings
            } else {
                Action::ShowSettings
            });
        }
        self.pending.extend(actions);

        let footer = Pos2::new(row.right() - space::S, row.center().y);
        match &self.git_status {
            GitStatus::Checking => {}
            GitStatus::Found(version) => {
                ui.painter().text(
                    footer,
                    Align2::RIGHT_CENTER,
                    format!("Git {version}"),
                    theme::small(),
                    palette.text_tertiary,
                );
            }
            GitStatus::Missing(detail) => {
                let text_rect = ui.painter().text(
                    footer,
                    Align2::RIGHT_CENTER,
                    "Git not found",
                    theme::small_strong(),
                    palette.danger,
                );
                ui.interact(text_rect, Id::unique("git-missing"), Sense::hover())
                    .on_hover_text(detail);
            }
        }
    }

    /// The flat row list: repository sections (for a working copy), then the recent list.
    fn sidebar_rows(&self) -> Vec<Row<'_>> {
        let collapsed = |section: Section| {
            self.settings
                .collapsed_sections
                .iter()
                .any(|s| s == section.title())
        };
        let mut rows = Vec::new();
        let repo = self
            .active_tab()
            .filter(|tab| matches!(&tab.state, TabState::Ready { info, .. } if !info.is_bare()));
        if let Some(tab) = repo.filter(|tab| tab.refs.loaded) {
            let snapshot = &tab.refs.snapshot;

            let branches: Vec<&GitRef> = snapshot.local().collect();
            rows.push(Row::Header(
                Section::Branches,
                branches.len(),
                collapsed(Section::Branches),
            ));
            if !collapsed(Section::Branches) {
                if branches.is_empty() {
                    rows.push(Row::Hint("No branches yet"));
                }
                rows.extend(branches.into_iter().map(Row::Branch));
            }

            rows.push(Row::Header(
                Section::Remotes,
                snapshot.remotes.len(),
                collapsed(Section::Remotes),
            ));
            if !collapsed(Section::Remotes) {
                if snapshot.remotes.is_empty() {
                    rows.push(Row::Hint("No remotes"));
                }
                for remote in &snapshot.remotes {
                    rows.push(Row::Remote(remote));
                    rows.extend(
                        snapshot
                            .remote_branches(&remote.name)
                            .filter(|r| r.short_name() != "HEAD")
                            .map(Row::RemoteBranch),
                    );
                }
            }

            let tags: Vec<&GitRef> = snapshot.tags().collect();
            rows.push(Row::Header(
                Section::Tags,
                tags.len(),
                collapsed(Section::Tags),
            ));
            if !collapsed(Section::Tags) {
                if tags.is_empty() {
                    rows.push(Row::Hint("No tags"));
                }
                rows.extend(tags.into_iter().map(Row::Tag));
            }

            rows.push(Row::Header(
                Section::Stashes,
                snapshot.stashes.len(),
                collapsed(Section::Stashes),
            ));
            if !collapsed(Section::Stashes) {
                if snapshot.stashes.is_empty() {
                    rows.push(Row::Hint("Nothing stashed"));
                }
                rows.extend(
                    snapshot
                        .stashes
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| Row::Stash(index, entry)),
                );
            }

            let worktrees: Vec<&Worktree> = snapshot.worktrees.iter().filter(|w| !w.bare).collect();
            rows.push(Row::Header(
                Section::Worktrees,
                worktrees.len(),
                collapsed(Section::Worktrees),
            ));
            if !collapsed(Section::Worktrees) {
                if worktrees.len() <= 1 {
                    rows.push(Row::Hint("Only this working folder"));
                } else {
                    rows.extend(worktrees.into_iter().map(|worktree| {
                        Row::Worktree(worktree, same_dir(Path::new(&worktree.path), &tab.path))
                    }));
                }
            }

            if !snapshot.submodules.is_empty() {
                rows.push(Row::Header(
                    Section::Submodules,
                    snapshot.submodules.len(),
                    collapsed(Section::Submodules),
                ));
                if !collapsed(Section::Submodules) {
                    rows.extend(snapshot.submodules.iter().map(Row::Submodule));
                }
            }

            let lfs = &tab.lfs;
            if lfs.usage.lfs {
                rows.push(Row::Header(
                    Section::Locks,
                    lfs.locks.len(),
                    collapsed(Section::Locks),
                ));
                if !collapsed(Section::Locks) {
                    if lfs.locks_running && !lfs.locks_loaded {
                        rows.push(Row::Hint("Asking the LFS server…"));
                    } else if let Some(error) = &lfs.locks_error {
                        rows.push(Row::Note(error.clone()));
                    } else if !lfs.locks_loaded {
                        rows.push(Row::Hint("Refresh to see who has files locked"));
                    } else if lfs.locks.is_empty() {
                        rows.push(Row::Hint("No files are locked"));
                    }
                    rows.extend(lfs.locks.iter().map(Row::Lock));
                }
            }
        }
        rows.push(Row::Header(
            Section::Recent,
            self.settings.recent_repos.len(),
            collapsed(Section::Recent),
        ));
        if !collapsed(Section::Recent) {
            if self.settings.recent_repos.is_empty() {
                rows.push(Row::Hint("Repositories you open appear here"));
            }
            rows.extend(self.settings.recent_repos.iter().map(Row::Recent));
        }
        rows
    }
}

/// Row background and the shared click handling.
/// A navigation item: icon and label, an accent bar and soft fill while it is the page shown,
/// and an optional count on the right.
fn nav_item(
    ui: &Ui,
    rect: Rect,
    glyph: char,
    label: &str,
    badge: Option<&str>,
    active: bool,
    palette: &Palette,
) -> egui::Response {
    let response = ui.interact(rect, Id::unique(("sidebar-nav", label)), Sense::click());
    let fill = if active {
        palette.accent_soft
    } else {
        interaction_fill(ui, &response, palette)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);
    if active {
        let bar = Rect::from_center_size(
            Pos2::new(rect.left() + 1.5, rect.center().y),
            Vec2::new(3.0, 16.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), palette.accent);
    }
    widgets::focus_ring(ui, &response, rect, radius::M, palette);
    ui.painter().text(
        Pos2::new(rect.left() + 20.0, rect.center().y),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(15.0),
        if active {
            palette.accent
        } else {
            palette.text_secondary
        },
    );
    ui.painter().text(
        Pos2::new(rect.left() + 38.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if active {
            theme::strong()
        } else {
            theme::body()
        },
        palette.text,
    );
    if let Some(badge) = badge {
        let galley = ui.painter().layout_no_wrap(
            badge.to_owned(),
            theme::small_strong(),
            palette.text_secondary,
        );
        let width = (galley.size().x + 12.0).max(20.0);
        let pill = Rect::from_center_size(
            Pos2::new(rect.right() - space::S - width / 2.0, rect.center().y),
            Vec2::new(width, 18.0),
        );
        ui.painter().rect_filled(
            pill,
            CornerRadius::same(9),
            if active {
                palette.surface
            } else {
                palette.hover
            },
        );
        ui.painter().galley(
            pill.center() - galley.size() / 2.0,
            galley,
            palette.text_secondary,
        );
    }
    response
}

fn row_base(
    ui: &Ui,
    rect: Rect,
    id: Id,
    active: bool,
    palette: &Palette,
) -> eframe::egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let inner = rect.shrink2(Vec2::new(0.0, 1.0));
    let fill = if active {
        palette.pressed
    } else {
        interaction_fill(ui, &response, palette)
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(inner, CornerRadius::same(radius::M), fill);
    }
    widgets::focus_ring(ui, &response, inner, radius::M, palette);
    response
}

/// Small text right-aligned at `right`; returns where the row's main text has to end.
fn right_text(ui: &Ui, right: f32, center: f32, text: &str, color: Color32) -> f32 {
    if text.is_empty() {
        return right;
    }
    let galley = widgets::elided(ui, text, theme::small(), color, 120.0);
    let left = right - galley.size().x;
    ui.painter().galley(
        Pos2::new(left, center - galley.size().y / 2.0),
        galley,
        color,
    );
    left - space::S
}

fn track_text(track: Track) -> &'static str {
    match track {
        Track::None | Track::InSync => "",
        Track::Ahead => "↑",
        Track::Behind => "↓",
        Track::Diverged => "↑↓",
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn draw_row(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    row: &Row<'_>,
    active_path: Option<&std::path::Path>,
    shown_stash: Option<&str>,
    forge: Option<ForgeKind>,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let center = rect.center().y;
    let text_left = rect.left() + INDENT;
    let mut right = rect.right() - space::S;
    match row {
        Row::Header(section, count, collapsed) => {
            let response = ui.interact(rect, Id::unique(("sidebar-header", index)), Sense::click());
            let hovered = response.hovered();
            // "+" for sections that create something, ⟳ for those that refresh.
            let mut add_clicked = false;
            if let Some((glyph, tooltip, action)) = section.button() {
                let add_rect = Rect::from_center_size(
                    Pos2::new(rect.right() - 14.0, center),
                    Vec2::splat(24.0),
                );
                if (hovered || ui.rect_contains_pointer(add_rect))
                    && widgets::icon_button_at(
                        ui,
                        add_rect,
                        Id::unique(("sidebar-add", index)),
                        glyph,
                        tooltip,
                    )
                    .clicked()
                {
                    actions.push(action);
                    add_clicked = true;
                }
                right = add_rect.left() - space::XS;
            }
            if response.clicked() && !add_clicked {
                actions.push(Action::ToggleSidebarSection(section.title().to_owned()));
            }
            ui.painter().text(
                Pos2::new(rect.left() + space::S + 2.0, center),
                Align2::CENTER_CENTER,
                if *collapsed {
                    icon::CHEVRON_RIGHT
                } else {
                    icon::CHEVRON_DOWN
                },
                theme::icon_font(9.0),
                palette.text_tertiary,
            );
            let width = text_line(
                ui,
                Pos2::new(text_left, center),
                section.title(),
                theme::small_strong(),
                if hovered {
                    palette.text_secondary
                } else {
                    palette.text_tertiary
                },
                right - text_left,
            );
            if *count > 0 {
                text_line(
                    ui,
                    Pos2::new(text_left + width + space::S, center),
                    &count.to_string(),
                    theme::small(),
                    palette.text_tertiary,
                    (right - text_left - width - space::S).max(0.0),
                );
            }
            response.on_hover_text(format!("Show or hide {}", section.title().to_lowercase()));
        }
        Row::Hint(text) => {
            text_line(
                ui,
                Pos2::new(text_left, center),
                text,
                theme::small(),
                palette.text_tertiary,
                right - text_left,
            );
        }
        Row::Branch(branch) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-branch", index)),
                false,
                palette,
            );
            if branch.head {
                ui.painter().text(
                    Pos2::new(rect.left() + space::S + 2.0, center),
                    Align2::CENTER_CENTER,
                    icon::CHECK,
                    theme::icon_font(11.0),
                    palette.accent,
                );
            }
            let track = track_text(branch.track);
            if !track.is_empty() {
                let painted = ui.painter().text(
                    Pos2::new(right, center),
                    Align2::RIGHT_CENTER,
                    track,
                    theme::small_strong(),
                    palette.text_secondary,
                );
                right = painted.left() - space::XS;
            }
            text_line(
                ui,
                Pos2::new(text_left, center),
                &branch.name,
                if branch.head {
                    theme::strong()
                } else {
                    theme::body()
                },
                palette.text,
                right - text_left,
            );
            if response.double_clicked() && !branch.head {
                actions.push(Action::SwitchBranch(branch.name.clone()));
            } else if response.clicked() {
                actions.push(Action::RevealCommit(branch.oid.clone()));
            }
            let hover = match &branch.upstream {
                Some(upstream) => format!(
                    "{}  ·  tracks {upstream}\nClick to show in the history; double-click to switch",
                    branch.name
                ),
                None => format!(
                    "{}\nClick to show in the history; double-click to switch",
                    branch.name
                ),
            };
            let response = response.on_hover_text(hover);
            response.context_menu(|ui| {
                if !branch.head {
                    menu_item(
                        ui,
                        "Switch to Branch",
                        Action::SwitchBranch(branch.name.clone()),
                        actions,
                    );
                    menu_item(
                        ui,
                        "Merge into Current Branch",
                        Action::MergeIntoCurrent(branch.name.clone()),
                        actions,
                    );
                }
                menu_item(
                    ui,
                    "Show in History",
                    Action::RevealCommit(branch.oid.clone()),
                    actions,
                );
                menu_item(
                    ui,
                    "New Branch from Here…",
                    Action::NewBranch {
                        at: Some(branch.oid.clone()),
                    },
                    actions,
                );
                ui.separator();
                menu_item(
                    ui,
                    "Rename…",
                    Action::RenameBranch(branch.name.clone()),
                    actions,
                );
                if !branch.head {
                    menu_item(
                        ui,
                        "Delete…",
                        Action::DeleteBranch(branch.name.clone()),
                        actions,
                    );
                }
                ui.separator();
                menu_item(
                    ui,
                    "Copy Branch Name",
                    Action::CopyText {
                        text: branch.name.clone(),
                        what: "branch name",
                    },
                    actions,
                );
                if let Some(kind) = forge {
                    if branch.upstream.is_some() {
                        menu_item(
                            ui,
                            &format!("Open on {}", forge_name(kind)),
                            Action::OpenBranchOnForge(branch.name.clone()),
                            actions,
                        );
                        let request = match kind {
                            ForgeKind::GitLab => "Create Merge Request",
                            _ => "Create Pull Request",
                        };
                        menu_item(
                            ui,
                            request,
                            Action::CreatePullRequest(branch.name.clone()),
                            actions,
                        );
                    }
                }
            });
        }
        Row::Remote(remote) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-remote", index)),
                false,
                palette,
            );
            text_line(
                ui,
                Pos2::new(text_left, center),
                &remote.name,
                theme::strong(),
                palette.text,
                right - text_left,
            );
            let response =
                response.on_hover_text(remote.url.clone().unwrap_or_else(|| "No URL".to_owned()));
            response.context_menu(|ui| {
                menu_item(
                    ui,
                    &format!("Fetch {}", remote.name),
                    Action::FetchRemote(remote.name.clone()),
                    actions,
                );
                if let Some(url) = &remote.url {
                    menu_item(
                        ui,
                        "Copy URL",
                        Action::CopyText {
                            text: url.clone(),
                            what: "remote URL",
                        },
                        actions,
                    );
                }
                ui.separator();
                menu_item(
                    ui,
                    "Remove Remote…",
                    Action::RemoveRemote(remote.name.clone()),
                    actions,
                );
            });
        }
        Row::RemoteBranch(branch) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-remote-branch", index)),
                false,
                palette,
            );
            let left = text_left + INDENT - space::S;
            text_line(
                ui,
                Pos2::new(left, center),
                branch.short_name(),
                theme::body(),
                palette.text_secondary,
                right - left,
            );
            if response.double_clicked() {
                actions.push(Action::CheckoutRemoteBranch(branch.name.clone()));
            } else if response.clicked() {
                actions.push(Action::RevealCommit(branch.oid.clone()));
            }
            let response = response.on_hover_text(format!(
                "{}\nClick to show in the history; double-click to check out",
                branch.name
            ));
            response.context_menu(|ui| {
                menu_item(
                    ui,
                    "Check Out",
                    Action::CheckoutRemoteBranch(branch.name.clone()),
                    actions,
                );
                menu_item(
                    ui,
                    "Merge into Current Branch",
                    Action::MergeIntoCurrent(branch.name.clone()),
                    actions,
                );
                menu_item(
                    ui,
                    "Show in History",
                    Action::RevealCommit(branch.oid.clone()),
                    actions,
                );
                menu_item(
                    ui,
                    "New Branch from Here…",
                    Action::NewBranch {
                        at: Some(branch.oid.clone()),
                    },
                    actions,
                );
                ui.separator();
                menu_item(
                    ui,
                    "Copy Branch Name",
                    Action::CopyText {
                        text: branch.name.clone(),
                        what: "branch name",
                    },
                    actions,
                );
                if let (Some(kind), RefKind::Remote { .. }) = (forge, &branch.kind) {
                    menu_item(
                        ui,
                        &format!("Open on {}", forge_name(kind)),
                        Action::OpenBranchOnForge(branch.short_name().to_owned()),
                        actions,
                    );
                }
            });
        }
        Row::Tag(tag) => {
            let response = row_base(ui, rect, Id::unique(("sidebar-tag", index)), false, palette);
            text_line(
                ui,
                Pos2::new(text_left, center),
                &tag.name,
                theme::body(),
                palette.text,
                right - text_left,
            );
            if response.clicked() {
                actions.push(Action::RevealCommit(tag.oid.clone()));
            }
            let kind = if tag.tag_object.is_some() {
                "Annotated tag"
            } else {
                "Lightweight tag"
            };
            let response = response.on_hover_text(format!(
                "{} · {kind}\nClick to show in the history",
                tag.name
            ));
            response.context_menu(|ui| {
                menu_item(
                    ui,
                    "Show in History",
                    Action::RevealCommit(tag.oid.clone()),
                    actions,
                );
                menu_item(
                    ui,
                    "Check Out (Detached)",
                    Action::CheckoutCommit(tag.oid.clone()),
                    actions,
                );
                menu_item(ui, "Push Tag", Action::PushTag(tag.name.clone()), actions);
                ui.separator();
                menu_item(
                    ui,
                    "Copy Tag Name",
                    Action::CopyText {
                        text: tag.name.clone(),
                        what: "tag name",
                    },
                    actions,
                );
                menu_item(ui, "Delete…", Action::DeleteTag(tag.name.clone()), actions);
            });
        }
        Row::Stash(position, entry) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-stash", index)),
                shown_stash == Some(entry.oid.as_str()),
                palette,
            );
            if response.clicked() {
                actions.push(Action::ShowStash(*position));
            }
            // Apply / pop on hover.
            let hovered = ui.rect_contains_pointer(rect);
            if hovered {
                for (label, tooltip, action) in [
                    (
                        "Pop",
                        "Apply and drop this stash",
                        Action::PopStash(*position),
                    ),
                    (
                        "Apply",
                        "Apply this stash and keep it",
                        Action::ApplyStash(*position),
                    ),
                ] {
                    let width = widgets::text_button_width(ui, label);
                    let button = Rect::from_min_max(
                        Pos2::new(right - width, center - 11.0),
                        Pos2::new(right, center + 11.0),
                    );
                    right -= width + 2.0;
                    if widgets::text_button_at(
                        ui,
                        button,
                        Id::unique(("stash-button", index, label)),
                        label,
                        tooltip,
                        palette.text,
                        false,
                    )
                    .clicked()
                    {
                        actions.push(action);
                    }
                }
            }
            let message = entry
                .message
                .split_once(": ")
                .map_or(entry.message.as_str(), |(_, rest)| rest);
            text_line(
                ui,
                Pos2::new(text_left, center),
                message,
                theme::body(),
                palette.text,
                (right - text_left - space::XS).max(0.0),
            );
            let response = response.on_hover_text(format!(
                "{}\n{}{}",
                entry.selector,
                entry.message,
                if entry.untracked {
                    "\nIncludes untracked files"
                } else {
                    ""
                }
            ));
            response.context_menu(|ui| {
                menu_item(ui, "Apply", Action::ApplyStash(*position), actions);
                menu_item(
                    ui,
                    "Pop (Apply and Drop)",
                    Action::PopStash(*position),
                    actions,
                );
                ui.separator();
                menu_item(ui, "Drop…", Action::DropStash(*position), actions);
            });
        }
        Row::Note(text) => {
            let painted = text_line(
                ui,
                Pos2::new(text_left, center),
                text,
                theme::small(),
                palette.warning,
                right - text_left,
            );
            let hit =
                Rect::from_min_size(Pos2::new(text_left, rect.top()), Vec2::new(painted, ROW));
            ui.interact(hit, Id::unique(("sidebar-note", index)), Sense::hover())
                .on_hover_text(text.as_str());
        }
        Row::Worktree(worktree, current) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-worktree", index)),
                false,
                palette,
            );
            let path = native_path(&worktree.path);
            if *current {
                ui.painter().text(
                    Pos2::new(rect.left() + space::S + 2.0, center),
                    Align2::CENTER_CENTER,
                    icon::CHECK,
                    theme::icon_font(11.0),
                    palette.accent,
                );
            }
            let (detail, detail_color) = if worktree.prunable.is_some() {
                ("folder gone".to_owned(), palette.warning)
            } else if let Some(branch) = worktree.branch_name() {
                (branch.to_owned(), palette.text_secondary)
            } else {
                ("detached".to_owned(), palette.text_tertiary)
            };
            right = right_text(ui, right, center, &detail, detail_color);
            text_line(
                ui,
                Pos2::new(text_left, center),
                worktree.name(),
                if *current {
                    theme::strong()
                } else {
                    theme::body()
                },
                palette.text,
                right - text_left,
            );
            let openable = !*current && worktree.prunable.is_none();
            if response.clicked() && openable {
                actions.push(Action::OpenPath(path.clone()));
            }
            let mut hover = path.display().to_string();
            if worktree.main {
                hover.push_str("\nThe main worktree");
            }
            if let Some(reason) = &worktree.locked {
                hover.push_str("\nLocked against pruning");
                if !reason.is_empty() {
                    hover.push_str(&format!(": {reason}"));
                }
            }
            if openable {
                hover.push_str("\nClick to open it in a tab");
            }
            let response = response.on_hover_text(hover);
            response.context_menu(|ui| {
                if openable {
                    menu_item(ui, "Open in Tab", Action::OpenPath(path.clone()), actions);
                }
                menu_item(
                    ui,
                    "Copy Folder Path",
                    Action::CopyText {
                        text: path.display().to_string(),
                        what: "folder path",
                    },
                    actions,
                );
                if worktree.prunable.is_some() {
                    ui.separator();
                    menu_item(
                        ui,
                        "Forget Worktrees Whose Folders Are Gone",
                        Action::PruneWorktrees,
                        actions,
                    );
                } else if !worktree.main && !*current {
                    ui.separator();
                    menu_item(
                        ui,
                        "Remove Worktree…",
                        Action::RemoveWorktree(path.clone()),
                        actions,
                    );
                }
            });
        }
        Row::Submodule(module) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-submodule", index)),
                false,
                palette,
            );
            let (detail, color) = match (&module.recorded, module.populated) {
                (_, false) => ("not checked out".to_owned(), palette.warning),
                (Some(oid), true) => (oid.chars().take(7).collect(), palette.text_tertiary),
                (None, true) => (String::new(), palette.text_tertiary),
            };
            right = right_text(ui, right, center, &detail, color);
            text_line(
                ui,
                Pos2::new(text_left, center),
                module.short_name(),
                theme::body(),
                palette.text,
                right - text_left,
            );
            if response.clicked() {
                actions.push(if module.populated {
                    Action::OpenSubmodule(module.path.clone())
                } else {
                    Action::UpdateSubmodules(Some(module.path.clone()))
                });
            }
            let mut hover = module.path.clone();
            if let Some(url) = &module.url {
                hover.push_str(&format!("\n{url}"));
            }
            hover.push_str(if module.populated {
                "\nClick to open it in a tab"
            } else {
                "\nClick to check it out (update)"
            });
            let response = response.on_hover_text(hover);
            response.context_menu(|ui| {
                if module.populated {
                    menu_item(
                        ui,
                        "Open in Tab",
                        Action::OpenSubmodule(module.path.clone()),
                        actions,
                    );
                }
                menu_item(
                    ui,
                    "Update (Check Out Recorded Commit)",
                    Action::UpdateSubmodules(Some(module.path.clone())),
                    actions,
                );
                menu_item(
                    ui,
                    "Sync URL from .gitmodules",
                    Action::SyncSubmodules(Some(module.path.clone())),
                    actions,
                );
                if let Some(url) = &module.url {
                    ui.separator();
                    menu_item(
                        ui,
                        "Copy URL",
                        Action::CopyText {
                            text: url.clone(),
                            what: "submodule URL",
                        },
                        actions,
                    );
                }
            });
        }
        Row::Lock(lock) => {
            let response = row_base(
                ui,
                rect,
                Id::unique(("sidebar-lock", index)),
                false,
                palette,
            );
            let ours = lock.ours == Some(true);
            ui.painter().text(
                Pos2::new(rect.left() + space::S + 2.0, center),
                Align2::CENTER_CENTER,
                icon::LOCK,
                theme::icon_font(11.0),
                if ours {
                    palette.accent
                } else {
                    palette.warning
                },
            );
            let unlock = if lock.ours == Some(false) {
                Action::ForceUnlock {
                    id: lock.id.clone(),
                    path: lock.path.clone(),
                    owner: lock.owner.clone(),
                }
            } else {
                Action::UnlockFile {
                    id: lock.id.clone(),
                    path: lock.path.clone(),
                }
            };
            if ui.rect_contains_pointer(rect) {
                let button =
                    Rect::from_center_size(Pos2::new(right - 12.0, center), Vec2::splat(24.0));
                right = button.left() - space::XS;
                let tooltip = if lock.ours == Some(false) {
                    "Force unlock…"
                } else {
                    "Unlock"
                };
                if widgets::icon_button_at(
                    ui,
                    button,
                    Id::unique(("lock-button", index)),
                    icon::UNLOCK,
                    tooltip,
                )
                .clicked()
                {
                    actions.push(unlock.clone());
                }
            } else {
                let owner = if ours { "you" } else { lock.owner.as_str() };
                right = right_text(ui, right, center, owner, palette.text_tertiary);
            }
            let name = lock.path.rsplit('/').next().unwrap_or(&lock.path);
            text_line(
                ui,
                Pos2::new(text_left, center),
                name,
                theme::body(),
                palette.text,
                right - text_left,
            );
            let owner = match lock.ours {
                Some(true) => "you".to_owned(),
                _ if lock.owner.is_empty() => "someone".to_owned(),
                _ => lock.owner.clone(),
            };
            let response = response.on_hover_text(format!(
                "{}\nLocked by {owner}{}",
                lock.path,
                if lock.locked_at.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", lock.locked_at)
                }
            ));
            response.context_menu(|ui| {
                let label = if lock.ours == Some(false) {
                    "Force Unlock…"
                } else {
                    "Unlock"
                };
                menu_item(ui, label, unlock.clone(), actions);
                if lock.ours.is_none() {
                    menu_item(
                        ui,
                        "Force Unlock…",
                        Action::ForceUnlock {
                            id: lock.id.clone(),
                            path: lock.path.clone(),
                            owner: lock.owner.clone(),
                        },
                        actions,
                    );
                }
                ui.separator();
                menu_item(
                    ui,
                    "Copy File Path",
                    Action::CopyText {
                        text: lock.path.clone(),
                        what: "file path",
                    },
                    actions,
                );
            });
        }
        Row::Recent(path) => {
            let is_active = active_path.is_some_and(|a| same_path(a, path));
            let remove_rect = Rect::from_center_size(
                Pos2::new(rect.right() - space::XS - 12.0, center),
                Vec2::splat(24.0),
            );
            let response = row_base(ui, rect, Id::unique(("recent", index)), is_active, palette);
            let hovered = ui.rect_contains_pointer(rect);
            ui.painter().text(
                Pos2::new(rect.left() + space::S + 2.0, center),
                Align2::CENTER_CENTER,
                icon::FOLDER,
                theme::icon_font(12.0),
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
            let text_right = if hovered {
                remove_rect.left() - space::XS
            } else {
                right
            };
            text_line(
                ui,
                Pos2::new(text_left, center),
                &name,
                theme::body(),
                palette.text,
                text_right - text_left,
            );
            let mut removed = false;
            if hovered
                && widgets::icon_button_at(
                    ui,
                    remove_rect,
                    Id::unique(("recent-remove", index)),
                    icon::CLOSE,
                    "Remove from recent repositories",
                )
                .clicked()
            {
                actions.push(Action::ForgetRecent((*path).clone()));
                removed = true;
            }
            if response.clicked() && !removed {
                actions.push(Action::OpenPath((*path).clone()));
            }
            response.on_hover_text(path.display().to_string());
        }
    }
}
