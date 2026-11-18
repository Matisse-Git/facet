//! Branches, tags, remotes and stashes against real repositories.

mod common;

use std::sync::{Arc, Mutex};

use common::TestRepo;
use facet_git::parse::{RefKind, Track};
use facet_git::{
    add_remote, create_branch, create_tag, delete_branch, delete_tag, fetch, pull, push, push_tag,
    refs, remove_remote, rename_branch, restore_branch, restore_remote, restore_stash, restore_tag,
    stash_apply, stash_drop, stash_push, switch, valid_branch_name, valid_tag_name,
    CancellationToken, Network, ProgressFn, PullMode, PushRequest, StashOptions, SwitchTarget,
};

async fn rev(repo: &TestRepo, rev: &str) -> String {
    String::from_utf8(repo.git_ok(&["rev-parse", rev]).await)
        .unwrap()
        .trim()
        .to_owned()
}

async fn current_branch(repo: &TestRepo) -> String {
    String::from_utf8(repo.git_ok(&["branch", "--show-current"]).await)
        .unwrap()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn refs_of_an_unborn_repository_are_empty() {
    let repo = TestRepo::init().await;
    let snapshot = refs(&repo.git, &repo.path).await.expect("refs");
    assert!(snapshot.refs.is_empty());
    assert!(snapshot.remotes.is_empty());
    assert!(snapshot.stashes.is_empty());
}

#[tokio::test]
async fn branches_are_created_renamed_deleted_and_restored() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "1\n", "first").await;
    let first = rev(&repo, "HEAD").await;
    repo.commit_file("a.txt", "2\n", "second").await;

    assert!(valid_branch_name(&repo.git, &repo.path, "feature/x").await);
    assert!(!valid_branch_name(&repo.git, &repo.path, "bad..name").await);
    assert!(!valid_branch_name(&repo.git, &repo.path, "-flag").await);
    assert!(!valid_branch_name(&repo.git, &repo.path, "").await);

    create_branch(&repo.git, &repo.path, "feature/x", &first, false)
        .await
        .unwrap();
    assert_eq!(current_branch(&repo).await, "main");
    assert_eq!(rev(&repo, "feature/x").await, first);

    rename_branch(&repo.git, &repo.path, "feature/x", "feature/y")
        .await
        .unwrap();
    let snapshot = refs(&repo.git, &repo.path).await.unwrap();
    let names: Vec<&str> = snapshot.local().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["feature/y", "main"]);
    assert!(snapshot.find("refs/heads/main").unwrap().head);

    let deleted = delete_branch(&repo.git, &repo.path, "feature/y", None)
        .await
        .unwrap();
    assert_eq!(deleted.oid, first);
    assert!(refs(&repo.git, &repo.path)
        .await
        .unwrap()
        .find("refs/heads/feature/y")
        .is_none());
    restore_branch(&repo.git, &repo.path, &deleted)
        .await
        .unwrap();
    assert_eq!(rev(&repo, "feature/y").await, first);

    switch(
        &repo.git,
        &repo.path,
        &SwitchTarget::Branch("feature/y".into()),
    )
    .await
    .unwrap();
    assert_eq!(current_branch(&repo).await, "feature/y");
    assert_eq!(repo.read("a.txt"), "1\n");

    create_branch(&repo.git, &repo.path, "new", "main", true)
        .await
        .unwrap();
    assert_eq!(current_branch(&repo).await, "new");
    assert_eq!(repo.read("a.txt"), "2\n");

    switch(
        &repo.git,
        &repo.path,
        &SwitchTarget::Detached(first.clone()),
    )
    .await
    .unwrap();
    assert_eq!(current_branch(&repo).await, "");
    assert_eq!(rev(&repo, "HEAD").await, first);
}

