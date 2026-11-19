//! Other programs Facet works with: opening the repository in Windows Terminal, VS Code or
//! Rider, and the external diff / merge tools that settings offers as presets.
//!
//! Detection only looks at well-known install folders and `PATH` (no registry, no shell): it
//! runs once in the background at startup and again when settings open.

use std::path::{Path, PathBuf};

use crate::settings::ExternalTool;

/// A program the repository can be opened in. File Explorer is always there and has its own
/// action (`RevealInExplorer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    Terminal,
    VsCode,
    Rider,
}

impl OpenTarget {
    pub const ALL: [Self; 3] = [Self::Terminal, Self::VsCode, Self::Rider];

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Windows Terminal",
            Self::VsCode => "VS Code",
            Self::Rider => "Rider",
        }
    }
}

/// A diff / merge tool settings can set up in one click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPreset {
    UnityYamlMerge,
    P4Merge,
    BeyondCompare,
}

impl ToolPreset {
    pub const ALL: [Self; 3] = [Self::UnityYamlMerge, Self::P4Merge, Self::BeyondCompare];

    pub fn label(self) -> &'static str {
        match self {
            Self::UnityYamlMerge => "UnityYAMLMerge",
            Self::P4Merge => "P4Merge",
            Self::BeyondCompare => "Beyond Compare",
        }
    }

    pub fn exe_name(self) -> &'static str {
        match self {
            Self::UnityYamlMerge => "UnityYAMLMerge.exe",
            Self::P4Merge => "p4merge.exe",
            Self::BeyondCompare => "BComp.exe",
        }
    }

    /// The tool's settings with `exe` (a full path, or the bare name when not found).
    pub fn tool(self, exe: &str) -> ExternalTool {
        let exe = format!("\"{exe}\"");
        match self {
            // Smart merge for Unity's YAML scenes and prefabs; it has no diff mode.
            Self::UnityYamlMerge => ExternalTool {
                extensions: ExternalTool::parse_extensions(
                    "unity prefab asset mat anim controller overrideController physicMaterial \
                     playable mask terrainlayer spriteatlas meta",
                ),
                diff: String::new(),
                merge: format!("{exe} merge -p $BASE $REMOTE $LOCAL $MERGED"),
            },
            // Same argument order as git's own p4merge driver: base, theirs, yours, result.
            Self::P4Merge => ExternalTool {
                extensions: vec!["*".into()],
                diff: format!("{exe} $LOCAL $REMOTE"),
                merge: format!("{exe} $BASE $REMOTE $LOCAL $MERGED"),
            },
            // Left, right, center (base), output.
            Self::BeyondCompare => ExternalTool {
                extensions: vec!["*".into()],
                diff: format!("{exe} $LOCAL $REMOTE"),
                merge: format!("{exe} $LOCAL $REMOTE $BASE $MERGED"),
            },
        }
    }
}

/// What was found on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installed {
    pub terminal: Option<PathBuf>,
    pub vscode: Option<PathBuf>,
    pub rider: Option<PathBuf>,
    pub unity_yaml_merge: Option<PathBuf>,
    pub p4merge: Option<PathBuf>,
    pub beyond_compare: Option<PathBuf>,
}

impl Installed {
    pub fn launcher(&self, target: OpenTarget) -> Option<&Path> {
        match target {
            OpenTarget::Terminal => self.terminal.as_deref(),
            OpenTarget::VsCode => self.vscode.as_deref(),
            OpenTarget::Rider => self.rider.as_deref(),
        }
    }

    pub fn preset(&self, preset: ToolPreset) -> Option<&Path> {
        match preset {
            ToolPreset::UnityYamlMerge => self.unity_yaml_merge.as_deref(),
            ToolPreset::P4Merge => self.p4merge.as_deref(),
            ToolPreset::BeyondCompare => self.beyond_compare.as_deref(),
        }
    }

    /// The programs the repository can be opened in, in menu order.
    pub fn open_targets(&self) -> Vec<OpenTarget> {
        OpenTarget::ALL
            .into_iter()
            .filter(|target| self.launcher(*target).is_some())
            .collect()
    }
}

/// Where to look.
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub local_app_data: Option<PathBuf>,
    /// `Program Files` and `Program Files (x86)`.
    pub program_files: Vec<PathBuf>,
    pub path: Vec<PathBuf>,
}

impl Places {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let mut program_files: Vec<PathBuf> = ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(var)
            .map(PathBuf::from)
            .collect();
        program_files.dedup_by(|a, b| crate::settings::same_path(a, b));
        Self {
            local_app_data: var("LOCALAPPDATA").map(PathBuf::from),
            program_files,
            path: var("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default(),
        }
    }
}

