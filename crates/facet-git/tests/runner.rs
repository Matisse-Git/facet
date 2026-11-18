//! Integration tests for the git process runner: output capture, streaming, stdin, failure
//! mapping, cancellation and the command log.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::TestRepo;
use facet_git::{CancellationToken, CommandLog, CommandOutcome, Git, GitError};

#[tokio::test]
async fn output_captures_stdout_and_status() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "hello\n", "init").await;

    let ok = repo
        .git
        .command(["rev-parse", "--verify", "HEAD"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert!(ok.success());
    assert_eq!(ok.stdout.len(), 41, "40 hex digits + newline");

    let missing = repo
        .git
        .command(["rev-parse", "--verify", "-q", "refs/heads/nope"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert_eq!(missing.status, 1);
    assert!(missing.stdout.is_empty());
}

#[tokio::test]
async fn run_maps_failure_to_an_error_with_gits_message() {
    let repo = TestRepo::init().await;
    let err = repo
        .git
        .command(["rev-parse", "--verify", "refs/heads/nope"])
        .current_dir(&repo.path)
        .run()
        .await
        .unwrap_err();
    match err {
        GitError::Failed {
            command,
            code,
            stderr,
        } => {
            assert!(command.contains("rev-parse --verify refs/heads/nope"));
            assert_eq!(code, 128);
            assert!(!stderr.is_empty());
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_delivers_the_same_bytes_as_output() {
    let repo = TestRepo::init().await;
    // Enough data to span several read chunks.
    let big = "0123456789abcdef\n".repeat(20_000);
    repo.commit_file("big.txt", &big, "big file").await;

    let buffered = repo
        .git
        .command(["cat-file", "blob", "HEAD:big.txt"])
        .current_dir(&repo.path)
        .run()
        .await
        .unwrap();

    let mut streamed = Vec::new();
    let mut chunks = 0usize;
    let result = repo
        .git
        .command(["cat-file", "blob", "HEAD:big.txt"])
        .current_dir(&repo.path)
        .stream(|chunk| {
            chunks += 1;
            streamed.extend_from_slice(chunk);
        })
        .await
        .unwrap();

    assert!(result.stdout.is_empty(), "streamed output is not buffered");
    assert_eq!(streamed, buffered.stdout);
    assert_eq!(streamed, big.as_bytes());
    assert!(chunks >= 1);
}

#[tokio::test]
async fn stdin_is_fed_to_the_process() {
    let repo = TestRepo::init().await;
    let output = repo
        .git
        .command(["hash-object", "--stdin"])
        .current_dir(&repo.path)
        .stdin("hello\n")
        .run()
        .await
        .unwrap();
    // The well-known blob id of "hello\n".
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
}

#[tokio::test]
async fn cancellation_kills_a_running_command() {
    let repo = TestRepo::init().await;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        canceller.cancel();
    });

    // `hash-object --stdin` blocks until stdin closes; an alias running `sleep` keeps the
    // process alive without needing any input.
    let started = Instant::now();
    let result = repo
        .git
        .command(["-c", "alias.nap=!sleep 5", "nap"])
        .current_dir(&repo.path)
        .cancel_with(&token)
        .output()
        .await;

    assert!(matches!(result, Err(GitError::Cancelled)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "cancel should not wait for the command to finish"
    );
    let last = repo.log.snapshot().pop().unwrap();
    assert_eq!(last.outcome, CommandOutcome::Cancelled);
}

/// G6: killing `git.exe` alone leaves its children (hooks, `sh`, ssh, credential helpers)
/// running. Commands run in a Job Object, so cancelling ends the whole process tree.
#[tokio::test]
async fn cancellation_kills_the_whole_process_tree() {
    let repo = TestRepo::init().await;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        canceller.cancel();
    });

    // git → sh → sleep; the shell writes the marker only if it outlives the cancel.
    let result = repo
        .git
        .command([
            "-c",
            "alias.slow=!sleep 2 && echo survived > marker.txt",
            "slow",
        ])
        .current_dir(&repo.path)
        .cancel_with(&token)
        .output()
        .await;
    assert!(matches!(result, Err(GitError::Cancelled)), "{result:?}");

    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(
        !repo.exists("marker.txt"),
        "a child of git kept running after the command was cancelled"
    );
}

/// Dropping a command's future (a closed tab, an exiting app) ends its process tree too.
#[tokio::test]
async fn dropping_a_running_command_kills_its_process_tree() {
    let repo = TestRepo::init().await;
    let command = repo
        .git
        .command([
            "-c",
            "alias.slow=!sleep 2 && echo survived > marker.txt",
            "slow",
        ])
        .current_dir(&repo.path)
        .output();
    let timed_out = tokio::time::timeout(Duration::from_millis(400), command).await;
    assert!(timed_out.is_err(), "the command should still be running");

    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(!repo.exists("marker.txt"));
}

/// A command that finishes normally does not take down what it deliberately left running
/// in the background (`git gc --auto` detaches this way after a fetch).
#[tokio::test]
async fn a_finished_command_leaves_detached_children_alone() {
    let repo = TestRepo::init().await;
    repo.git
        .command([
            "-c",
            "alias.detach=!(sleep 1 && echo done > marker.txt) </dev/null >/dev/null 2>&1 &",
            "detach",
        ])
        .current_dir(&repo.path)
        .run()
        .await
        .unwrap();
    let mut found = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if repo.exists("marker.txt") {
            found = true;
            break;
        }
    }
    assert!(found, "the detached child was killed when git exited");
}

#[tokio::test]
async fn an_already_cancelled_token_stops_immediately() {
    let repo = TestRepo::init().await;
    let token = CancellationToken::new();
    token.cancel();
    let result = repo
        .git
        .command(["status", "--porcelain=v2", "-z"])
        .current_dir(&repo.path)
        .cancel_with(&token)
        .output()
        .await;
    assert!(matches!(result, Err(GitError::Cancelled)));
}

#[tokio::test]
async fn a_missing_executable_is_a_spawn_error_and_is_logged() {
    let log = Arc::new(CommandLog::new(8));
    let git = Git::new("facet-no-such-git-executable", log.clone());
    let err = git.command(["version"]).output().await.unwrap_err();
    assert!(matches!(err, GitError::Spawn { .. }), "{err:?}");

    let records = log.snapshot();
    assert_eq!(records.len(), 1);
    assert!(matches!(records[0].outcome, CommandOutcome::SpawnFailed(_)));
}

#[tokio::test]
async fn every_command_is_logged_exactly_as_run() {
    let repo = TestRepo::init().await;
    let before = repo.log.len();
    let _ = repo
        .git
        .command(["rev-parse", "--verify", "-q", "HEAD^{commit}"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();

    let records = repo.log.snapshot();
    assert_eq!(records.len(), before + 1);
    let record = records.last().unwrap();
    assert_eq!(
        record.command_line,
        "git --no-pager -c color.ui=never rev-parse --verify -q HEAD^{commit}"
    );
    assert_eq!(record.cwd.as_deref(), Some(repo.path.as_path()));
    assert_eq!(record.outcome, CommandOutcome::Exited(1), "unborn HEAD");
    assert!(record.duration.is_some());
}

#[tokio::test]
async fn background_commands_do_not_take_optional_locks() {
    // With GIT_OPTIONAL_LOCKS=0, `status` must not rewrite the index even when it could
    // refresh stat info. We assert the observable contract: the index file is untouched.
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "init").await;
    // Touch the file so a normal `status` would want to refresh the index.
    std::thread::sleep(Duration::from_millis(1100));
    repo.write("a.txt", "one\n");

    let index = repo.path.join(".git").join("index");
    let before = std::fs::metadata(&index).unwrap().modified().unwrap();
    repo.git
        .command(["status", "--porcelain=v2", "-z"])
        .current_dir(&repo.path)
        .background()
        .run()
        .await
        .unwrap();
    let after = std::fs::metadata(&index).unwrap().modified().unwrap();
    assert_eq!(before, after, "background status must not write the index");
}

#[tokio::test]
async fn another_executable_keeps_the_log_and_environment() {
    let repo = TestRepo::init().await;
    let marked = repo.git.with_env("FACET_TEST_MARK", "kept");

    // A path that does not exist: `git version` fails instead of running some other git.
    let missing = marked.with_exe(repo.path.join("no-such-git.exe"));
    assert!(facet_git::version(&missing).await.is_err());

    // The real one again (as a full path, the way settings store it): the environment set
    // before the switch still reaches git.
    let found = String::from_utf8(
        std::process::Command::new("where")
            .arg("git")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let exe = found.lines().next().unwrap().trim().to_owned();
    let real = missing.with_exe(&exe);
    assert_eq!(real.exe(), std::path::Path::new(&exe));
    assert!(facet_git::version(&real).await.is_ok());
    let env = real
        .command(["-c", "alias.mark=!echo $FACET_TEST_MARK", "mark"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&env.stdout).trim(), "kept");
    // Every handle writes to the same command log.
    assert!(Arc::ptr_eq(real.log(), repo.git.log()));
}
