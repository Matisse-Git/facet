//! Integration tests for cloning and creating repositories.

mod common;

use std::sync::Arc;

use common::TestRepo;
use facet_git::{clone, discover, init, CancellationToken, GitError};

#[tokio::test]
async fn init_creates_the_folder_and_a_repository_on_the_chosen_branch() {
    let dir = TestRepo::empty_dir();
    let dest = dir.path.join("nested").join("fresh");
    init(&dir.git, &dest, "trunk").await.expect("init");
    let info = discover(&dir.git, &dest).await.expect("discover");
    assert!(!info.is_bare());
    let head = std::fs::read_to_string(dest.join(".git").join("HEAD")).expect("HEAD");
    assert_eq!(head.trim(), "ref: refs/heads/trunk");
}

#[tokio::test]
async fn clone_copies_a_local_repository_with_its_history() {
    let source = TestRepo::init().await;
    source.write("README.md", "hello\n");
    source.git_ok(&["add", "README.md"]).await;
    source.git_ok(&["commit", "-m", "first"]).await;

    let target = TestRepo::empty_dir();
    let dest = target.path.join("copy");
    let url = source.path.to_string_lossy().into_owned();
    clone(
        &target.git,
        &url,
        &dest,
        &CancellationToken::new(),
        Arc::new(|_: &str| {}),
    )
    .await
    .expect("clone");
    assert_eq!(
        std::fs::read_to_string(dest.join("README.md")).expect("README"),
        "hello\n"
    );
    let log = target
        .git
        .command(["log", "--format=%s"])
        .current_dir(&dest)
        .output()
        .await
        .expect("log");
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "first");
}

#[tokio::test]
async fn clone_into_a_folder_with_files_fails_with_gits_message() {
    let source = TestRepo::init().await;
    let target = TestRepo::empty_dir();
    target.write("taken/file.txt", "x");
    let result = clone(
        &target.git,
        &source.path.to_string_lossy(),
        &target.path.join("taken"),
        &CancellationToken::new(),
        Arc::new(|_: &str| {}),
    )
    .await;
    match result {
        Err(GitError::Failed { stderr, .. }) => {
            assert!(stderr.contains("already exists"), "{stderr}");
        }
        other => panic!("expected git to refuse, got {other:?}"),
    }
}
