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
    /// Most recently opened first.
    pub recent_repos: Vec<PathBuf>,
    /// Repositories open in tabs when the app was last closed, in tab order.
    pub open_repos: Vec<PathBuf>,
    pub active_repo: Option<usize>,
    pub window: Option<Placement>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            sidebar_visible: true,
            sidebar_width: 240.0,
            command_log_visible: false,
            command_log_height: 200.0,
            recent_repos: Vec::new(),
            open_repos: Vec::new(),
            active_repo: None,
            window: None,
        }
    }
}

impl Settings {
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
        match serde_json::from_str::<Self>(&text) {
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
            r#"{ "sidebar_width": 5.0, "command_log_height": 99999.0, "active_repo": 3 }"#,
        )
        .unwrap();
        let settings = Settings::load_from(dir.path());
        assert_eq!(settings.sidebar_width, *SIDEBAR_WIDTH.start());
        assert_eq!(settings.command_log_height, *COMMAND_LOG_HEIGHT.end());
        assert_eq!(settings.active_repo, None);
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
