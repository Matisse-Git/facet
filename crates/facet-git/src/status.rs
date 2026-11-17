//! Working tree status and the per-repository settings that make it fast on huge repositories.

use std::path::Path;

use crate::parse::{parse_status, Status};
use crate::{CancellationToken, Git, GitError};

/// Read the status of the working tree and index, plus what `HEAD` points at.
///
/// `background` marks an automatic refresh (file watcher, window focus): git then takes no
/// optional locks, so it can never collide with a command the user runs in a terminal. The
/// price is that git cannot save what it learned (refreshed stat data, untracked cache), so
/// refreshes the user asks for, and the ones after Facet's own operations, run with
/// `background = false`.
///
/// Untracked files are listed individually (`--untracked-files=all`) so each one can be
/// diffed and staged on its own.
pub async fn status(
    git: &Git,
    root: &Path,
    background: bool,
    cancel: Option<&CancellationToken>,
) -> Result<Status, GitError> {
    let mut command = git
        .command([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ])
        .current_dir(root);
    if background {
        command = command.background();
    }
    if let Some(cancel) = cancel {
        command = command.cancel_with(cancel);
    }
    let output = command.run().await?;
    parse_status(&output.stdout)
}

/// The two per-repository settings that speed up `git status` on very large working trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfConfig {
    /// `core.fsmonitor`: git asks a file system monitor what changed instead of scanning.
    pub fsmonitor: bool,
    /// `core.untrackedCache`: git remembers which directories hold no untracked files.
    pub untracked_cache: bool,
}

impl PerfConfig {
    pub fn all_enabled(self) -> bool {
        self.fsmonitor && self.untracked_cache
    }
}

/// Read this repository's own (`--local`) values; global and system config are not consulted.
pub async fn perf_config(git: &Git, root: &Path) -> Result<PerfConfig, GitError> {
    let output = git
        .command([
            "config",
            "--local",
            "-z",
            "--get-regexp",
            r"^core\.(fsmonitor|untrackedcache)$",
        ])
        .current_dir(root)
        .background()
        .output()
        .await?;
    // Exit status 1 just means "no such keys".
    let mut config = PerfConfig::default();
    if !output.success() {
        return Ok(config);
    }
    // `-z`: `<key>\n<value>\0` per entry; keys are lower-cased by git.
    for record in output.stdout.split(|b| *b == 0) {
        let text = String::from_utf8_lossy(record);
        let (key, value) = text.split_once('\n').unwrap_or((&text, "true"));
        let enabled = !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "no" | "off" | "0" | ""
        );
        match key {
            "core.fsmonitor" => config.fsmonitor = enabled,
            "core.untrackedcache" => config.untracked_cache = enabled,
            _ => {}
        }
    }
    Ok(config)
}

/// Turn `core.fsmonitor` and `core.untrackedCache` on or off **for this repository only**
/// (`git config --local`). Call only after the user agreed; the global config is never touched.
pub async fn set_perf_config(git: &Git, root: &Path, enable: bool) -> Result<(), GitError> {
    for key in ["core.fsmonitor", "core.untrackedCache"] {
        if enable {
            git.command(["config", "--local", key, "true"])
                .current_dir(root)
                .run()
                .await?;
        } else {
            // Exit status 5: the key was not set, which is the goal anyway.
            let output = git
                .command(["config", "--local", "--unset", key])
                .current_dir(root)
                .output()
                .await?;
            if !output.success() && output.status != 5 {
                return Err(GitError::Failed {
                    command: format!("git config --local --unset {key}"),
                    code: output.status,
                    stderr: output.stderr_text(),
                });
            }
        }
    }
    if !enable {
        // Stop the monitor daemon this repository may have started; failing is fine (it may
        // not be running, or this git may not have one).
        let _ = git
            .command(["fsmonitor--daemon", "stop"])
            .current_dir(root)
            .output()
            .await;
    }
    Ok(())
}
