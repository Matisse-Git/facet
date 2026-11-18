//! The askpass bridge (G7): prompts travel from the helper process to the app and back over
//! an authenticated local pipe. The end-to-end test with real git lives in the `facet`
//! crate (`tests/askpass.rs`), which can run Facet's own executable as the helper.

use std::ffi::OsString;
use std::path::Path;

use facet_git::askpass::{self, AskpassEnv, PIPE_ENV, TOKEN_ENV};

fn var<'a>(env: &'a AskpassEnv, name: &str) -> &'a OsString {
    &env.vars
        .iter()
        .find(|(key, _)| key == name)
        .unwrap_or_else(|| panic!("{name} not set"))
        .1
}

#[tokio::test(flavor = "multi_thread")]
async fn prompts_round_trip_and_cancel() {
    let env = askpass::start(Path::new(r"C:\Facet\facet.exe"), |request| {
        if request.prompt.starts_with("Username") {
            request.answer(Some("octocat".to_owned()));
        } else if request.is_secret() {
            request.answer(Some("hunter2 with spaces".to_owned()));
        } else {
            request.answer(None);
        }
    })
    .unwrap();
    assert_eq!(var(&env, "GIT_ASKPASS"), r"C:\Facet\facet.exe");
    assert_eq!(var(&env, "SSH_ASKPASS"), "C:/Facet/facet.exe");
    assert_eq!(var(&env, "SSH_ASKPASS_REQUIRE"), "force");
    let pipe = var(&env, PIPE_ENV).to_string_lossy().into_owned();
    let token = var(&env, TOKEN_ENV).to_string_lossy().into_owned();

    let ask = |prompt: &'static str, token: String| {
        let pipe = pipe.clone();
        tokio::task::spawn_blocking(move || askpass::ask(&pipe, &token, prompt))
    };
    let (user, password, question) = tokio::join!(
        ask("Username for 'https://example.com': ", token.clone()),
        ask(
            "Password for 'https://octocat@example.com': ",
            token.clone()
        ),
        ask("Continue connecting (yes/no)?", token.clone()),
    );
    assert_eq!(user.unwrap().unwrap().as_deref(), Some("octocat"));
    assert_eq!(
        password.unwrap().unwrap().as_deref(),
        Some("hunter2 with spaces")
    );
    assert_eq!(question.unwrap().unwrap(), None, "dismissed prompt");

    // Without the token the server hangs up without answering.
    let forged = ask("Password for 'https://example.com': ", "0".repeat(32))
        .await
        .unwrap();
    assert!(forged.is_err(), "{forged:?}");
}
