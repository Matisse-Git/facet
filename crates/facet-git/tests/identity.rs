//! Integration tests for the commit author: where it comes from, setting and removing the
//! repository's own, and the last commit's author (kept when amending).

mod common;

use common::{TestRepo, AUTHOR, EMAIL};
use facet_git::{
    global_identity, identity, last_commit_author, set_local_identity, unset_local_identity,
};

#[tokio::test]
async fn repository_author_overrides_the_global_one_and_can_be_removed() {
    let repo = TestRepo::init().await;
    let current = identity(&repo.git, Some(&repo.path)).await.unwrap();
    assert!(current.is_unset());
    assert!(!current.is_repo_override());

    repo.git_ok(&["config", "--global", "user.name", "Work Me"])
        .await;
    repo.git_ok(&["config", "--global", "user.email", "me@corp.example"])
        .await;
    let current = identity(&repo.git, Some(&repo.path)).await.unwrap();
    assert_eq!(
        (current.name.as_str(), current.email.as_str()),
        ("Work Me", "me@corp.example")
    );
    assert_eq!(
        (current.scope.as_str(), current.email_scope.as_str()),
        ("global", "global")
    );
    assert!(!current.is_repo_override());
    let global = global_identity(&repo.git).await.unwrap();
    assert_eq!(global.email, "me@corp.example");

    set_local_identity(
        &repo.git,
        &repo.path,
        "GitHub Me",
        "1+me@users.noreply.github.com",
    )
    .await
    .unwrap();
    let current = identity(&repo.git, Some(&repo.path)).await.unwrap();
    assert_eq!(current.name, "GitHub Me");
    assert_eq!(current.email, "1+me@users.noreply.github.com");
    assert!(current.is_repo_override());
    // The global config is untouched.
    assert_eq!(global_identity(&repo.git).await.unwrap(), global);

    unset_local_identity(&repo.git, &repo.path).await.unwrap();
    let current = identity(&repo.git, Some(&repo.path)).await.unwrap();
    assert_eq!(current.email, "me@corp.example");
    assert!(!current.is_repo_override());
    // Nothing left to remove is not an error.
    unset_local_identity(&repo.git, &repo.path).await.unwrap();
}

#[tokio::test]
async fn last_commit_author_is_read_from_head() {
    let repo = TestRepo::init().await;
    assert_eq!(
        last_commit_author(&repo.git, &repo.path).await.unwrap(),
        None
    );
    repo.commit_file("a.txt", "a\n", "First").await;
    assert_eq!(
        last_commit_author(&repo.git, &repo.path).await.unwrap(),
        Some((AUTHOR.to_owned(), EMAIL.to_owned()))
    );
}
