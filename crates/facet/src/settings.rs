//! Persisted application state: theme choice, layout, recent and open repositories, window
//! placement. Stored as JSON in `%APPDATA%\Facet\settings.json`.
//!
//! Loading never fails: a missing file gives defaults, an unreadable one is set aside as
//! `settings.json.corrupt` and defaults are used. Saving is atomic (write temp, then rename).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::Placement;

/// Set this environment variable to keep settings somewhere else (used for tests and demos).
pub const CONFIG_DIR_ENV: &str = "FACET_CONFIG_DIR";

const FILE_NAME: &str = "settings.json";
const MAX_RECENT: usize = 12;

pub const SIDEBAR_WIDTH: std::ops::RangeInclusive<f32> = 176.0..=480.0;
pub const COMMAND_LOG_HEIGHT: std::ops::RangeInclusive<f32> = 96.0..=640.0;
pub const CHANGES_WIDTH: std::ops::RangeInclusive<f32> = 240.0..=720.0;
pub const COMMIT_BOX_HEIGHT: std::ops::RangeInclusive<f32> = 104.0..=480.0;
pub const DETAIL_WIDTH: std::ops::RangeInclusive<f32> = 320.0..=1200.0;
/// Sidebar sections that can be collapsed (by their title).
pub const SIDEBAR_SECTIONS: [&str; 8] = [
    "Branches",
    "Remotes",
    "Tags",
    "Stashes",
    "Worktrees",
    "Submodules",
    "Locks",
    "Recent",
];
/// Interface sizes offered in settings (egui zoom factors). Ctrl+= / Ctrl+- / Ctrl+0 step
/// through the same range.
pub const UI_SCALES: [f32; 5] = [0.9, 1.0, 1.1, 1.25, 1.5];
pub const UI_SCALE: std::ops::RangeInclusive<f32> = 0.5..=2.0;

/// The typeface used for diffs, ids and command lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodeFont {
    #[default]
    CascadiaMono,
    CascadiaCode,
    Consolas,
}

impl CodeFont {
    pub const ALL: [Self; 3] = [Self::CascadiaMono, Self::CascadiaCode, Self::Consolas];

    pub fn label(self) -> &'static str {
        match self {
            Self::CascadiaMono => "Cascadia Mono",
            Self::CascadiaCode => "Cascadia Code",
            Self::Consolas => "Consolas",
        }
    }

    /// The font file in `%WINDIR%\Fonts`.
    pub fn file(self) -> &'static str {
        match self {
            Self::CascadiaMono => "CascadiaMono.ttf",
            Self::CascadiaCode => "CascadiaCode.ttf",
            Self::Consolas => "consola.ttf",
        }
    }
}

/// How "Pull" integrates the upstream's commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullMode {
    /// Fast-forward only; never creates a merge commit.
    #[default]
    FastForwardOnly,
    /// Rebase local commits onto the upstream.
    Rebase,
}

impl PullMode {
    pub const ALL: [Self; 2] = [Self::FastForwardOnly, Self::Rebase];

    pub fn label(self) -> &'static str {
        match self {
            Self::FastForwardOnly => "Fast-Forward Only",
            Self::Rebase => "Rebase",
        }
    }

    pub fn git(self) -> facet_git::PullMode {
        match self {
            Self::FastForwardOnly => facet_git::PullMode::FastForwardOnly,
            Self::Rebase => facet_git::PullMode::Rebase,
        }
    }
}

/// How two versions of an image are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageMode {
    #[default]
    SideBySide,
    Swipe,
    OnionSkin,
}

impl ImageMode {
    pub const ALL: [Self; 3] = [Self::SideBySide, Self::Swipe, Self::OnionSkin];

    pub fn label(self) -> &'static str {
        match self {
            Self::SideBySide => "Side by Side",
            Self::Swipe => "Swipe",
            Self::OnionSkin => "Onion Skin",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the Windows light/dark setting.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeMode,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub command_log_visible: bool,
    pub command_log_height: f32,
    /// Width of the file list between the sidebar and the diff.
    pub changes_width: f32,
    /// Height of the commit message area under the file list.
    pub commit_box_height: f32,
    pub image_mode: ImageMode,
    /// Width of the commit detail pane beside the history list.
    pub detail_width: f32,
    pub pull_mode: PullMode,
    /// Collapsed sidebar sections (titles from [`SIDEBAR_SECTIONS`]).
    pub collapsed_sections: Vec<String>,
    /// Repositories where the offer to enable `core.fsmonitor` / `core.untrackedCache` was
    /// declined; it is not shown there again.
    pub perf_offer_declined: Vec<PathBuf>,
    /// Most recently opened first.
    pub recent_repos: Vec<PathBuf>,
    /// Repositories open in tabs when the app was last closed, in tab order.
    pub open_repos: Vec<PathBuf>,
    pub active_repo: Option<usize>,
    pub window: Option<Placement>,
    /// External diff / merge tools, chosen by file extension (first match wins).
    pub external_tools: Vec<ExternalTool>,
    /// The git executable; `None` finds `git` on `PATH`.
    pub git_path: Option<PathBuf>,
    /// Interface size (egui zoom factor; 1.0 is the design size).
    pub ui_scale: f32,
    pub code_font: CodeFont,
}

/// An external diff and/or merge tool for some file extensions. Commands are command lines
/// with placeholders: `$LOCAL` and `$REMOTE` for a diff; `$BASE`, `$LOCAL`, `$REMOTE` and
/// `$MERGED` for a merge.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalTool {
    /// Lower-case extensions without the dot (`unity`, `prefab`); `*` matches every file.
    pub extensions: Vec<String>,
    pub diff: String,
    pub merge: String,
}

