//! Credential prompts for network operations (PLAN.md, G7).
//!
//! Facet runs git with `GIT_TERMINAL_PROMPT=0`: there is no console to type into. Two things
//! still let git ask for credentials:
//!
//! * Git Credential Manager (bundled with Git for Windows) shows its own windows. Facet sets
//!   `GCM_MODAL_PARENTHWND` so they open in front of, and modal to, Facet's window.
//! * Anything else (a username prompt without a credential helper, an SSH key passphrase, an
//!   unknown SSH host key) goes to `GIT_ASKPASS` / `SSH_ASKPASS`. Facet's own executable is
//!   that program: started that way it connects back to the running app over a named pipe,
//!   the app shows the prompt in its own dialog, and the answer is printed for git.
//!
//! The pipe is local-only and every request must carry a random per-session token that only
//! git's children inherit, so another process can neither read answers nor fake prompts.

use std::ffi::OsString;
use std::path::Path;

/// Environment variables that make the helper connect back to the app.
pub const PIPE_ENV: &str = "FACET_ASKPASS_PIPE";
pub const TOKEN_ENV: &str = "FACET_ASKPASS_TOKEN";

/// Upper bound for one request (token + prompt); prompts are a line of text.
const MAX_REQUEST: usize = 64 * 1024;

/// A prompt from git waiting for the person's answer. Dropping it without answering cancels
/// the prompt (git then fails with its own "could not read Username" message).
pub struct AskpassRequest {
    pub prompt: String,
    reply: tokio::sync::oneshot::Sender<Option<String>>,
}

impl AskpassRequest {
    /// Does the prompt ask for something secret (shown masked)?
    pub fn is_secret(&self) -> bool {
        is_secret_prompt(&self.prompt)
    }

    /// Answer the prompt; `None` cancels it.
    pub fn answer(self, answer: Option<String>) {
        let _ = self.reply.send(answer);
    }
}

impl std::fmt::Debug for AskpassRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskpassRequest")
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

/// Passwords, passphrases, PINs and tokens are masked; usernames and host key questions
/// ("Are you sure you want to continue connecting (yes/no)?") are not.
pub fn is_secret_prompt(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    ["password", "passphrase", "pin", "token", "secret"]
        .iter()
        .any(|word| lower.contains(word))
}

/// The environment git needs to use the helper: set it on the [`crate::Git`] handle.
#[derive(Debug, Clone)]
pub struct AskpassEnv {
    pub vars: Vec<(OsString, OsString)>,
}

/// A random 128-bit value in hex, from the OS-seeded hasher keys of the standard library.
fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for salt in 0..2u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

/// Constant-time comparison, so the token can't be guessed byte by byte.
fn same_token(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Start the prompt server on the current tokio runtime. `helper` is the executable git
/// should run (Facet itself); `on_request` is called on a worker for every prompt.
#[cfg(windows)]
pub fn start(
    helper: &Path,
    on_request: impl Fn(AskpassRequest) + Send + Sync + 'static,
) -> std::io::Result<AskpassEnv> {
    use std::sync::Arc;
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe = format!(
        r"\\.\pipe\facet-askpass-{}-{}",
        std::process::id(),
        random_hex()
    );
    let token = random_hex();
    let first = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&pipe)?;

    let on_request = Arc::new(on_request);
    let (server_pipe, server_token) = (pipe.clone(), token.clone());
    tokio::spawn(async move {
        let mut server = first;
        loop {
            if server.connect().await.is_err() {
                continue;
            }
            let connected = server;
            // The next client can connect while this one is being served.
            server = match ServerOptions::new()
                .reject_remote_clients(true)
                .create(&server_pipe)
            {
                Ok(next) => next,
                Err(_) => return,
            };
            let (token, on_request) = (server_token.clone(), on_request.clone());
            tokio::spawn(async move {
                let _ = serve(connected, &token, &*on_request).await;
            });
        }
    });

    // ssh is an MSYS program: give it forward slashes.
    let ssh_helper = helper.to_string_lossy().replace('\\', "/");
    Ok(AskpassEnv {
        vars: vec![
            ("GIT_ASKPASS".into(), helper.as_os_str().to_owned()),
            ("SSH_ASKPASS".into(), ssh_helper.into()),
            // Use the helper even though there is no X display (OpenSSH 8.4+).
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (PIPE_ENV.into(), pipe.into()),
            (TOKEN_ENV.into(), token.into()),
        ],
    })
}

