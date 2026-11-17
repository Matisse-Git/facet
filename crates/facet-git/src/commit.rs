//! Creating commits. Hooks always run: there is no `--no-verify` anywhere in Facet.

use std::path::Path;

use crate::{Git, GitError};

/// What happened when `git commit` ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub success: bool,
    /// Everything git and the hooks printed (stdout, then stderr), trimmed. On failure this is
    /// the explanation to show the user; on success it is hook chatter, usually empty.
    pub output: String,
}

/// Commit what is staged, with `message` as the commit message. With `amend`, replace the
/// last commit instead (its content plus whatever is staged).
///
/// A rejected commit (failing hook, nothing staged, empty message, …) is not an `Err`: it
/// comes back as `success == false` with git's and the hooks' own words.
pub async fn commit(
    git: &Git,
    root: &Path,
    message: &str,
    amend: bool,
) -> Result<CommitOutcome, GitError> {
    // `--quiet` drops git's own human-readable summary so the output is the hooks' alone.
    // The message goes over stdin: no quoting, no length limit, no temp file.
    let mut command = git.command(["commit", "--quiet"]);
    if amend {
        command = command.arg("--amend");
    }
    let output = command
        .arg("--file=-")
        .current_dir(root)
        .stdin(message)
        .log_stdout()
        .output()
        .await?;

    let mut text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = output.stderr_text();
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    Ok(CommitOutcome {
        success: output.success(),
        output: text,
    })
}

/// The full message of the commit at `HEAD` (for pre-filling the editor when amending).
/// `None` when there is no commit yet.
pub async fn last_commit_message(git: &Git, root: &Path) -> Result<Option<String>, GitError> {
    let output = git
        .command(["log", "-1", "--no-show-signature", "--format=%B", "HEAD"])
        .current_dir(root)
        .background()
        .output()
        .await?;
    if !output.success() {
        return Ok(None); // unborn branch
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned(),
    ))
}
