//! Integration tests for repository discovery and HEAD inspection against real temp repos.

mod common;

use common::{same_path, TestRepo, AUTHOR};
use facet_git::{discover, head, version, GitError, Head};

#[tokio::test]
async fn version_is_reported() {
    let repo = TestRepo::empty_dir();
    let v = version(&repo.git).await.expect("git version");
    assert!(v.major >= 2, "unexpectedly old git: {v}");
}

#[tokio::test]
async fn discover_rejects_a_plain_directory() {
    let dir = TestRepo::empty_dir();
    match discover(&dir.git, &dir.path).await {
        Err(GitError::NotARepository { path, detail }) => {
            assert_eq!(path, dir.path);
            assert!(!detail.is_empty(), "git's own message should be kept");
        }
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[tokio::test]
async fn discover_reports_missing_folders_and_files_without_running_git() {
    let dir = TestRepo::empty_dir();
    let before = dir.log.len();

    let missing = dir.path.join("does-not-exist");
    let result = discover(&dir.git, &missing).await;
    assert!(
        matches!(&result, Err(GitError::PathNotFound { path }) if *path == missing),
        "{result:?}"
    );

    dir.write("notes.txt", "x");
    let file = dir.path.join("notes.txt");
    let result = discover(&dir.git, &file).await;
    assert!(
        matches!(result, Err(GitError::PathNotFound { .. })),
        "{result:?}"
    );

    assert_eq!(
        dir.log.len(),
        before,
        "git must not be spawned for a bad path"
    );
}

#[tokio::test]
async fn discover_finds_the_root_from_the_root() {
    let repo = TestRepo::init().await;
    let info = discover(&repo.git, &repo.path).await.expect("discover");
    assert!(!info.is_bare());
    assert!(same_path(info.workdir.as_deref().unwrap(), &repo.path));
    assert!(same_path(&info.git_dir, &repo.path.join(".git")));
    assert_eq!(info.name(), "work");
}

#[tokio::test]
async fn discover_finds_the_root_from_a_subdirectory() {
    let repo = TestRepo::init().await;
    repo.write("Content/Maps/level.txt", "x");
    let nested = repo.path.join("Content").join("Maps");
    let info = discover(&repo.git, &nested).await.expect("discover");
    assert!(same_path(info.workdir.as_deref().unwrap(), &repo.path));
}

#[tokio::test]
async fn discover_accepts_the_dot_git_directory() {
    let repo = TestRepo::init().await;
    let info = discover(&repo.git, &repo.path.join(".git"))
        .await
        .expect("discover");
    assert!(same_path(info.workdir.as_deref().unwrap(), &repo.path));
}

#[tokio::test]
async fn discover_handles_spaces_and_unicode_in_paths() {
    let repo = TestRepo::init().await;
    let nested = repo.path.join("Mes Niveaux ✓").join("sous dossier");
    std::fs::create_dir_all(&nested).unwrap();
    let info = discover(&repo.git, &nested).await.expect("discover");
    assert!(same_path(info.workdir.as_deref().unwrap(), &repo.path));
}

#[tokio::test]
async fn discover_reports_bare_repositories() {
    let repo = TestRepo::init_bare().await;
    let info = discover(&repo.git, &repo.path).await.expect("discover");
    assert!(info.is_bare());
    assert!(info.workdir.is_none());
    assert!(same_path(&info.git_dir, &repo.path));
}

#[tokio::test]
async fn head_on_an_unborn_branch() {
    let repo = TestRepo::init().await;
    let state = head(&repo.git, &repo.path).await.expect("head");
    assert_eq!(
        state,
        Head::Branch {
            name: "main".into(),
            commit: None
        }
    );
    assert!(state.commit().is_none());
}

#[tokio::test]
async fn head_on_a_branch_with_commits() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "First commit").await;
    repo.commit_file("a.txt", "two\n", "Second: with \"quotes\" & ünïcode")
        .await;
    repo.git_ok(&["switch", "-c", "feature/lighting"]).await;

    match head(&repo.git, &repo.path).await.expect("head") {
        Head::Branch {
            name,
            commit: Some(commit),
        } => {
            assert_eq!(name, "feature/lighting");
            assert_eq!(commit.subject, "Second: with \"quotes\" & ünïcode");
            assert_eq!(commit.author, AUTHOR);
            assert_eq!(commit.author_time, 1_700_000_000);
            assert_eq!(commit.oid.len(), 40);
            assert!(commit.oid.starts_with(&commit.short_oid));
        }
        other => panic!("expected a branch with a commit, got {other:?}"),
    }
}

#[tokio::test]
async fn head_when_detached() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "First commit").await;
    repo.commit_file("a.txt", "two\n", "Second commit").await;
    repo.git_ok(&["checkout", "--detach", "HEAD~1"]).await;

    match head(&repo.git, &repo.path).await.expect("head") {
        Head::Detached { commit } => assert_eq!(commit.subject, "First commit"),
        other => panic!("expected detached HEAD, got {other:?}"),
    }
}

#[tokio::test]
async fn head_ignores_show_signature_config() {
    // `log.showSignature=true` makes plain `git log` print GPG lines before each record.
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "Signed-ish").await;
    repo.git_ok(&["config", "log.showSignature", "true"]).await;
    let state = head(&repo.git, &repo.path).await.expect("head");
    assert_eq!(state.commit().unwrap().subject, "Signed-ish");
}

#[tokio::test]
async fn head_survives_color_always_config() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "Colorful").await;
    repo.git_ok(&["config", "color.ui", "always"]).await;
    let state = head(&repo.git, &repo.path).await.expect("head");
    assert_eq!(state.commit().unwrap().subject, "Colorful");
}