#[cfg(windows)]
async fn serve(
    mut pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    token: &str,
    on_request: &(dyn Fn(AskpassRequest) + Send + Sync),
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Request: token NUL prompt NUL.
    let mut request = Vec::new();
    let mut buf = [0u8; 4096];
    while request.iter().filter(|byte| **byte == 0).count() < 2 {
        let n = pipe.read(&mut buf).await?;
        if n == 0 || request.len() + n > MAX_REQUEST {
            return Ok(());
        }
        request.extend_from_slice(&buf[..n]);
    }
    let mut parts = request.split(|byte| *byte == 0);
    let (Some(given), Some(prompt)) = (parts.next(), parts.next()) else {
        return Ok(());
    };
    if !same_token(given, token.as_bytes()) {
        return Ok(());
    }
    let (reply, answer) = tokio::sync::oneshot::channel();
    on_request(AskpassRequest {
        prompt: String::from_utf8_lossy(prompt).trim().to_owned(),
        reply,
    });
    // Reply: status (1 answered, 0 cancelled), u32 length (little endian), answer.
    let answer = answer.await.ok().flatten();
    let mut response = vec![u8::from(answer.is_some())];
    let text = answer.unwrap_or_default();
    response.extend_from_slice(&(text.len() as u32).to_le_bytes());
    response.extend_from_slice(text.as_bytes());
    pipe.write_all(&response).await?;
    // Closing the server end could discard bytes the client has not read yet: wait for the
    // client to hang up first.
    let mut rest = [0u8; 1];
    let _ = pipe.read(&mut rest).await;
    Ok(())
}

/// Ask the running app through the pipe; blocking. `Ok(None)` means the person cancelled.
pub fn ask(pipe: &str, token: &str, prompt: &str) -> std::io::Result<Option<String>> {
    use std::io::{Read, Write};

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut stream = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe)
        {
            Ok(stream) => break stream,
            // ERROR_PIPE_BUSY: another prompt is being connected; try again shortly.
            Err(error)
                if error.raw_os_error() == Some(231) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    };
    let mut request = Vec::with_capacity(token.len() + prompt.len() + 2);
    request.extend_from_slice(token.as_bytes());
    request.push(0);
    request.extend_from_slice(prompt.as_bytes());
    request.push(0);
    stream.write_all(&request)?;
    stream.flush()?;

    let mut header = [0u8; 5];
    stream.read_exact(&mut header)?;
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_REQUEST {
        return Err(std::io::Error::other("askpass answer too long"));
    }
    let mut answer = vec![0u8; len];
    stream.read_exact(&mut answer)?;
    Ok((header[0] == 1).then(|| String::from_utf8_lossy(&answer).into_owned()))
}

/// Entry point for the helper: if this process was started by git as its askpass program,
/// answer the prompt in `args` and return the exit code. `None` means "not the helper".
pub fn helper_main(args: &[OsString]) -> Option<i32> {
    let pipe = std::env::var(PIPE_ENV).ok()?;
    let token = std::env::var(TOKEN_ENV).ok()?;
    let prompt = args
        .first()
        .map(|arg| arg.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(match ask(&pipe, &token, &prompt) {
        Ok(Some(answer)) => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let written = writeln!(out, "{answer}").and_then(|()| out.flush());
            i32::from(written.is_err())
        }
        Ok(None) | Err(_) => 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_prompts_are_recognised() {
        assert!(is_secret_prompt("Password for 'https://me@github.com': "));
        assert!(is_secret_prompt(
            "Enter passphrase for key '/c/Users/me/.ssh/id_ed25519': "
        ));
        assert!(!is_secret_prompt("Username for 'https://github.com': "));
        assert!(!is_secret_prompt(
            "Are you sure you want to continue connecting (yes/no/[fingerprint])?"
        ));
    }

    #[test]
    fn tokens_are_random_and_compared_exactly() {
        let (a, b) = (random_hex(), random_hex());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(same_token(a.as_bytes(), a.as_bytes()));
        assert!(!same_token(a.as_bytes(), b.as_bytes()));
        assert!(!same_token(a.as_bytes(), &a.as_bytes()[1..]));
    }
}
