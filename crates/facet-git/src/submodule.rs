//! Submodules: list (from `.gitmodules` + the index), init/update, sync, and the commits
//! between two recorded pointers (the "diff" of a submodule).

use std::path::Path;

use crate::parse::{parse_gitlinks, parse_gitmodules, parse_log, LogCommit, Submodule, LOG_FORMAT};
use crate::remote::Network;
use crate::{Git, GitError};

/// Every submodule declared in `.gitmodules`, with the commit the index records and whether
/// it is checked out. A repository without `.gitmodules` costs no process at all.
pub async fn submodules(git: &Git, root: &Path) -> Result<Vec<Submodule>, GitError> {
    if !tokio::fs::try_exists(root.join(".gitmodules"))
        .await
        .unwrap_or(false)
    {
        return Ok(Vec::new());
    }
    let config = git
        .command([
            "config",
            "-z",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\.",
        ])
        .current_dir(root)
        .background()
        .output()
        .await?;
    // Exit 1: nothing matched (an empty `.gitmodules`).
    let mut modules = match config.status {
        0 => parse_gitmodules(&config.stdout)?,
        1 => return Ok(Vec::new()),
        code => {
            return Err(GitError::Failed {
                command: "git config --file .gitmodules".to_owned(),
                code,
                stderr: config.stderr_text(),
            })
        }
    };
    if modules.is_empty() {
        return Ok(modules);
    }
    let links = git
        .command(["ls-files", "-z", "--stage", "--"])
        .args(
            modules
                .iter()
                .map(|module| format!(":(literal){}", module.path)),
        )
        .current_dir(root)
        .background()
        .run()
        .await?;
    let links = parse_gitlinks(&links.stdout)?;
    for module in &mut modules {
        module.recorded = links
            .iter()
            .find(|(path, _)| *path == module.path)
            .map(|(_, oid)| oid.clone());
        module.populated = tokio::fs::try_exists(root.join(&module.path).join(".git"))
            .await
            .unwrap_or(false);
    }
    Ok(modules)
}

/// `git submodule update --init --recursive [-- path]`: clone or fetch as needed and check
/// out the recorded commits. A network command (credentials, progress, cancel).
pub async fn update_submodules(net: &Network<'_>, path: Option<&str>) -> Result<(), GitError> {
    let mut args = vec!["submodule", "update", "--init", "--recursive", "--progress"];
    if let Some(path) = path {
        args.extend(["--", path]);
    }
    net.run(&args).await
}

/// `git submodule sync --recursive [-- path]`: copy URLs from `.gitmodules` into the
/// repository's config (after a submodule moved servers).
pub async fn sync_submodules(git: &Git, root: &Path, path: Option<&str>) -> Result<(), GitError> {
    let mut command = git.command(["submodule", "sync", "--recursive"]);
    if let Some(path) = path {
        command = command.arg("--").arg(path);
    }
    command.current_dir(root).run().await.map(drop)
}

/// The commits a submodule pointer change brings in and drops: `(added, removed)`, newest
/// first. Read inside the submodule's own repository; fails when it is not checked out or
/// lacks one of the commits (not fetched yet).
pub async fn submodule_range(
    git: &Git,
    submodule_root: &Path,
    old: &str,
    new: &str,
) -> Result<(Vec<LogCommit>, Vec<LogCommit>), GitError> {
    let log = |range: String| {
        git.command(["log", "-z", LOG_FORMAT, "--no-show-signature", "-n", "500"])
            .arg(range)
            .arg("--")
            .current_dir(submodule_root)
            .background()
            .run()
    };
    let (added, removed) = tokio::join!(log(format!("{old}..{new}")), log(format!("{new}..{old}")));
    Ok((parse_log(&added?.stdout)?, parse_log(&removed?.stdout)?))
}
