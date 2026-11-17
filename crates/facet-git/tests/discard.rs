//! Integration tests for discard and its undo/redo.

mod common;

use std::time::Duration;

use common::TestRepo;
use facet_git::{
    build_patch, discard_files, discard_patch, list_undo, prune_undo, undo, DiffSource,
    DiscardFile, PatchDirection,
};

fn tracked(path: &str) -> DiscardFile {
    DiscardFile {
        path: path.to_owned(),
        tracked: true,
    }
}

fn untracked(path: &str) -> DiscardFile {
    DiscardFile {
        path: path.to_owned(),
        tracked: false,
    }
}

#[tokio::test]
async fn discard_undo_redo_for_files_of_every_kind() {
    let repo = TestRepo::init().await;
    repo.write("modified.txt", "committed\n");
    repo.write("dir/deleted.txt", "committed\n");
    repo.write("staged.txt", "committed\n");
    repo.git_ok(&["add", "-A"]).await;
    repo.git_ok(&["commit", "-m", "base"]).await;

    repo.write("modified.txt", "my edit\n");
    repo.remove("dir/deleted.txt");
    repo.write("staged.txt", "staged edit\n");
    repo.git_ok(&["add", "staged.txt"]).await;
    repo.write("staged.txt", "staged edit\nplus unstaged\n");
    repo.write("new/deep/untracked.txt", "brand new\n");

    let files = [
        tracked("modified.txt"),
        tracked("dir/deleted.txt"),
        tracked("staged.txt"),
        untracked("new/deep/untracked.txt"),
    ];
    let entry = discard_files(&repo.git, &repo.info(), &files, "4 files")
        .await
        .unwrap();
    assert!(!entry.redo);
    assert_eq!(entry.label, "4 files");

    // Tracked files are back at their index version; the untracked one (and its now empty
    // directories) is gone; staged changes are untouched.
    assert_eq!(repo.read("modified.txt"), "committed\n");
    assert_eq!(repo.read("dir/deleted.txt"), "committed\n");
    assert_eq!(repo.read("staged.txt"), "staged edit\n");
    assert!(!repo.exists("new"));
    let status = repo.status().await;
    assert_eq!(status.entries.len(), 1);
    assert!(status.entries[0].is_staged() && !status.entries[0].is_unstaged());

    // Undo puts every byte back, including the deletion.
    let redo = undo(&repo.git, &repo.info(), &entry).await.unwrap();
    assert!(redo.redo);
    assert!(!entry.dir.exists(), "a consumed entry is removed");
    assert_eq!(repo.read("modified.txt"), "my edit\n");
    assert!(!repo.exists("dir/deleted.txt"));
    assert_eq!(repo.read("staged.txt"), "staged edit\nplus unstaged\n");
    assert_eq!(repo.read("new/deep/untracked.txt"), "brand new\n");

    // Redo discards again without asking git; undoing that restores once more.
    let again = undo(&repo.git, &repo.info(), &redo).await.unwrap();
    assert!(!again.redo);
    assert_eq!(repo.read("modified.txt"), "committed\n");
    assert_eq!(repo.read("dir/deleted.txt"), "committed\n");
    assert!(!repo.exists("new/deep/untracked.txt"));
    undo(&repo.git, &repo.info(), &again).await.unwrap();
    assert_eq!(repo.read("modified.txt"), "my edit\n");
    assert_eq!(repo.read("new/deep/untracked.txt"), "brand new\n");
}

#[tokio::test]
async fn undo_never_overwrites_newer_edits() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "committed\n", "base").await;
    repo.write("f.txt", "first edit\n");
    let entry = discard_files(&repo.git, &repo.info(), &[tracked("f.txt")], "f.txt")
        .await
        .unwrap();

    // The user keeps working after the discard, then undoes it anyway.
    repo.write("f.txt", "second edit\n");
    let redo = undo(&repo.git, &repo.info(), &entry).await.unwrap();
    assert_eq!(repo.read("f.txt"), "first edit\n");
    // The newer edit was saved by the undo and comes back with one more step.
    undo(&repo.git, &repo.info(), &redo).await.unwrap();
    assert_eq!(repo.read("f.txt"), "second edit\n");
}

