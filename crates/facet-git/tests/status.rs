//! Integration tests for `status` and the per-repository performance settings.

mod common;

use common::TestRepo;
use facet_git::parse::{Change, EntryKind, StatusEntry};
use facet_git::{perf_config, set_perf_config, PerfConfig};

fn entry<'a>(entries: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
    entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("no entry for {path} in {entries:#?}"))
}

#[tokio::test]
async fn unborn_branch_and_clean_tree() {
    let repo = TestRepo::init().await;
    let status = repo.status().await;
    assert_eq!(status.branch.oid, None);
    assert_eq!(status.branch.head.as_deref(), Some("main"));
    assert!(status.entries.is_empty());

    // Staged on an unborn branch: added, with no blob in HEAD.
    repo.write("first.txt", "hello\n");
    repo.git_ok(&["add", "first.txt"]).await;
    let status = repo.status().await;
    let first = entry(&status.entries, "first.txt");
    assert_eq!(first.index, Change::Added);
    assert!(first.head_oid.is_none() && first.index_oid.is_some());

    repo.git_ok(&["commit", "-m", "first"]).await;
    let status = repo.status().await;
    assert!(status.branch.oid.is_some());
    assert!(status.entries.is_empty());
}

#[tokio::test]
async fn modified_staged_deleted_and_untracked_with_awkward_names() {
    let repo = TestRepo::init().await;
    repo.write("plain.txt", "1\n");
    repo.write("Content/My Level.umap", "level\n");
    repo.write("été/光.txt", "unicode\n");
    repo.write("gone.txt", "bye\n");
    repo.git_ok(&["add", "-A"]).await;
    repo.git_ok(&["commit", "-m", "base"]).await;

    repo.write("plain.txt", "2\n");
    repo.git_ok(&["add", "plain.txt"]).await;
    repo.write("plain.txt", "3\n"); // staged *and* unstaged changes
    repo.write("Content/My Level.umap", "changed\n");
    repo.write("été/光.txt", "changed\n");
    repo.remove("gone.txt");
    repo.write("new dir/[a] #1.txt", "untracked\n");
    repo.write("new dir/deeper/b.txt", "untracked\n");

    let status = repo.status().await;
    let plain = entry(&status.entries, "plain.txt");
    assert_eq!(
        (plain.index, plain.worktree),
        (Change::Modified, Change::Modified)
    );
    assert!(plain.is_staged() && plain.is_unstaged());
    assert_ne!(plain.head_oid, plain.index_oid);

    let level = entry(&status.entries, "Content/My Level.umap");
    assert_eq!(
        (level.index, level.worktree),
        (Change::Unmodified, Change::Modified)
    );
    assert_eq!(
        entry(&status.entries, "été/光.txt").worktree,
        Change::Modified
    );
    assert_eq!(entry(&status.entries, "gone.txt").worktree, Change::Deleted);

    // Untracked files are listed one by one, never collapsed to their directory.
    for path in ["new dir/[a] #1.txt", "new dir/deeper/b.txt"] {
        let untracked = entry(&status.entries, path);
        assert_eq!(untracked.kind, EntryKind::Untracked);
        assert!(untracked.is_unstaged() && !untracked.is_staged());
    }
    assert_eq!(status.entries.len(), 6);
}

#[tokio::test]
async fn staged_renames_carry_their_origin() {
    let repo = TestRepo::init().await;
    repo.commit_file("old name.txt", "same content\nline two\n", "base")
        .await;
    repo.git_ok(&["mv", "old name.txt", "new name.txt"]).await;

    let status = repo.status().await;
    assert_eq!(status.entries.len(), 1);
    let renamed = &status.entries[0];
    assert_eq!(renamed.path, "new name.txt");
    assert_eq!(renamed.orig_path.as_deref(), Some("old name.txt"));
    assert_eq!(renamed.index, Change::Renamed);
    assert!(renamed.is_staged());
}