impl ExternalTool {
    fn matches(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|ext| ext == "*" || ext.eq_ignore_ascii_case(extension))
    }

    pub fn label(&self) -> String {
        let extensions = self
            .extensions
            .iter()
            .map(|ext| {
                if ext == "*" {
                    "all files".to_owned()
                } else {
                    format!(".{ext}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let program = facet_git::tools::split_command(if self.diff.is_empty() {
            &self.merge
        } else {
            &self.diff
        })
        .into_iter()
        .next()
        .map(|program| {
            program
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&program)
                .to_owned()
        })
        .unwrap_or_default();
        format!("{program} ({extensions})")
    }

    /// Parse the extensions as typed: `unity, .prefab  asset` → `unity`, `prefab`, `asset`.
    pub fn parse_extensions(text: &str) -> Vec<String> {
        text.split([',', ' ', ';'])
            .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            sidebar_visible: true,
            sidebar_width: 240.0,
            command_log_visible: false,
            command_log_height: 200.0,
            changes_width: 340.0,
            commit_box_height: 148.0,
            image_mode: ImageMode::default(),
            detail_width: 520.0,
            pull_mode: PullMode::default(),
            collapsed_sections: Vec::new(),
            perf_offer_declined: Vec::new(),
            recent_repos: Vec::new(),
            open_repos: Vec::new(),
            active_repo: None,
            window: None,
            external_tools: Vec::new(),
            git_path: None,
            ui_scale: 1.0,
            code_font: CodeFont::default(),
        }
    }
}

impl Settings {
    /// The command line of the external tool for `path` (a diff tool, or with `merge` a
    /// merge tool): the first tool whose extensions match and that has that command.
    pub fn external_tool(&self, path: &str, merge: bool) -> Option<&str> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let extension = name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map_or("", |(_, ext)| ext);
        self.external_tools
            .iter()
            .filter(|tool| tool.matches(extension))
            .map(|tool| if merge { &tool.merge } else { &tool.diff })
            .find(|command| !command.trim().is_empty())
            .map(String::as_str)
    }

    /// Directory holding Facet's settings, or `None` if no location can be determined.
    pub fn config_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(|appdata| PathBuf::from(appdata).join("Facet"))
    }

    pub fn load() -> Self {
        Self::config_dir()
            .map(|dir| Self::load_from(&dir))
            .unwrap_or_default()
    }

    pub fn load_from(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        // Editors like Notepad may save a byte order mark; JSON parsers reject it.
        match serde_json::from_str::<Self>(text.trim_start_matches('\u{feff}')) {
            Ok(mut settings) => {
                settings.sanitize();
                settings
            }
            Err(_) => {
                // Keep the unreadable file for the user instead of silently overwriting it.
                let _ = std::fs::rename(&path, dir.join(format!("{FILE_NAME}.corrupt")));
                Self::default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        match Self::config_dir() {
            Some(dir) => self.save_to(&dir),
            None => Ok(()),
        }
    }

    pub fn save_to(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, dir.join(FILE_NAME))
    }

    /// Clamp values that came from disk into their valid ranges.
    fn sanitize(&mut self) {
        self.sidebar_width = clamp_or(self.sidebar_width, SIDEBAR_WIDTH, 240.0);
        self.command_log_height = clamp_or(self.command_log_height, COMMAND_LOG_HEIGHT, 200.0);
        self.changes_width = clamp_or(self.changes_width, CHANGES_WIDTH, 340.0);
        self.commit_box_height = clamp_or(self.commit_box_height, COMMIT_BOX_HEIGHT, 148.0);
        self.detail_width = clamp_or(self.detail_width, DETAIL_WIDTH, 520.0);
        self.ui_scale = clamp_or(self.ui_scale, UI_SCALE, 1.0);
        if self
            .git_path
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            self.git_path = None;
        }
        self.collapsed_sections
            .retain(|section| SIDEBAR_SECTIONS.contains(&section.as_str()));
        self.collapsed_sections.sort_unstable();
        self.collapsed_sections.dedup();
        self.recent_repos.truncate(MAX_RECENT);
        if self
            .active_repo
            .is_some_and(|index| index >= self.open_repos.len())
        {
            self.active_repo = None;
        }
    }

    /// Record `path` as the most recently used repository.
    pub fn touch_recent(&mut self, path: &Path) {
        self.recent_repos.retain(|p| !same_path(p, path));
        self.recent_repos.insert(0, path.to_owned());
        self.recent_repos.truncate(MAX_RECENT);
    }

    pub fn forget_recent(&mut self, path: &Path) {
        self.recent_repos.retain(|p| !same_path(p, path));
    }
}

