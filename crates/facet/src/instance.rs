//! Single instance: the first Facet owns a named pipe; a second launch hands its path (if any)
//! to the first over that pipe and exits, and the first opens it and comes to the front.
//!
//! The pipe name is derived from the settings folder, so instances started with a different
//! `FACET_CONFIG_DIR` (tests, measurements, demos) stay independent, as do other Windows users.
//! Remote clients are rejected, and only the owner can write to a pipe it created.

use std::path::{Path, PathBuf};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

/// Message header: protocol name and version, then the UTF-8 path (empty: just focus).
const MAGIC: &[u8] = b"FACET-OPEN-1\0";
const MAX_MESSAGE: usize = 64 * 1024;

/// The first instance's end of the pipe, to hand to [`serve`].
pub struct Server {
    pipe: NamedPipeServer,
    name: String,
}

pub enum Claim {
    /// This is the first instance: serve the pipe with [`serve`].
    Primary(Server),
    /// Another instance took over; exit.
    Forwarded,
    /// Neither worked (the pipe could not be created or reached): run on its own.
    Alone,
}

/// The pipe for one settings folder.
pub fn pipe_name(config_dir: Option<&Path>) -> String {
    let key = config_dir
        .map(|dir| {
            dir.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_lowercase()
        })
        .unwrap_or_default();
    // FNV-1a: stable across builds, unlike the standard library's hasher.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!(r"\\.\pipe\facet-instance-{hash:016x}")
}

/// Become the first instance, or hand `open` to the one that is. Must run inside the tokio
/// runtime (`Runtime::enter`). Blocks briefly (at most ~2 s) only when another instance
/// exists but is busy accepting.
pub fn claim(name: &str, open: Option<&Path>) -> Claim {
    match ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(name)
    {
        Ok(pipe) => Claim::Primary(Server {
            pipe,
            name: name.to_owned(),
        }),
        Err(_) => {
            if forward(name, open).is_ok() {
                Claim::Forwarded
            } else {
                Claim::Alone
            }
        }
    }
}

/// Send `open` to the first instance.
fn forward(name: &str, open: Option<&Path>) -> std::io::Result<()> {
    use std::io::Write;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut pipe = loop {
        match std::fs::OpenOptions::new().write(true).open(name) {
            Ok(pipe) => break pipe,
            // ERROR_PIPE_BUSY (231), or ERROR_FILE_NOT_FOUND (2) between two server
            // instances: the first instance is about to accept again.
            Err(error)
                if matches!(error.raw_os_error(), Some(231 | 2))
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    };
    // This process was just started by the user, so it may hand the foreground on.
    crate::platform::allow_any_foreground();
    let mut message = MAGIC.to_vec();
    if let Some(path) = open {
        message.extend_from_slice(path.to_string_lossy().as_bytes());
    }
    pipe.write_all(&message)?;
    pipe.flush()
}

/// Accept hand-overs on the current tokio runtime; `on_open` gets each forwarded path
/// (`None`: the second launch had no path, just bring the window up).
pub fn serve(first: Server, on_open: impl Fn(Option<PathBuf>) + Send + Sync + 'static) {
    let on_open = std::sync::Arc::new(on_open);
    tokio::spawn(async move {
        let Server {
            pipe: mut server,
            name,
        } = first;
        loop {
            if server.connect().await.is_err() {
                continue;
            }
            let connected = server;
            server = match ServerOptions::new()
                .reject_remote_clients(true)
                .create(&name)
            {
                Ok(next) => next,
                Err(_) => return,
            };
            let on_open = on_open.clone();
            tokio::spawn(async move {
                if let Some(open) = read_message(connected).await {
                    on_open(open);
                }
            });
        }
    });
}

async fn read_message(mut pipe: NamedPipeServer) -> Option<Option<PathBuf>> {
    use tokio::io::AsyncReadExt;
    let mut message = Vec::new();
    let mut buf = [0u8; 4096];
    let read = async {
        loop {
            let n = pipe.read(&mut buf).await.ok()?;
            if n == 0 {
                return Some(());
            }
            if message.len() + n > MAX_MESSAGE {
                return None;
            }
            message.extend_from_slice(&buf[..n]);
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), read)
        .await
        .ok()??;
    parse(&message)
}

/// The forwarded path, or `None` for a message that isn't ours.
fn parse(message: &[u8]) -> Option<Option<PathBuf>> {
    let path = message.strip_prefix(MAGIC)?;
    let path = std::str::from_utf8(path).ok()?;
    Some((!path.is_empty()).then(|| PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names_follow_the_settings_folder() {
        let a = pipe_name(Some(Path::new(r"C:\Users\Someone\AppData\Roaming\Facet")));
        assert_eq!(
            a,
            pipe_name(Some(Path::new(r"c:\users\someone\appdata\roaming\facet\")))
        );
        assert_ne!(a, pipe_name(Some(Path::new(r"D:\tmp\facet-bench"))));
        assert!(a.starts_with(r"\\.\pipe\facet-instance-"));
        assert_eq!(a.len(), r"\\.\pipe\facet-instance-".len() + 16);
    }

    #[test]
    fn messages_carry_an_optional_path() {
        assert_eq!(parse(b"FACET-OPEN-1\0"), Some(None));
        assert_eq!(
            parse("FACET-OPEN-1\0D:\\Spiel ü".as_bytes()),
            Some(Some(PathBuf::from("D:\\Spiel ü")))
        );
        assert_eq!(parse(b"hello"), None);
    }

    #[test]
    fn a_second_launch_is_handed_to_the_first() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let name = pipe_name(Some(
            &std::env::temp_dir().join(format!("facet-instance-test-{}", std::process::id())),
        ));
        let Claim::Primary(server) = claim(&name, None) else {
            panic!("the first claim should own the pipe");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        serve(server, move |path| {
            let _ = tx.send(path);
        });
        for open in [Some(Path::new(r"D:\Games\Shooter")), None] {
            assert!(matches!(claim(&name, open), Claim::Forwarded));
            let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert_eq!(got.as_deref(), open);
        }
    }
}