#[tokio::test]
async fn conflicts_are_unmerged_entries() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "base\n", "base").await;
    repo.git_ok(&["checkout", "-q", "-b", "other"]).await;
    repo.commit_file("f.txt", "theirs\n", "theirs").await;
    repo.git_ok(&["checkout", "-q", "main"]).await;
    repo.commit_file("f.txt", "ours\n", "ours").await;

    // The merge stops with a conflict (non-zero exit), which is the state under test.
    let merge = repo
        .git
        .command(["merge", "other"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert!(!merge.success());

    let status = repo.status().await;
    let conflict = entry(&status.entries, "f.txt");
    assert_eq!(conflict.kind, EntryKind::Unmerged);
    assert_eq!(conflict.conflict, Some(*b"UU"));
    assert!(conflict.is_unstaged() && !conflict.is_staged());
    assert!(conflict.head_oid.is_some());
}

#[tokio::test]
async fn ahead_and_behind_of_the_upstream() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "1\n", "one").await;
    repo.git_ok(&["branch", "upstream-like"]).await;
    repo.git_ok(&["branch", "--set-upstream-to=upstream-like"])
        .await;
    repo.commit_file("f.txt", "2\n", "two").await;
    repo.commit_file("f.txt", "3\n", "three").await;

    let status = repo.status().await;
    assert_eq!(status.branch.upstream.as_deref(), Some("upstream-like"));
    assert_eq!(
        (status.branch.ahead, status.branch.behind),
        (Some(2), Some(0))
    );

    // Detached HEAD: no branch name, no upstream.
    repo.git_ok(&["checkout", "-q", "--detach"]).await;
    let status = repo.status().await;
    assert_eq!(status.branch.head, None);
    assert_eq!(status.branch.upstream, None);
    assert!(status.branch.oid.is_some());
}

#[tokio::test]
async fn submodules_are_flagged() {
    let library = TestRepo::init().await;
    library.commit_file("lib.txt", "v1\n", "lib").await;

    let repo = TestRepo::init().await;
    repo.commit_file("main.txt", "x\n", "base").await;
    let source = library.path.to_string_lossy().replace('\\', "/");
    repo.git_ok(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        &source,
        "libs/engine",
    ])
    .await;
    repo.git_ok(&["commit", "-q", "-m", "add submodule"]).await;

    // A change inside the submodule shows up as a modified submodule entry.
    repo.write("libs/engine/lib.txt", "edited\n");
    let status = repo.status().await;
    let submodule = entry(&status.entries, "libs/engine");
    assert!(submodule.submodule);
    assert_eq!(submodule.worktree, Change::Modified);
}

#[tokio::test]
async fn background_status_takes_no_locks_and_matches() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "1\n", "one").await;
    repo.write("f.txt", "2\n");
    let background = facet_git::status(&repo.git, &repo.path, true, None)
        .await
        .unwrap();
    assert_eq!(background, repo.status().await);
    assert!(!repo.exists(".git/index.lock"));
}

#[tokio::test]
async fn perf_settings_are_local_to_the_repository() {
    let repo = TestRepo::init().await;
    assert_eq!(
        perf_config(&repo.git, &repo.path).await.unwrap(),
        PerfConfig::default()
    );

    set_perf_config(&repo.git, &repo.path, true).await.unwrap();
    let config = perf_config(&repo.git, &repo.path).await.unwrap();
    assert!(config.fsmonitor && config.untracked_cache && config.all_enabled());
    let local = repo.read(".git/config");
    assert!(local.contains("fsmonitor = true") && local.contains("untrackedCache = true"));

    set_perf_config(&repo.git, &repo.path, false).await.unwrap();
    assert_eq!(
        perf_config(&repo.git, &repo.path).await.unwrap(),
        PerfConfig::default()
    );
    // Disabling twice is not an error.
    set_perf_config(&repo.git, &repo.path, false).await.unwrap();

    // The (isolated) global config was never written.
    let global = repo.path.parent().unwrap().join("scratch/global.gitconfig");
    assert_eq!(std::fs::read_to_string(global).unwrap(), "");
}
