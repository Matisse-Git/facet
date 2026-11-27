//! The check shown before a repository's folder goes to the Recycle Bin: uncommitted and
//! untracked files, unpushed commits per branch, stashes, remotes and worktrees.

mod common;

use std::path::Path;

use common::TestRepo;
use facet_git::{add_worktree, removal_check, same_dir, GitError, WorktreeTarget};

#[tokio::test]
async fn a_repository_without_a_remote_is_the_only_copy() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "first").await;

    let check = removal_check(&repo.git, &repo.path).await.unwrap();
    assert!(!check.has_remote);
    assert!(
        check.unpushed.is_empty(),
        "no remote: nothing counts as unpushed"
    );
    assert!(!check.is_safe());
}

#[tokio::test]
async fn pushed_and_clean_is_safe_and_local_work_is_listed() {
    let remote = TestRepo::init_bare().await;
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "first").await;
    let url = remote.path.to_string_lossy().into_owned();
    repo.git_ok(&["remote", "add", "origin", &url]).await;
    repo.git_ok(&["push", "-u", "origin", "main"]).await;

    let check = removal_check(&repo.git, &repo.path).await.unwrap();
    assert!(check.has_remote && check.is_safe(), "{check:?}");

    // Two commits only here on main, one on a new branch; a change, an untracked file and a
    // stash.
    repo.commit_file("a.txt", "two\n", "second").await;
    repo.commit_file("a.txt", "three\n", "third").await;
    repo.git_ok(&["switch", "-c", "topic"]).await;
    repo.commit_file("b.txt", "topic\n", "topic work").await;
    repo.write("a.txt", "stashed\n");
    repo.git_ok(&["stash", "push", "-m", "kept"]).await;
    repo.write("a.txt", "edited\n");
    repo.write("new.txt", "untracked\n");

    let check = removal_check(&repo.git, &repo.path).await.unwrap();
    assert_eq!(check.changed_files, 1);
    assert_eq!(check.untracked_files, 1);
    assert_eq!(check.stashes, 1);
    assert_eq!(
        check.unpushed,
        vec![("topic".to_owned(), 3), ("main".to_owned(), 2)]
    );
    assert!(!check.unpushed_capped);
    assert!(!check.is_safe());
}

#[tokio::test]
async fn worktrees_are_reported_from_both_sides() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "first").await;
    let linked = repo.path.parent().unwrap().join("linked wt");
    add_worktree(
        &repo.git,
        &repo.path,
        &linked,
        &WorktreeTarget::NewBranch {
            name: "side".into(),
            start: None,
        },
    )
    .await
    .unwrap();

    let main = removal_check(&repo.git, &repo.path).await.unwrap();
    assert!(main.main_worktree.is_none());
    assert_eq!(main.linked_worktrees.len(), 1);
    assert!(same_dir(&main.linked_worktrees[0], &linked));

    let side = removal_check(&repo.git, &linked).await.unwrap();
    assert!(side.linked_worktrees.is_empty());
    assert!(same_dir(side.main_worktree.as_deref().unwrap(), &repo.path));
}

#[tokio::test]
async fn folders_that_are_not_a_repository_root_are_refused() {
    let repo = TestRepo::init().await;
    repo.commit_file("sub/a.txt", "one\n", "first").await;

    let inside = removal_check(&repo.git, &repo.path.join("sub")).await;
    assert!(
        matches!(inside, Err(GitError::NotARepository { .. })),
        "{inside:?}"
    );

    let plain = TestRepo::empty_dir();
    let error = removal_check(&plain.git, &plain.path).await;
    assert!(
        matches!(error, Err(GitError::NotARepository { .. })),
        "{error:?}"
    );

    let missing = removal_check(&repo.git, Path::new(r"Z:\no\such\folder")).await;
    assert!(
        matches!(missing, Err(GitError::PathNotFound { .. })),
        "{missing:?}"
    );
}

#[tokio::test]
async fn bare_repositories_have_no_working_tree_to_check() {
    let bare = TestRepo::init_bare().await;
    let check = removal_check(&bare.git, &bare.path).await.unwrap();
    assert!(check.bare);
    assert_eq!(check.changed_files + check.untracked_files, 0);
}
