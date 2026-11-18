//! Getting a repository onto the machine: clone one from a URL, or start a new one.
//!
//! Clone is a network command like fetch (live progress shown verbatim, cancellable,
//! credentials through Git Credential Manager or Facet's askpass bridge).

use std::path::Path;

use crate::{CancellationToken, Git, GitError, ProgressFn};

/// `git clone --progress -- <url> <dest>`. `dest` must not exist yet or be an empty folder
/// (git refuses anything else with its own message). Run from `dest`'s parent so a relative
/// path in `url` (a local clone) means what the person typed.
pub async fn clone(
    git: &Git,
    url: &str,
    dest: &Path,
    cancel: &CancellationToken,
    progress: ProgressFn,
) -> Result<(), GitError> {
    let parent = dest.parent().unwrap_or(dest);
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| GitError::Unsupported(format!("{}: {error}", parent.display())))?;
    let dest = dest.to_string_lossy();
    git.with_env("GIT_LFS_FORCE_PROGRESS", "1")
        .command(["clone", "--progress", "--", url, &dest])
        .current_dir(parent)
        .cancel_with(cancel)
        .progress(progress)
        .run()
        .await
        .map(drop)
}

/// `git init -b <branch> -- <dest>`: a new, empty repository (the folder is created if
/// needed; an existing repository is left as it is, as git does).
pub async fn init(git: &Git, dest: &Path, initial_branch: &str) -> Result<(), GitError> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|error| GitError::Unsupported(format!("{}: {error}", dest.display())))?;
    git.command(["init", "-b", initial_branch, "--", "."])
        .current_dir(dest)
        .run()
        .await
        .map(drop)
}

/// The folder name `git clone` would pick for `url`: the last path segment without a
/// trailing `.git` (`https://github.com/acme/website.git` → `website`,
/// `git@github.com:acme/api.git` → `api`, `D:\src\tool\` → `tool`). `None` if nothing usable
/// is left.
pub fn clone_folder_name(url: &str) -> Option<String> {
    let trimmed = url
        .trim()
        .trim_end_matches(['/', '\\'])
        .trim_end_matches(".git")
        .trim_end_matches(['/', '\\']);
    let name = trimmed
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or_default()
        .trim();
    let valid = !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || c.is_control());
    valid.then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::clone_folder_name;

    #[test]
    fn folder_names_follow_git() {
        for (url, name) in [
            ("https://github.com/acme/website.git", Some("website")),
            ("https://github.com/acme/website", Some("website")),
            ("https://github.com/acme/website/", Some("website")),
            ("git@github.com:acme/api.git", Some("api")),
            ("ssh://git@host:2222/team/tools.git/", Some("tools")),
            (r"D:\src\tool\", Some("tool")),
            ("  https://example.com/a/b-c.d.git  ", Some("b-c.d")),
            ("", None),
            ("https://", None),
            ("..", None),
        ] {
            assert_eq!(clone_folder_name(url).as_deref(), name, "{url}");
        }
    }
}
