//! Accounts on hosting services as the credential helper keeps them, and the commit author.
//!
//! * Who is signed in to a host is read from the Windows credential store, where Git Credential
//!   Manager keeps what it signs in to under `git:https://host` (only the user name; the secret
//!   is never read).
//! * Signing in is `git credential fill` — the credential manager shows its own sign-in window
//!   — followed by `git credential approve`, which stores what it returned.
//! * Signing out is `git credential reject`, which makes the helper forget it.
//! * The commit author and line-ending conversion are read with `git config`; Facet only ever
//!   writes them to the repository's own config (`--local`), never the global one.

use std::path::Path;

use crate::{Git, GitError};

/// The user names the credential store holds for `https://{host}`, first the one stored for
/// the host itself. Empty: not signed in. Always empty off Windows.
pub fn stored_accounts(host: &str) -> Vec<String> {
    let mut accounts = Vec::new();
    for (target, user) in credential_targets() {
        let Some(url) = target.strip_prefix("git:https://") else {
            continue;
        };
        let authority = url.split('/').next().unwrap_or(url);
        let (user_in_url, target_host) = match authority.rsplit_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, authority),
        };
        if !target_host.eq_ignore_ascii_case(host) {
            continue;
        }
        let name = user_in_url.map_or(user, ToOwned::to_owned);
        if name.is_empty() || accounts.contains(&name) {
            continue;
        }
        if user_in_url.is_none() {
            accounts.insert(0, name);
        } else {
            accounts.push(name);
        }
    }
    accounts
}

/// Every generic credential in the user's Windows credential store: target name and user name.
#[cfg(windows)]
fn credential_targets() -> Vec<(String, String)> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{
        CredEnumerateW, CredFree, CREDENTIALW, CRED_ENUMERATE_FLAGS,
    };

    let read = |text: windows::core::PWSTR| {
        if text.is_null() {
            String::new()
        } else {
            // SAFETY: the credential store hands out NUL-terminated strings that live until
            // `CredFree`.
            unsafe { text.to_string() }.unwrap_or_default()
        }
    };
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: plain out-parameters; the list is freed below with `CredFree`.
    let found = unsafe {
        CredEnumerateW(
            PCWSTR::null(),
            Some(CRED_ENUMERATE_FLAGS(0)),
            &mut count,
            &mut list,
        )
    };
    if found.is_err() || list.is_null() {
        return Vec::new();
    }
    let mut targets = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        // SAFETY: `list` holds `count` valid credential pointers.
        let credential = unsafe { &**list.add(index) };
        targets.push((read(credential.TargetName), read(credential.UserName)));
    }
    // SAFETY: allocated by `CredEnumerateW`, freed once.
    unsafe { CredFree(list.cast()) };
    targets
}

#[cfg(not(windows))]
fn credential_targets() -> Vec<(String, String)> {
    Vec::new()
}

/// `key=value` lines of the credential protocol for `https://{host}` (and `username`).
fn request(host: &str, user: Option<&str>) -> String {
    let mut request = format!("protocol=https\nhost={host}\n");
    if let Some(user) = user {
        request.push_str(&format!("username={user}\n"));
    }
    request.push('\n');
    request
}

/// Sign in to `https://{host}` through the credential helper (Git Credential Manager shows its
/// own window) and keep the result. Returns the user name.
pub async fn sign_in(git: &Git, host: &str) -> Result<String, GitError> {
    let output = git
        .command(["credential", "fill"])
        .stdin(request(host, None))
        .run()
        .await?;
    let answer = String::from_utf8_lossy(&output.stdout).into_owned();
    let user = answer
        .lines()
        .find_map(|line| line.strip_prefix("username="))
        .unwrap_or_default()
        .to_owned();
    if !answer.lines().any(|line| line.starts_with("password=")) {
        return Err(GitError::Unsupported(format!(
            "No credential helper signed in to {host}. Install Git Credential Manager (it \
             comes with Git for Windows) and try again."
        )));
    }
    git.command(["credential", "approve"])
        .stdin(answer)
        .run()
        .await?;
    Ok(user)
}

/// Make the credential helper forget `user` (or every account) on `https://{host}`.
pub async fn sign_out(git: &Git, host: &str, user: Option<&str>) -> Result<(), GitError> {
    git.command(["credential", "reject"])
        .stdin(request(host, user))
        .run()
        .await
        .map(drop)
}

/// Who commits are made as, and where that is set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
    /// `global`, `local`, `system`… (from `git config --show-scope`) of the name; empty when
    /// it is not set anywhere.
    pub scope: String,
}

/// The commit author git uses in `root` (or outside any repository).
pub async fn identity(git: &Git, root: Option<&Path>) -> Result<Identity, GitError> {
    let mut identity = Identity::default();
    for key in ["user.name", "user.email"] {
        let mut command = git
            .command(["config", "--show-scope", "--get", key])
            .background();
        if let Some(root) = root {
            command = command.current_dir(root);
        }
        let output = command.output().await?;
        // Exit status 1: not set.
        if !output.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.trim_end_matches(['\r', '\n']);
        let (scope, value) = line.split_once('\t').unwrap_or(("", line));
        if key == "user.name" {
            identity.name = value.to_owned();
            identity.scope = scope.to_owned();
        } else {
            identity.email = value.to_owned();
        }
    }
    Ok(identity)
}

/// Set the commit author **for this repository only** (`git config --local`).
pub async fn set_local_identity(
    git: &Git,
    root: &Path,
    name: &str,
    email: &str,
) -> Result<(), GitError> {
    for (key, value) in [("user.name", name), ("user.email", email)] {
        git.command(["config", "--local", key, value])
            .current_dir(root)
            .run()
            .await?;
    }
    Ok(())
}

/// How a repository converts line endings (`core.autocrlf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndings {
    /// `false`: files are stored and checked out as they are.
    AsIs,
    /// `true`: LF in the repository, CRLF in the working tree.
    Windows,
    /// `input`: CRLF becomes LF on commit; checkout leaves LF.
    Input,
}

impl LineEndings {
    fn value(self) -> &'static str {
        match self {
            Self::AsIs => "false",
            Self::Windows => "true",
            Self::Input => "input",
        }
    }
}

/// `core.autocrlf` as it applies in `root` (any scope; unset is `AsIs`).
pub async fn line_endings(git: &Git, root: &Path) -> Result<LineEndings, GitError> {
    let output = git
        .command(["config", "--type=bool-or-str", "--get", "core.autocrlf"])
        .current_dir(root)
        .background()
        .output()
        .await?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(match text.trim() {
        "true" => LineEndings::Windows,
        "input" => LineEndings::Input,
        _ => LineEndings::AsIs,
    })
}

/// Set `core.autocrlf` **for this repository only**.
pub async fn set_line_endings(git: &Git, root: &Path, mode: LineEndings) -> Result<(), GitError> {
    git.command(["config", "--local", "core.autocrlf", mode.value()])
        .current_dir(root)
        .run()
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_end_with_a_blank_line() {
        assert_eq!(
            request("github.com", None),
            "protocol=https\nhost=github.com\n\n"
        );
        assert_eq!(
            request("gitlab.com", Some("me")),
            "protocol=https\nhost=gitlab.com\nusername=me\n\n"
        );
    }
}