/// Look for every integration. Touches the file system: run it off the UI thread.
pub fn detect(places: &Places) -> Installed {
    Installed {
        terminal: find_terminal(places),
        vscode: find_vscode(places),
        rider: find_rider(places),
        unity_yaml_merge: find_unity_yaml_merge(places),
        p4merge: first_file(
            places
                .program_files
                .iter()
                .map(|dir| dir.join(r"Perforce\p4merge.exe"))
                .chain(on_path(places, "p4merge.exe")),
        ),
        beyond_compare: first_file(
            ["Beyond Compare 5", "Beyond Compare 4"]
                .into_iter()
                .flat_map(|name| {
                    places
                        .program_files
                        .iter()
                        .map(move |dir| dir.join(name).join("BComp.exe"))
                })
                .chain(on_path(places, "BComp.exe")),
        ),
    }
}

/// Start `target` on `folder`. Spawns and returns; the program is not Facet's child in any
/// way that outlives this call (it keeps running when Facet exits).
pub fn launch(target: OpenTarget, exe: &Path, folder: &Path) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    match target {
        // `wt` splits its command line at `;` (new tab); a folder name may contain one.
        OpenTarget::Terminal => command
            .arg("-d")
            .arg(folder.to_string_lossy().replace(';', r"\;")),
        OpenTarget::VsCode | OpenTarget::Rider => command.arg(folder),
    };
    command.current_dir(folder).spawn().map(drop)
}

/// A file exists here. App execution aliases (`wt.exe` in `WindowsApps`) are reparse points
/// that can't be opened for metadata, so the link itself is checked.
fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_dir())
}

fn first_file(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| is_file(path))
}

fn on_path<'a>(places: &'a Places, name: &'a str) -> impl Iterator<Item = PathBuf> + 'a {
    places.path.iter().map(move |dir| dir.join(name))
}

fn find_terminal(places: &Places) -> Option<PathBuf> {
    first_file(
        places
            .local_app_data
            .iter()
            .map(|dir| dir.join(r"Microsoft\WindowsApps\wt.exe"))
            .chain(on_path(places, "wt.exe")),
    )
}

fn find_vscode(places: &Places) -> Option<PathBuf> {
    let installs = places
        .local_app_data
        .iter()
        .map(|dir| dir.join(r"Programs\Microsoft VS Code\Code.exe"))
        .chain(
            places
                .program_files
                .iter()
                .map(|dir| dir.join(r"Microsoft VS Code\Code.exe")),
        );
    // `code.cmd` on PATH lives in `<install>\bin`; run the editor itself, not the script.
    let from_path = places
        .path
        .iter()
        .filter(|dir| is_file(&dir.join("code.cmd")))
        .filter_map(|dir| Some(dir.parent()?.join("Code.exe")));
    first_file(installs.chain(from_path))
}

fn find_rider(places: &Places) -> Option<PathBuf> {
    // JetBrains Toolbox writes a launcher script that names the current install.
    let toolbox = places
        .local_app_data
        .as_ref()
        .and_then(|dir| {
            std::fs::read_to_string(dir.join(r"JetBrains\Toolbox\scripts\rider.cmd")).ok()
        })
        .and_then(|script| quoted_exe(&script, "rider64.exe"));
    let toolbox_apps = places
        .local_app_data
        .iter()
        .flat_map(|dir| newest_subdirs(&dir.join("Programs"), "Rider"))
        .map(|dir| dir.join(r"bin\rider64.exe"));
    let installers = places
        .program_files
        .iter()
        .flat_map(|dir| newest_subdirs(&dir.join("JetBrains"), "JetBrains Rider"))
        .map(|dir| dir.join(r"bin\rider64.exe"));
    first_file(
        toolbox
            .into_iter()
            .chain(toolbox_apps)
            .chain(installers)
            .chain(on_path(places, "rider64.exe")),
    )
}

fn find_unity_yaml_merge(places: &Places) -> Option<PathBuf> {
    let hub = places
        .program_files
        .iter()
        .flat_map(|dir| newest_subdirs(&dir.join(r"Unity\Hub\Editor"), ""))
        .map(|dir| dir.join(r"Editor\Data\Tools\UnityYAMLMerge.exe"));
    let standalone = places
        .program_files
        .iter()
        .map(|dir| dir.join(r"Unity\Editor\Data\Tools\UnityYAMLMerge.exe"));
    first_file(hub.chain(standalone))
}