fn clamp_or(value: f32, range: std::ops::RangeInclusive<f32>, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(*range.start(), *range.end())
    } else {
        fallback
    }
}

/// Windows paths are case-insensitive; compare them that way.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    normalize(a) == normalize(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_tools_are_chosen_by_extension() {
        let mut settings = Settings::default();
        assert!(settings.external_tool("a.unity", true).is_none());
        settings.external_tools = vec![
            ExternalTool {
                extensions: ExternalTool::parse_extensions(".Unity, prefab"),
                diff: String::new(),
                merge: r#""C:\Tools\UnityYAMLMerge.exe" merge -p $BASE $REMOTE $LOCAL $MERGED"#
                    .into(),
            },
            ExternalTool {
                extensions: vec!["*".into()],
                diff: "bcomp $LOCAL $REMOTE".into(),
                merge: "bcomp $LOCAL $REMOTE $BASE $MERGED".into(),
            },
        ];
        assert_eq!(
            settings.external_tools[0].extensions,
            vec!["unity", "prefab"]
        );
        assert!(settings
            .external_tool("Assets/Main.UNITY", true)
            .unwrap()
            .contains("UnityYAMLMerge"));
        // No diff command for .unity in the first tool: the catch-all one is used.
        assert_eq!(
            settings.external_tool("Assets/Main.unity", false),
            Some("bcomp $LOCAL $REMOTE")
        );
        assert!(settings
            .external_tool("Makefile", true)
            .unwrap()
            .starts_with("bcomp"));
        assert_eq!(
            settings.external_tools[0].label(),
            "UnityYAMLMerge.exe (.unity, .prefab)"
        );
        assert_eq!(settings.external_tools[1].label(), "bcomp (all files)");
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load_from(dir.path()), Settings::default());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            theme: ThemeMode::Dark,
            sidebar_visible: false,
            sidebar_width: 300.0,
            command_log_visible: true,
            command_log_height: 250.0,
            open_repos: vec![PathBuf::from(r"D:\Games\Shooter")],
            active_repo: Some(0),
            window: Some(Placement {
                x: 10,
                y: 20,
                width: 1280,
                height: 800,
                maximized: true,
            }),
            ..Settings::default()
        };
        settings.touch_recent(Path::new(r"D:\Games\Shooter"));
        settings.save_to(dir.path()).unwrap();

        assert_eq!(Settings::load_from(dir.path()), settings);
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[test]
    fn corrupt_file_is_set_aside_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(Settings::load_from(dir.path()), Settings::default());
        assert!(dir.path().join("settings.json.corrupt").exists());
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            r#"{ "theme": "light", "from_the_future": 1 }"#,
        )
        .unwrap();
        let settings = Settings::load_from(dir.path());
        assert_eq!(settings.theme, ThemeMode::Light);
        assert_eq!(settings.sidebar_width, Settings::default().sidebar_width);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            r#"{ "sidebar_width": 5.0, "command_log_height": 99999.0, "active_repo": 3,
                 "detail_width": 1.0, "collapsed_sections": ["Tags", "Bogus", "Tags"],
                 "pull_mode": "rebase", "ui_scale": 9.0, "git_path": "",
                 "code_font": "consolas" }"#,
        )
        .unwrap();
        let settings = Settings::load_from(dir.path());
        assert_eq!(settings.ui_scale, *UI_SCALE.end());
        assert_eq!(settings.git_path, None);
        assert_eq!(settings.code_font, CodeFont::Consolas);
        assert_eq!(settings.sidebar_width, *SIDEBAR_WIDTH.start());
        assert_eq!(settings.command_log_height, *COMMAND_LOG_HEIGHT.end());
        assert_eq!(settings.active_repo, None);
        assert_eq!(settings.detail_width, *DETAIL_WIDTH.start());
        assert_eq!(settings.collapsed_sections, ["Tags"]);
        assert_eq!(settings.pull_mode, PullMode::Rebase);
    }

    #[test]
    fn recents_are_deduplicated_case_insensitively_and_capped() {
        let mut settings = Settings::default();
        settings.touch_recent(Path::new(r"D:\Repo\One"));
        settings.touch_recent(Path::new(r"D:\Repo\Two"));
        settings.touch_recent(Path::new(r"d:\repo\one\"));
        assert_eq!(
            settings.recent_repos,
            [
                PathBuf::from(r"d:\repo\one\"),
                PathBuf::from(r"D:\Repo\Two")
            ]
        );

        for i in 0..40 {
            settings.touch_recent(Path::new(&format!(r"D:\Repo\N{i}")));
        }
        assert_eq!(settings.recent_repos.len(), MAX_RECENT);
        assert_eq!(settings.recent_repos[0], PathBuf::from(r"D:\Repo\N39"));

        settings.forget_recent(Path::new(r"D:\REPO\n39"));
        assert_eq!(settings.recent_repos[0], PathBuf::from(r"D:\Repo\N38"));
    }
}
