//! Integration tests for commit / amend, including hooks (which are never bypassed).

mod common;

use common::{TestRepo, AUTHOR};
use facet_git::{commit, commit_summary, last_commit_message};

async fn log_subjects(repo: &TestRepo) -> Vec<String> {
    String::from_utf8(repo.git_ok(&["log", "--format=%s"]).await)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn commit_then_amend() {
    let repo = TestRepo::init().await;
    assert_eq!(
        last_commit_message(&repo.git, &repo.path).await.unwrap(),
        None
    );

    repo.write("a.txt", "a\n");
    repo.git_ok(&["add", "a.txt"]).await;
    let message = "Add \"a\" — first commit\n\nA body with a # hash line:\n#42 stays\n";
    let outcome = commit(&repo.git, &repo.path, message, false).await.unwrap();
    assert!(outcome.success, "{}", outcome.output);
    assert_eq!(outcome.output, "");

    let summary = commit_summary(&repo.git, &repo.path).await.unwrap();
    assert_eq!(summary.subject, "Add \"a\" — first commit");
    assert_eq!(summary.author, AUTHOR);
    // The message is kept as written (lines starting with `#` are not comments here).
    assert_eq!(
        last_commit_message(&repo.git, &repo.path)
            .await
            .unwrap()
            .unwrap(),
        message.trim_end()
    );
    assert!(repo.status().await.entries.is_empty());

    // Amend: new message plus one more staged file, still a single commit.
    repo.write("b.txt", "b\n");
    repo.git_ok(&["add", "b.txt"]).await;
    let outcome = commit(&repo.git, &repo.path, "Add a and b", true)
        .await
        .unwrap();
    assert!(outcome.success, "{}", outcome.output);
    assert_eq!(log_subjects(&repo).await, ["Add a and b"]);
    let files = String::from_utf8(repo.git_ok(&["ls-tree", "--name-only", "HEAD"]).await).unwrap();
    assert_eq!(files, "a.txt\nb.txt\n");

    // Amending only the message, with nothing staged, also works.
    let outcome = commit(&repo.git, &repo.path, "Reworded", true)
        .await
        .unwrap();
    assert!(outcome.success, "{}", outcome.output);
    assert_eq!(log_subjects(&repo).await, ["Reworded"]);
}

#[tokio::test]
async fn japanese_paths_and_messages_round_trip() {
    let repo = TestRepo::init().await;
    repo.write("ドキュメント/説明.md", "日本語のテキスト\n");

    let status = repo.status().await;
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].path, "ドキュメント/説明.md");

    repo.git_ok(&["add", "--all"]).await;
    let message = "説明を追加\n\n本文：日本語の説明です。\n";
    let outcome = commit(&repo.git, &repo.path, message, false).await.unwrap();
    assert!(outcome.success, "{}", outcome.output);

    let summary = commit_summary(&repo.git, &repo.path).await.unwrap();
    assert_eq!(summary.subject, "説明を追加");
    assert_eq!(
        last_commit_message(&repo.git, &repo.path)
            .await
            .unwrap()
            .unwrap(),
        message.trim_end()
    );
    assert!(repo.status().await.entries.is_empty());
}

#[tokio::test]
async fn nothing_staged_is_a_rejected_commit_not_an_error() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "a\n", "base").await;
    let outcome = commit(&repo.git, &repo.path, "empty", false).await.unwrap();
    assert!(!outcome.success);
    assert_eq!(log_subjects(&repo).await, ["base"]);
}

#[tokio::test]
async fn a_failing_hook_blocks_the_commit_and_its_output_is_surfaced() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "a\n", "base").await;
    repo.hook(
        "pre-commit",
        "echo 'lint: 2 problems found'\necho 'trailing whitespace in a.txt' >&2\nexit 1",
    );

    repo.write("a.txt", "a \n");
    repo.git_ok(&["add", "a.txt"]).await;
    let outcome = commit(&repo.git, &repo.path, "blocked", false)
        .await
        .unwrap();

    assert!(!outcome.success);
    assert!(
        outcome.output.contains("lint: 2 problems found"),
        "{}",
        outcome.output
    );
    assert!(
        outcome.output.contains("trailing whitespace in a.txt"),
        "{}",
        outcome.output
    );
    assert_eq!(log_subjects(&repo).await, ["base"]);
    // Still staged: nothing was lost.
    assert!(repo.status().await.entries[0].is_staged());

    // The command log shows what the hook said (stdout and stderr), and no bypass flag.
    let record = repo
        .log
        .snapshot()
        .into_iter()
        .rev()
        .find(|record| record.command_line.contains(" commit "))
        .unwrap();
    assert!(record.stderr.contains("lint: 2 problems found"));
    assert!(record.stderr.contains("trailing whitespace in a.txt"));
    assert!(!record.command_line.contains("no-verify"));
}

#[tokio::test]
async fn hooks_run_and_may_rewrite_the_message() {
    let repo = TestRepo::init().await;
    repo.hook("pre-commit", "echo 'formatting ok'");
    repo.hook("commit-msg", "echo 'Reviewed-by: hook' >> \"$1\"");

    repo.write("a.txt", "a\n");
    repo.git_ok(&["add", "a.txt"]).await;
    let outcome = commit(&repo.git, &repo.path, "Subject", false)
        .await
        .unwrap();
    assert!(outcome.success, "{}", outcome.output);
    assert!(outcome.output.contains("formatting ok"));
    assert_eq!(
        last_commit_message(&repo.git, &repo.path)
            .await
            .unwrap()
            .unwrap(),
        "Subject\nReviewed-by: hook"
    );
}
