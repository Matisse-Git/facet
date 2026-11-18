//! History: the streamed log, the graph built from it, and commit details.

mod common;

use common::TestRepo;
use facet_git::graph::{ascii, layout};
use facet_git::parse::LogCommit;
use facet_git::{commit_detail, stream_log, CancellationToken};

async fn log(repo: &TestRepo) -> Vec<LogCommit> {
    let mut commits = Vec::new();
    let total = stream_log(
        &repo.git,
        &repo.path,
        &CancellationToken::new(),
        2,
        3,
        |page| commits.extend(page),
    )
    .await
    .expect("stream log");
    assert_eq!(total, commits.len());
    commits
}

#[tokio::test]
async fn an_unborn_repository_has_an_empty_history() {
    let repo = TestRepo::init().await;
    assert!(log(&repo).await.is_empty());
}

#[tokio::test]
async fn history_streams_in_pages_with_merges_drawn() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "1\n", "first").await;
    repo.commit_file("a.txt", "2\n", "second").await;
    repo.git_ok(&["switch", "-c", "topic"]).await;
    repo.commit_file("b.txt", "topic\n", "on topic").await;
    repo.git_ok(&["switch", "main"]).await;
    repo.commit_file("c.txt", "main\n", "on main").await;
    repo.git_ok(&["merge", "--no-ff", "-m", "merge topic", "topic"])
        .await;
    repo.git_ok(&["stash", "list"]).await;

    let commits = log(&repo).await;
    let subjects: Vec<&str> = commits.iter().map(|c| &*c.subject).collect();
    assert_eq!(subjects.len(), 5);
    assert_eq!(subjects[0], "merge topic");
    assert_eq!(commits[0].parents.len(), 2);
    assert_eq!(*subjects.last().unwrap(), "first");
    // Every parent appears after its child (the graph relies on it).
    for (index, commit) in commits.iter().enumerate() {
        for parent in commit.parents.iter() {
            let at = commits.iter().position(|c| &c.oid == parent).unwrap();
            assert!(at > index, "{} before its parent", commit.subject);
        }
    }
    let rows = layout(&commits);
    let picture = ascii(&rows);
    assert_eq!(picture[0], "M\\", "{picture:#?}");
    assert_eq!(picture.last().unwrap(), "*", "{picture:#?}");
    assert_eq!(rows.iter().map(|r| r.width).max(), Some(2));
}

#[tokio::test]
async fn stashes_are_not_part_of_the_history() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "1\n", "first").await;
    repo.write("a.txt", "dirty\n");
    repo.git_ok(&["stash", "push", "-m", "wip"]).await;
    let commits = log(&repo).await;
    assert_eq!(commits.len(), 1);
}

#[tokio::test]
async fn commit_detail_lists_changes_with_line_counts() {
    let repo = TestRepo::init().await;
    repo.commit_file("keep.txt", "a\nb\nc\nd\ne\nf\n", "root\n\nbody line")
        .await;
    repo.write("keep.txt", "a\nb\nC\nd\ne\nf\ng\n");
    repo.write("bin.dat", "\0\x01binary");
    repo.git_ok(&["mv", "keep.txt", "moved.txt"]).await;
    repo.git_ok(&["add", "-A"]).await;
    repo.git_ok(&["commit", "-m", "change things"]).await;

    let head = String::from_utf8(repo.git_ok(&["rev-parse", "HEAD"]).await).unwrap();
    let parent = String::from_utf8(repo.git_ok(&["rev-parse", "HEAD^"]).await).unwrap();
    let (detail, files) = commit_detail(
        &repo.git,
        &repo.path,
        head.trim(),
        Some(parent.trim()),
        None,
    )
    .await
    .expect("detail");
    assert_eq!(detail.subject(), "change things");
    assert_eq!(detail.author, common::AUTHOR);
    assert_eq!(detail.author_email, common::EMAIL);
    assert_eq!(files.len(), 2, "{files:#?}");
    let binary = files.iter().find(|f| f.path == "bin.dat").unwrap();
    assert_eq!(binary.status, 'A');
    assert_eq!(binary.added, None, "binary files have no line counts");
    let moved = files.iter().find(|f| f.path == "moved.txt").unwrap();
    assert_eq!(moved.status, 'R');
    assert_eq!(moved.orig_path.as_deref(), Some("keep.txt"));
    assert_eq!((moved.added, moved.removed), (Some(2), Some(1)));

    // The root commit is compared with the empty tree.
    let (root, files) = commit_detail(&repo.git, &repo.path, parent.trim(), None, None)
        .await
        .expect("root detail");
    assert_eq!(root.subject(), "root");
    assert_eq!(root.body(), "body line");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, 'A');
    assert_eq!(files[0].added, Some(6));
}

#[tokio::test]
async fn a_commit_diff_uses_the_same_pipeline_as_the_working_tree() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "first").await;
    repo.commit_file("a.txt", "two\n", "second").await;
    let head = String::from_utf8(repo.git_ok(&["rev-parse", "HEAD"]).await).unwrap();
    let parent = String::from_utf8(repo.git_ok(&["rev-parse", "HEAD^"]).await).unwrap();
    let diff = repo
        .diff(
            "a.txt",
            facet_git::DiffSource::Commit {
                old: Some(parent.trim().to_owned()),
                new: head.trim().to_owned(),
            },
        )
        .await;
    let text: Vec<String> = diff
        .lines
        .iter()
        .map(|line| String::from_utf8_lossy(diff.line_bytes(line)).into_owned())
        .collect();
    assert_eq!(text, vec!["one", "two"]);
    assert_eq!((diff.added, diff.removed), (1, 1));

    let root = repo
        .diff(
            "a.txt",
            facet_git::DiffSource::Commit {
                old: None,
                new: parent.trim().to_owned(),
            },
        )
        .await;
    assert_eq!(root.hunks.len(), 1);
}