#[tokio::test]
async fn discard_lines_then_undo() {
    let repo = TestRepo::init().await;
    let base: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    repo.commit_file("f.txt", &base, "base").await;
    let edited = base
        .replace("line 2\n", "line two\n")
        .replace("line 18\n", "line 18\nline 18b\n");
    repo.write("f.txt", &edited);

    // Discard only the second hunk.
    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;
    assert_eq!(diff.hunks.len(), 2);
    let second = diff.hunks[1].lines.clone();
    let patch = build_patch(&diff, "f.txt", PatchDirection::Reverse, |i| {
        second.contains(&(i as u32))
    })
    .unwrap();
    let entry = discard_patch(&repo.git, &repo.info(), &patch, "1 hunk in f.txt")
        .await
        .unwrap();
    assert_eq!(repo.read("f.txt"), base.replace("line 2\n", "line two\n"));

    let redo = undo(&repo.git, &repo.info(), &entry).await.unwrap();
    assert_eq!(repo.read("f.txt"), edited);
    assert!(redo.redo);
    let again = undo(&repo.git, &repo.info(), &redo).await.unwrap();
    assert_eq!(repo.read("f.txt"), base.replace("line 2\n", "line two\n"));

    // If the file changed so the patch no longer fits, undo fails cleanly and keeps the
    // entry for a later attempt.
    repo.write("f.txt", "something else entirely\n");
    let result = undo(&repo.git, &repo.info(), &again).await;
    assert!(result.is_err());
    assert_eq!(repo.read("f.txt"), "something else entirely\n");
    assert!(again.dir.join("patch").exists());
}

#[tokio::test]
async fn a_failed_discard_puts_everything_back() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "committed\n", "base").await;
    repo.write("f.txt", "my edit\n");
    repo.write("other.txt", "untracked\n");

    // `nope.txt` is not known to git, so restoring it from the index fails.
    let files = [
        tracked("f.txt"),
        untracked("other.txt"),
        tracked("nope.txt"),
    ];
    let result = discard_files(&repo.git, &repo.info(), &files, "3 files").await;
    assert!(result.is_err());
    assert_eq!(repo.read("f.txt"), "my edit\n");
    assert_eq!(repo.read("other.txt"), "untracked\n");
    assert!(list_undo(&repo.info()).await.is_empty());
}

#[tokio::test]
async fn entries_are_listed_in_order_and_pruned() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "committed\n", "base").await;
    assert!(list_undo(&repo.info()).await.is_empty());

    let mut made = Vec::new();
    for n in 0..4 {
        repo.write("f.txt", &format!("edit {n}\n"));
        let label = format!("discard {n}");
        made.push(
            discard_files(&repo.git, &repo.info(), &[tracked("f.txt")], &label)
                .await
                .unwrap(),
        );
    }
    let listed = list_undo(&repo.info()).await;
    assert_eq!(listed, made);

    // Keep the newest two.
    let removed = prune_undo(&repo.info(), 2, Duration::from_secs(3600)).await;
    assert_eq!(removed, 2);
    let listed = list_undo(&repo.info()).await;
    assert_eq!(
        listed.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        ["discard 2", "discard 3"]
    );
    // The survivors still work.
    undo(&repo.git, &repo.info(), &listed[1]).await.unwrap();
    assert_eq!(repo.read("f.txt"), "edit 3\n");

    // Age limit: everything older than "now" goes.
    tokio::time::sleep(Duration::from_millis(20)).await;
    prune_undo(&repo.info(), 100, Duration::ZERO).await;
    assert!(list_undo(&repo.info()).await.is_empty());
    // The undo store lives inside the git directory and never shows up in status.
    assert!(repo
        .status()
        .await
        .entries
        .iter()
        .all(|e| e.path == "f.txt"));
}