#[tokio::test]
async fn switching_refuses_to_overwrite_local_changes() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "1\n", "first").await;
    repo.git_ok(&["branch", "other"]).await;
    repo.commit_file("a.txt", "2\n", "second").await;
    repo.write("a.txt", "local edit\n");

    let error = switch(&repo.git, &repo.path, &SwitchTarget::Branch("other".into()))
        .await
        .unwrap_err();
    assert!(matches!(error, facet_git::GitError::Failed { .. }));
    assert_eq!(current_branch(&repo).await, "main");
    assert_eq!(repo.read("a.txt"), "local edit\n");
}

#[tokio::test]
async fn tags_lightweight_and_annotated_with_undo() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "1\n", "first").await;
    let head = rev(&repo, "HEAD").await;

    assert!(valid_tag_name(&repo.git, &repo.path, "v1.0").await);
    assert!(!valid_tag_name(&repo.git, &repo.path, "v1 0").await);

    create_tag(&repo.git, &repo.path, "light", "HEAD", None)
        .await
        .unwrap();
    create_tag(
        &repo.git,
        &repo.path,
        "v1.0",
        "HEAD",
        Some("Release 1.0\n\nNotes"),
    )
    .await
    .unwrap();

    let snapshot = refs(&repo.git, &repo.path).await.unwrap();
    let light = snapshot.find("refs/tags/light").unwrap();
    assert_eq!(light.oid, head);
    assert_eq!(light.tag_object, None);
    let annotated = snapshot.find("refs/tags/v1.0").unwrap();
    assert_eq!(annotated.kind, RefKind::Tag);
    assert_eq!(
        annotated.oid, head,
        "annotated tags are peeled to the commit"
    );
    let tag_object = annotated.tag_object.clone().expect("tag object");
    assert_ne!(tag_object, head);

    let deleted = delete_tag(&repo.git, &repo.path, "v1.0").await.unwrap();
    assert_eq!(deleted.object, tag_object);
    restore_tag(&repo.git, &repo.path, &deleted).await.unwrap();
    let message = String::from_utf8(
        repo.git_ok(&["tag", "-l", "--format=%(contents)", "v1.0"])
            .await,
    )
    .unwrap();
    assert!(message.starts_with("Release 1.0"), "{message}");
    // Restoring over a tag that exists again fails rather than overwriting it.
    assert!(restore_tag(&repo.git, &repo.path, &deleted).await.is_err());
}