/// Subfolders of `dir` whose names start with `prefix` (case-insensitive), newest version
/// first (`2023.2.10f1` after `2023.2.9f1`).
fn newest_subdirs(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = prefix.to_lowercase();
    let mut found: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.to_lowercase()
                .starts_with(&prefix)
                .then(|| (name, entry.path()))
        })
        .collect();
    found.sort_by(|(a, _), (b, _)| version_order(b, a));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Compare names with runs of digits as numbers, so `9` sorts before `10`.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(text: &str) -> Vec<(bool, String)> {
        let mut parts: Vec<(bool, String)> = Vec::new();
        for c in text.chars() {
            let digit = c.is_ascii_digit();
            match parts.last_mut() {
                Some((was_digit, part)) if *was_digit == digit => part.push(c),
                _ => parts.push((digit, c.to_string())),
            }
        }
        parts
    }
    let (a, b) = (parts(a), parts(b));
    for ((a_digit, a), (b_digit, b)) in a.iter().zip(&b) {
        let order = if *a_digit && *b_digit {
            let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        } else {
            a.to_lowercase().cmp(&b.to_lowercase())
        };
        if order.is_ne() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// The first double-quoted path in `script` whose file name is `name`.
fn quoted_exe(script: &str, name: &str) -> Option<PathBuf> {
    script
        .split('"')
        .skip(1)
        .step_by(2)
        .map(Path::new)
        .find(|path| {
            path.file_name()
                .is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case(name))
        })
        .map(Path::to_path_buf)
}

/// Open `folder` in the terminal chosen in settings. Console programs started from Facet get a
/// console window of their own. `git_exe` is where Git Bash is looked for (next to git).
pub fn open_terminal(
    terminal: crate::settings::Terminal,
    installed: &Installed,
    folder: &Path,
    git_exe: &Path,
) -> std::io::Result<()> {
    use crate::settings::Terminal;
    match terminal {
        Terminal::WindowsTerminal => match installed.terminal.as_deref() {
            Some(exe) => launch(OpenTarget::Terminal, exe, folder),
            None => Err(not_found("Windows Terminal")),
        },
        Terminal::PowerShell => {
            let places = Places::from_env();
            let exe = first_file(on_path(&places, "pwsh.exe"))
                .unwrap_or_else(|| PathBuf::from("powershell.exe"));
            std::process::Command::new(exe)
                .arg("-NoExit")
                .current_dir(folder)
                .spawn()
                .map(drop)
        }
        Terminal::CommandPrompt => std::process::Command::new("cmd.exe")
            .arg("/K")
            .current_dir(folder)
            .spawn()
            .map(drop),
        Terminal::GitBash => {
            let exe = git_bash(git_exe).ok_or_else(|| not_found("Git Bash"))?;
            std::process::Command::new(exe)
                .arg(format!("--cd={}", folder.display()))
                .current_dir(folder)
                .spawn()
                .map(drop)
        }
    }
}

/// `git-bash.exe` of the Git for Windows that `git_exe` belongs to (`…\Git\cmd\git.exe` or
/// `…\Git\mingw64\bin\git.exe`), else of the usual install.
fn git_bash(git_exe: &Path) -> Option<PathBuf> {
    let from_git = git_exe
        .ancestors()
        .skip(1)
        .take(3)
        .map(|dir| dir.join("git-bash.exe"));
    let places = Places::from_env();
    let installs = places
        .program_files
        .iter()
        .map(|dir| dir.join(r"Git\git-bash.exe"))
        .chain(
            places
                .local_app_data
                .iter()
                .map(|dir| dir.join(r"Programs\Git\git-bash.exe")),
        )
        .collect::<Vec<_>>();
    // `git` on PATH: find the folder it is in.
    let on_path_git = places
        .path
        .iter()
        .map(|dir| dir.join("git.exe"))
        .find(|path| is_file(path))
        .into_iter()
        .flat_map(|git| {
            git.ancestors()
                .skip(1)
                .take(3)
                .map(|dir| dir.join("git-bash.exe"))
                .collect::<Vec<_>>()
        });
    first_file(from_git.chain(on_path_git).chain(installs))
}

