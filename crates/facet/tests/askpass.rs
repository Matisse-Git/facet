//! End to end credentials (G7): real `git` talks to a local HTTP server that demands Basic
//! authentication, runs Facet's own executable as `GIT_ASKPASS`, and the executable relays the
//! prompts over the pipe to this test, which answers them. The server then sees exactly the
//! credentials typed "in the app".

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use facet_git::askpass;

/// Answers every request with 401 until one carries an `Authorization` header, which it
/// records (and then answers 403 so git gives up quickly).
fn serve(listener: TcpListener, seen: Arc<Mutex<Vec<String>>>) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut authorization = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("authorization") {
                    authorization = Some(value.trim().to_owned());
                }
            }
        }
        let response = match authorization {
            Some(value) => {
                seen.lock().unwrap().push(value);
                "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            }
            None => {
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"facet\"\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            }
        };
        let _ = stream.write_all(response.as_bytes());
    }
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn git_asks_facet_for_credentials() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    std::thread::spawn({
        let seen = seen.clone();
        move || serve(listener, seen)
    });

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let env = askpass::start(Path::new(env!("CARGO_BIN_EXE_facet")), {
        let prompts = prompts.clone();
        move |request| {
            prompts.lock().unwrap().push(request.prompt.clone());
            let answer = if request.prompt.starts_with("Username") {
                "octocat"
            } else {
                "s3cret pass"
            };
            request.answer(Some(answer.to_owned()));
        }
    })
    .unwrap();

    let output = Command::new("git")
        .args([
            // No credential helper (GCM would show its own window); per call only.
            "-c",
            "credential.helper=",
            "ls-remote",
            &format!("http://127.0.0.1:{port}/repo.git"),
        ])
        .envs(env.vars.iter().map(|(k, v)| (k, v)))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GCM_INTERACTIVE")
        .output()
        .unwrap();
    assert!(!output.status.success(), "the server never lets git in");

    let prompts = prompts.lock().unwrap().clone();
    assert_eq!(
        prompts.len(),
        2,
        "{prompts:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        prompts[0].starts_with("Username for 'http://127.0.0.1"),
        "{prompts:?}"
    );
    assert!(
        prompts[1].starts_with("Password for 'http://octocat@127.0.0.1"),
        "{prompts:?}"
    );
    let expected = format!("Basic {}", base64(b"octocat:s3cret pass"));
    assert_eq!(seen.lock().unwrap().as_slice(), [expected]);
}

#[test]
fn base64_matches_the_rfc_examples() {
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
}