#[tokio::test]
async fn stash_push_partial_staged_apply_pop_drop_restore() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "a\n", "first").await;
    repo.commit_file("b.txt", "b\n", "second").await;

    // Partial stash: only a.txt and an untracked file whose name looks like a pattern.
    repo.write("a.txt", "a changed\n");
    repo.write("b.txt", "b changed\n");
    // As a glob, `[ab].txt` would match b.txt too.
    repo.write("[ab].txt", "pattern-like\n");
    stash_push(
        &repo.git,
        &repo.path,
        &StashOptions {
            message: "only a".into(),
            paths: vec!["a.txt".into(), "[ab].txt".into()],
            include_untracked: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.read("a.txt"), "a\n");
    assert_eq!(repo.read("b.txt"), "b changed\n");
    assert!(!repo.exists("[ab].txt"));

    // Staged-only stash: b.txt staged, c.txt untracked stays.
    repo.git_ok(&["add", "b.txt"]).await;
    repo.write("c.txt", "untracked\n");
    stash_push(
        &repo.git,
        &repo.path,
        &StashOptions {
            message: "staged b".into(),
            staged_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.read("b.txt"), "b\n");
    assert!(repo.exists("c.txt"));

    // Including untracked files.
    stash_push(
        &repo.git,
        &repo.path,
        &StashOptions {
            message: "with untracked".into(),
            include_untracked: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!repo.exists("c.txt"));

    let stashes = refs(&repo.git, &repo.path).await.unwrap().stashes;
    let messages: Vec<&str> = stashes.iter().map(|s| s.message.as_str()).collect();
    assert_eq!(messages.len(), 3);
    assert!(messages[0].ends_with("with untracked"), "{messages:?}");
    assert!(stashes[0].untracked);
    assert!(!stashes[1].untracked);

    // Apply keeps the entry; pop removes it.
    stash_apply(&repo.git, &repo.path, &stashes[0], false)
        .await
        .unwrap();
    assert!(repo.exists("c.txt"));
    assert_eq!(refs(&repo.git, &repo.path).await.unwrap().stashes.len(), 3);
    repo.remove("c.txt");

    // Drop with undo.
    let dropped = stashes[1].clone();
    stash_drop(&repo.git, &repo.path, &dropped).await.unwrap();
    let after = refs(&repo.git, &repo.path).await.unwrap().stashes;
    assert_eq!(after.len(), 2);
    // The old selector of the last one now points elsewhere: acting on it is refused.
    assert!(stash_drop(&repo.git, &repo.path, &stashes[2])
        .await
        .is_err());
    restore_stash(&repo.git, &repo.path, &dropped)
        .await
        .unwrap();
    let restored = refs(&repo.git, &repo.path).await.unwrap().stashes;
    assert_eq!(restored.len(), 3);
    assert_eq!(restored[0].oid, dropped.oid);
    assert_eq!(restored[0].message, dropped.message);

    // Pop the partial "only a" stash.
    let only_a = restored
        .iter()
        .find(|s| s.message.ends_with("only a"))
        .unwrap();
    stash_apply(&repo.git, &repo.path, only_a, true)
        .await
        .unwrap();
    assert_eq!(repo.read("a.txt"), "a changed\n");
    assert_eq!(refs(&repo.git, &repo.path).await.unwrap().stashes.len(), 2);
}

fn progress_sink() -> (ProgressFn, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let progress: ProgressFn = Arc::new(move |line: &str| sink.lock().unwrap().push(line.into()));
    (progress, lines)
}

#[tokio::test]
async fn remotes_fetch_pull_and_push_with_lease() {
    let server = TestRepo::init_bare().await;
    let server_url = server.path.to_string_lossy().replace('\\', "/");

    let alice = TestRepo::init().await;
    alice.commit_file("a.txt", "1\n", "first").await;
    add_remote(&alice.git, &alice.path, "origin", &server_url)
        .await
        .unwrap();
    let (progress, _lines) = progress_sink();
    let cancel = CancellationToken::new();
    let net = Network {
        git: &alice.git,
        root: &alice.path,
        cancel: &cancel,
        progress: progress.clone(),
    };
    push(
        &net,
        &PushRequest {
            remote: "origin".into(),
            branch: "main".into(),
            remote_branch: "main".into(),
            set_upstream: true,
            force_with_lease: None,
        },
    )
    .await
    .unwrap();
    let snapshot = refs(&alice.git, &alice.path).await.unwrap();
    let main = snapshot.find("refs/heads/main").unwrap();
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!(main.track, Track::InSync);
    assert_eq!(snapshot.remotes.len(), 1);
    assert_eq!(
        snapshot.remotes[0].url.as_deref(),
        Some(server_url.as_str())
    );
    let remote_main = snapshot.find("refs/remotes/origin/main").unwrap();
    assert_eq!(
        remote_main.kind,
        RefKind::Remote {
            remote: "origin".into()
        }
    );

    // Bob clones, commits and pushes.
    let bob = TestRepo::init().await;
    add_remote(&bob.git, &bob.path, "origin", &server_url)
        .await
        .unwrap();
    let bob_net = Network {
        git: &bob.git,
        root: &bob.path,
        cancel: &cancel,
        progress: progress.clone(),
    };
    fetch(&bob_net, Some("origin")).await.unwrap();
    switch(
        &bob.git,
        &bob.path,
        &SwitchTarget::Track {
            remote_branch: "origin/main".into(),
            local: "main".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(bob.read("a.txt"), "1\n");
    bob.commit_file("b.txt", "bob\n", "bob's work").await;
    push(
        &bob_net,
        &PushRequest {
            remote: "origin".into(),
            branch: "main".into(),
            remote_branch: "main".into(),
            set_upstream: false,
            force_with_lease: None,
        },
    )
    .await
    .unwrap();

    // Alice fetches (all remotes): she is behind, then fast-forwards.
    fetch(&net, None).await.unwrap();
    let snapshot = refs(&alice.git, &alice.path).await.unwrap();
    assert_eq!(
        snapshot.find("refs/heads/main").unwrap().track,
        Track::Behind
    );
    pull(&net, PullMode::FastForwardOnly).await.unwrap();
    assert_eq!(alice.read("b.txt"), "bob\n");

    // Diverge: Alice rewrites her last commit; Bob pushes again. Fast-forward-only fails, the
    // lease protects Bob's commit from a force push based on a stale view, a rebase works.
    let seen = rev(&alice, "origin/main").await;
    alice.git_ok(&["commit", "--amend", "-m", "amended"]).await;
    bob.commit_file("b.txt", "bob 2\n", "bob again").await;
    push(
        &bob_net,
        &PushRequest {
            remote: "origin".into(),
            branch: "main".into(),
            remote_branch: "main".into(),
            set_upstream: false,
            force_with_lease: None,
        },
    )
    .await
    .unwrap();
    let forced = PushRequest {
        remote: "origin".into(),
        branch: "main".into(),
        remote_branch: "main".into(),
        set_upstream: false,
        force_with_lease: Some(seen),
    };
    assert!(push(&net, &forced).await.is_err(), "lease must protect bob");
    assert_eq!(rev(&server, "main").await, rev(&bob, "HEAD").await);

    fetch(&net, Some("origin")).await.unwrap();
    assert_eq!(
        refs(&alice.git, &alice.path)
            .await
            .unwrap()
            .find("refs/heads/main")
            .unwrap()
            .track,
        Track::Diverged
    );
    assert!(pull(&net, PullMode::FastForwardOnly).await.is_err());
    pull(&net, PullMode::Rebase).await.unwrap();
    assert_eq!(alice.read("b.txt"), "bob 2\n");

    // With a fresh lease, a force push goes through.
    alice
        .git_ok(&["commit", "--amend", "-m", "amended again"])
        .await;
    let lease = rev(&alice, "origin/main").await;
    push(
        &net,
        &PushRequest {
            force_with_lease: Some(lease),
            ..forced
        },
    )
    .await
    .unwrap();
    assert_eq!(rev(&server, "main").await, rev(&alice, "HEAD").await);

    // Tags.
    create_tag(&alice.git, &alice.path, "v1", "HEAD", Some("one"))
        .await
        .unwrap();
    push_tag(&net, "origin", "v1").await.unwrap();
    assert!(server.git_ok(&["tag", "-l"]).await.starts_with(b"v1"));

    // Remove and restore a remote.
    let origin = refs(&alice.git, &alice.path)
        .await
        .unwrap()
        .remote("origin")
        .cloned()
        .unwrap();
    remove_remote(&alice.git, &alice.path, "origin")
        .await
        .unwrap();
    assert!(refs(&alice.git, &alice.path)
        .await
        .unwrap()
        .remotes
        .is_empty());
    restore_remote(&alice.git, &alice.path, &origin)
        .await
        .unwrap();
    assert_eq!(
        refs(&alice.git, &alice.path).await.unwrap().remotes,
        vec![origin]
    );
}

#[tokio::test]
async fn a_failing_network_command_reports_progress_and_an_error() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "1\n", "first").await;
    add_remote(&repo.git, &repo.path, "nowhere", "D:/definitely/not/a/repo")
        .await
        .unwrap();
    let (progress, _) = progress_sink();
    let cancel = CancellationToken::new();
    let net = Network {
        git: &repo.git,
        root: &repo.path,
        cancel: &cancel,
        progress,
    };
    let error = fetch(&net, Some("nowhere")).await.unwrap_err();
    match error {
        facet_git::GitError::Failed { stderr, .. } => assert!(!stderr.is_empty()),
        other => panic!("unexpected {other:?}"),
    }
}