/// Open the repository folder, or `file` in it, in the editor chosen in settings.
pub fn open_in_editor(
    editor: crate::settings::Editor,
    installed: &Installed,
    root: &Path,
    file: Option<&Path>,
) -> std::io::Result<()> {
    use crate::settings::Editor;
    let target = file.map_or_else(|| root.to_owned(), |file| root.join(file));
    let (open_target, exe) = match editor {
        Editor::VsCode => (OpenTarget::VsCode, installed.vscode.as_deref()),
        Editor::Rider => (OpenTarget::Rider, installed.rider.as_deref()),
        Editor::System => {
            return if file.is_some() {
                crate::platform::edit_file(&target)
            } else {
                crate::platform::reveal_in_explorer(root)
            };
        }
    };
    let exe = exe.ok_or_else(|| not_found(editor.label()))?;
    if file.is_some() {
        // Both editors take a file after the folder: the file opens in that project.
        std::process::Command::new(exe)
            .arg(root)
            .arg(&target)
            .current_dir(root)
            .spawn()
            .map(drop)
    } else {
        launch(open_target, exe, root)
    }
}

fn not_found(name: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{name} wasn't found on this PC"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn installs_are_found_in_their_usual_folders() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("Local");
        let programs = root.path().join("Program Files");
        let bin = root.path().join("bin");
        touch(&local.join(r"Microsoft\WindowsApps\wt.exe"));
        // VS Code found through `code.cmd` on PATH.
        touch(&root.path().join(r"VSCode\bin\code.cmd"));
        touch(&root.path().join(r"VSCode\Code.exe"));
        // Rider: the newest installer folder wins.
        touch(&programs.join(r"JetBrains\JetBrains Rider 2024.9\bin\rider64.exe"));
        touch(&programs.join(r"JetBrains\JetBrains Rider 2024.10\bin\rider64.exe"));
        // Unity: the newest editor from the hub.
        for version in ["2022.3.9f1", "2022.3.10f1"] {
            touch(&programs.join(format!(
                r"Unity\Hub\Editor\{version}\Editor\Data\Tools\UnityYAMLMerge.exe"
            )));
        }
        touch(&bin.join("p4merge.exe"));
        let places = Places {
            local_app_data: Some(local.clone()),
            program_files: vec![programs.clone()],
            path: vec![bin.clone(), root.path().join(r"VSCode\bin")],
        };
        let found = detect(&places);
        assert_eq!(
            found.terminal,
            Some(local.join(r"Microsoft\WindowsApps\wt.exe"))
        );
        assert_eq!(found.vscode, Some(root.path().join(r"VSCode\Code.exe")));
        assert_eq!(
            found.rider,
            Some(programs.join(r"JetBrains\JetBrains Rider 2024.10\bin\rider64.exe"))
        );
        assert!(found
            .unity_yaml_merge
            .unwrap()
            .to_string_lossy()
            .contains("2022.3.10f1"));
        assert_eq!(found.p4merge, Some(bin.join("p4merge.exe")));
        assert_eq!(found.beyond_compare, None);
        assert_eq!(
            Installed {
                rider: None,
                ..detect(&places)
            }
            .open_targets(),
            [OpenTarget::Terminal, OpenTarget::VsCode]
        );
    }

    #[test]
    fn the_toolbox_script_names_the_rider_install() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("Local");
        let rider = local.join(r"Programs\Rider 3\bin\rider64.exe");
        touch(&rider);
        let script = format!(
            "@echo off\r\nset waitarg=\r\nstart \"\" %waitarg% \"{}\" %intellij_args%\r\n",
            rider.display()
        );
        std::fs::create_dir_all(local.join(r"JetBrains\Toolbox\scripts")).unwrap();
        std::fs::write(local.join(r"JetBrains\Toolbox\scripts\Rider.cmd"), script).unwrap();
        let places = Places {
            local_app_data: Some(local),
            ..Places::default()
        };
        assert_eq!(detect(&places).rider, Some(rider));
    }

    #[test]
    fn versions_sort_numerically() {
        use std::cmp::Ordering::{Greater, Less};
        assert_eq!(version_order("2023.2.10f1", "2023.2.9f1"), Greater);
        assert_eq!(version_order("Rider 3", "Rider"), Greater);
        assert_eq!(
            version_order("JetBrains Rider 2024.1", "JetBrains Rider 2024.1.2"),
            Less
        );
    }

    #[test]
    fn presets_fill_in_the_found_program() {
        for preset in ToolPreset::ALL {
            let tool = preset.tool(r"C:\Tools\x.exe");
            assert!(!tool.extensions.is_empty());
            assert!(tool.merge.starts_with(r#""C:\Tools\x.exe" "#), "{tool:?}");
            assert!(tool.merge.contains("$MERGED"));
            assert!(tool.diff.is_empty() || tool.diff.contains("$LOCAL"));
        }
        let unity = ToolPreset::UnityYamlMerge.tool("UnityYAMLMerge.exe");
        assert!(unity.extensions.contains(&"overridecontroller".to_owned()));
        assert!(unity.diff.is_empty());
    }
}
