//! Integration tests for diffs and for staging/unstaging by file, hunk and line.

mod common;

use common::TestRepo;
use facet_git::parse::{Change, EntryKind, FileDiff, LineKind};
use facet_git::{
    apply_patch, build_patch, diff_file, object_sizes, read_blob, stage_all, stage_paths,
    unstage_all, unstage_paths, ApplyTo, DiffRequest, DiffSource, PatchDirection, PatchError,
};

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| (*path).to_owned()).collect()
}

/// Indices of the changed lines whose text is one of `wanted`.
fn lines_with(diff: &FileDiff, wanted: &[&str]) -> Vec<usize> {
    diff.lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.kind != LineKind::Context
                && wanted.contains(&String::from_utf8_lossy(diff.line_bytes(line)).as_ref())
        })
        .map(|(index, _)| index)
        .collect()
}

fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

// ----- whole files ----------------------------------------------------------------------------

#[tokio::test]
async fn stage_and_unstage_files_of_every_kind() {
    let repo = TestRepo::init().await;
    repo.write("modified.txt", "1\n");
    repo.write("deleted.txt", "1\n");
    repo.write("untouched.txt", "1\n");
    repo.git_ok(&["add", "-A"]).await;
    repo.git_ok(&["commit", "-m", "base"]).await;

    repo.write("modified.txt", "2\n");
    repo.remove("deleted.txt");
    repo.write("new dir/[a] #1.txt", "new\n");
    repo.write("été/光.txt", "new\n");
    repo.write("left alone.txt", "new\n");

    let selected = paths(&[
        "modified.txt",
        "deleted.txt",
        "new dir/[a] #1.txt",
        "été/光.txt",
    ]);
    stage_paths(&repo.git, &repo.path, &selected).await.unwrap();

    let status = repo.status().await;
    let state = |path: &str| {
        let entry = status.entries.iter().find(|e| e.path == path).unwrap();
        (entry.kind, entry.index, entry.worktree)
    };
    assert_eq!(
        state("modified.txt"),
        (EntryKind::Tracked, Change::Modified, Change::Unmodified)
    );
    assert_eq!(
        state("deleted.txt"),
        (EntryKind::Tracked, Change::Deleted, Change::Unmodified)
    );
    assert_eq!(
        state("new dir/[a] #1.txt"),
        (EntryKind::Tracked, Change::Added, Change::Unmodified)
    );
    assert_eq!(
        state("été/光.txt"),
        (EntryKind::Tracked, Change::Added, Change::Unmodified)
    );
    assert_eq!(state("left alone.txt").0, EntryKind::Untracked);

    // Unstage two of them; the working tree is not touched.
    unstage_paths(
        &repo.git,
        &repo.path,
        &paths(&["modified.txt", "new dir/[a] #1.txt"]),
    )
    .await
    .unwrap();
    let status = repo.status().await;
    let modified = status
        .entries
        .iter()
        .find(|e| e.path == "modified.txt")
        .unwrap();
    assert_eq!(
        (modified.index, modified.worktree),
        (Change::Unmodified, Change::Modified)
    );
    assert_eq!(repo.read("modified.txt"), "2\n");
    let new = status
        .entries
        .iter()
        .find(|e| e.path == "new dir/[a] #1.txt")
        .unwrap();
    assert_eq!(new.kind, EntryKind::Untracked);

    // Empty lists are a no-op, not an error (and spawn nothing).
    let before = repo.log.len();
    stage_paths(&repo.git, &repo.path, &[]).await.unwrap();
    unstage_paths(&repo.git, &repo.path, &[]).await.unwrap();
    assert_eq!(repo.log.len(), before);
}

#[tokio::test]
async fn a_glob_like_name_stages_only_itself() {
    let repo = TestRepo::init().await;
    repo.write("[ab].txt", "literal\n");
    repo.write("a.txt", "would match the pattern\n");
    repo.write("b.txt", "would match the pattern\n");
    stage_paths(&repo.git, &repo.path, &paths(&["[ab].txt"]))
        .await
        .unwrap();
    let status = repo.status().await;
    let staged: Vec<&str> = status
        .entries
        .iter()
        .filter(|entry| entry.is_staged())
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(staged, ["[ab].txt"]);
}

#[tokio::test]
async fn stage_all_and_unstage_all_including_on_an_unborn_branch() {
    let repo = TestRepo::init().await;
    repo.write("a.txt", "a\n");
    repo.write("dir/b.txt", "b\n");

    stage_all(&repo.git, &repo.path).await.unwrap();
    assert!(repo.status().await.entries.iter().all(|e| e.is_staged()));

    // No HEAD yet: unstaging must still work, for single paths and for everything.
    unstage_paths(&repo.git, &repo.path, &paths(&["a.txt"]))
        .await
        .unwrap();
    let status = repo.status().await;
    assert_eq!(
        status
            .entries
            .iter()
            .find(|e| e.path == "a.txt")
            .unwrap()
            .kind,
        EntryKind::Untracked
    );
    unstage_all(&repo.git, &repo.path).await.unwrap();
    assert!(repo
        .status()
        .await
        .entries
        .iter()
        .all(|e| e.kind == EntryKind::Untracked));

    // And with history.
    stage_all(&repo.git, &repo.path).await.unwrap();
    repo.git_ok(&["commit", "-m", "base"]).await;
    repo.write("a.txt", "changed\n");
    repo.remove("dir/b.txt");
    repo.write("c.txt", "c\n");
    stage_all(&repo.git, &repo.path).await.unwrap();
    let status = repo.status().await;
    assert_eq!(status.entries.len(), 3);
    assert!(status
        .entries
        .iter()
        .all(|e| e.is_staged() && !e.is_unstaged()));
    unstage_all(&repo.git, &repo.path).await.unwrap();
    assert!(repo.status().await.entries.iter().all(|e| !e.is_staged()));
}

#[tokio::test]
async fn unstaging_a_rename_takes_both_paths() {
    let repo = TestRepo::init().await;
    repo.commit_file("old.txt", "content\nmore\n", "base").await;
    repo.git_ok(&["mv", "old.txt", "new.txt"]).await;

    // The staged diff of a rename pairs the two paths and reports the similarity.
    let request = DiffRequest {
        path: "new.txt".into(),
        orig_path: Some("old.txt".into()),
        source: DiffSource::Staged,
    };
    let diff = diff_file(&repo.git, &repo.path, &request, None)
        .await
        .unwrap();
    assert_eq!(diff.similarity, Some(100));
    assert!(diff.hunks.is_empty() && !diff.binary);

    unstage_paths(&repo.git, &repo.path, &paths(&["new.txt", "old.txt"]))
        .await
        .unwrap();
    let status = repo.status().await;
    assert!(status.entries.iter().all(|e| !e.is_staged()));
    assert_eq!(
        status
            .entries
            .iter()
            .find(|e| e.path == "old.txt")
            .unwrap()
            .worktree,
        Change::Deleted
    );
}

// ----- diffs ----------------------------------------------------------------------------------

#[tokio::test]
async fn diffs_of_every_source() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "a\nb\nc\n", "base").await;

    // Unchanged file: an empty diff, not an error.
    assert!(repo.diff("f.txt", DiffSource::Unstaged).await.is_empty());
    assert!(repo.diff("f.txt", DiffSource::Staged).await.is_empty());

    repo.write("f.txt", "a\nB\nc\n");
    let unstaged = repo.diff("f.txt", DiffSource::Unstaged).await;
    assert_eq!((unstaged.added, unstaged.removed), (1, 1));
    assert_eq!(unstaged.hunks.len(), 1);
    assert!(repo.diff("f.txt", DiffSource::Staged).await.is_empty());

    repo.git_ok(&["add", "f.txt"]).await;
    let staged = repo.diff("f.txt", DiffSource::Staged).await;
    assert_eq!((staged.added, staged.removed), (1, 1));
    assert!(repo.diff("f.txt", DiffSource::Unstaged).await.is_empty());

    // Untracked: everything is an addition; an empty file is a new file with no hunks.
    repo.write("new dir/[x] new.txt", "one\ntwo\n");
    let untracked = repo
        .diff("new dir/[x] new.txt", DiffSource::Untracked)
        .await;
    assert!(untracked.new_file && !untracked.binary);
    assert_eq!(untracked.added, 2);
    assert!(untracked
        .lines
        .iter()
        .all(|line| line.kind == LineKind::Added));
    repo.write("empty.txt", "");
    let empty = repo.diff("empty.txt", DiffSource::Untracked).await;
    assert!(empty.new_file && empty.hunks.is_empty() && !empty.is_empty());

    // Deleted in the working tree.
    repo.git_ok(&["commit", "-q", "-m", "second"]).await;
    repo.remove("f.txt");
    let deleted = repo.diff("f.txt", DiffSource::Unstaged).await;
    assert!(deleted.deleted_file);
    assert_eq!(deleted.removed, 3);
}

#[tokio::test]
async fn binary_files_are_reported_not_diffed() {
    let repo = TestRepo::init().await;
    repo.write_bytes("asset.uasset", &[0, 1, 2, 3, 0, 255]);
    repo.git_ok(&["add", "-A"]).await;
    repo.git_ok(&["commit", "-m", "base"]).await;
    repo.write_bytes("asset.uasset", &[0, 9, 9, 9, 0, 255, 7]);
    repo.write_bytes("new.bin", &[0, 0, 0]);

    let diff = repo.diff("asset.uasset", DiffSource::Unstaged).await;
    assert!(diff.binary && diff.hunks.is_empty() && diff.lines.is_empty());
    assert_eq!(
        build_patch(&diff, "asset.uasset", PatchDirection::Forward, |_| true),
        Err(PatchError::Binary)
    );
    let untracked = repo.diff("new.bin", DiffSource::Untracked).await;
    assert!(untracked.binary && untracked.new_file);

    // Whole-file staging still works, and the blobs can be read back with their sizes.
    stage_paths(&repo.git, &repo.path, &paths(&["asset.uasset"]))
        .await
        .unwrap();
    let status = repo.status().await;
    let entry = status
        .entries
        .iter()
        .find(|e| e.path == "asset.uasset")
        .unwrap();
    let (head, index) = (
        entry.head_oid.clone().unwrap(),
        entry.index_oid.clone().unwrap(),
    );
    assert_eq!(
        read_blob(&repo.git, &repo.path, &head, None).await.unwrap(),
        [0, 1, 2, 3, 0, 255]
    );
    let missing = "0123456789012345678901234567890123456789";
    assert_eq!(
        object_sizes(&repo.git, &repo.path, &[&head, missing, &index])
            .await
            .unwrap(),
        [Some(6), None, Some(7)]
    );
}

#[tokio::test]
async fn a_conflicted_file_diffs_against_our_side() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "base\n", "base").await;
    repo.git_ok(&["checkout", "-q", "-b", "other"]).await;
    repo.commit_file("f.txt", "theirs\n", "theirs").await;
    repo.git_ok(&["checkout", "-q", "main"]).await;
    repo.commit_file("f.txt", "ours\n", "ours").await;
    let merge = repo
        .git
        .command(["merge", "other"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert!(!merge.success());

    // A plain two-way diff (not a combined `@@@` diff): ours → file with conflict markers.
    let diff = repo.diff("f.txt", DiffSource::Conflict).await;
    assert_eq!(diff.hunks.len(), 1);
    let text: Vec<String> = diff
        .lines
        .iter()
        .map(|line| String::from_utf8_lossy(diff.line_bytes(line)).into_owned())
        .collect();
    assert!(text.iter().any(|line| line.starts_with("<<<<<<<")));
    assert!(text.contains(&"theirs".to_owned()));

    // Staging the file marks the conflict resolved.
    repo.write("f.txt", "resolved\n");
    stage_paths(&repo.git, &repo.path, &paths(&["f.txt"]))
        .await
        .unwrap();
    let status = repo.status().await;
    assert_eq!(status.entries[0].kind, EntryKind::Tracked);
    assert_eq!(status.entries[0].index, Change::Modified);
}

#[tokio::test]
async fn a_ten_megabyte_diff_parses() {
    let repo = TestRepo::init().await;
    let line = "x".repeat(99);
    let old: String = (0..100_000).map(|n| format!("{n:07} {line}\n")).collect();
    repo.commit_file("big.txt", &old, "base").await;
    let new: String = (0..100_000).map(|n| format!("{n:07} {line}!\n")).collect();
    repo.write("big.txt", &new);

    let diff = repo.diff("big.txt", DiffSource::Unstaged).await;
    assert!(
        diff.data.len() > 20 * 1024 * 1024,
        "{} bytes",
        diff.data.len()
    );
    assert_eq!((diff.added, diff.removed), (100_000, 100_000));
    assert_eq!(diff.lines.len(), 200_000);
    assert_eq!(diff.lines.last().unwrap().new_no, 100_000);
}

// ----- hunks and lines ------------------------------------------------------------------------

/// Thirty lines with changes near the top and near the bottom: two separate hunks.
async fn two_hunk_repo() -> TestRepo {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", &numbered(30), "base").await;
    let edited = numbered(30)
        .replace("line 3\n", "line three\n")
        .replace("line 27\n", "line 27\nline 27b\n")
        .replace("line 28\n", "");
    repo.write("f.txt", &edited);
    repo
}

#[tokio::test]
async fn stage_one_hunk_then_unstage_it() {
    let repo = two_hunk_repo().await;
    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;
    assert_eq!(diff.hunks.len(), 2);

    // Stage only the second hunk.
    let second = diff.hunks[1].lines.clone();
    let patch = build_patch(&diff, "f.txt", PatchDirection::Forward, |i| {
        second.contains(&(i as u32))
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();

    let staged = repo.staged("f.txt").await;
    assert!(staged.contains("line 3\n") && !staged.contains("line three"));
    assert!(staged.contains("line 27b\n") && !staged.contains("line 28\n"));
    // The working tree is untouched and still has the first hunk as an unstaged change.
    let rest = repo.diff("f.txt", DiffSource::Unstaged).await;
    assert_eq!(rest.hunks.len(), 1);
    assert_eq!((rest.added, rest.removed), (1, 1));

    // Unstage it again from the staged diff.
    let staged_diff = repo.diff("f.txt", DiffSource::Staged).await;
    assert_eq!(staged_diff.hunks.len(), 1);
    let patch = build_patch(&staged_diff, "f.txt", PatchDirection::Reverse, |_| true).unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::IndexReverse)
        .await
        .unwrap();
    assert_eq!(repo.staged("f.txt").await, numbered(30));
    assert!(repo.diff("f.txt", DiffSource::Staged).await.is_empty());
}

#[tokio::test]
async fn stage_and_unstage_single_lines() {
    let repo = two_hunk_repo().await;
    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;

    // Stage the added `line 27b` (second hunk) and nothing else: neither the removal of
    // `line 28` next to it, nor the first hunk.
    let pick = lines_with(&diff, &["line 27b"]);
    assert_eq!(pick.len(), 1);
    let patch = build_patch(&diff, "f.txt", PatchDirection::Forward, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    let staged = repo.staged("f.txt").await;
    // In the diff the removal of `line 28` comes before the addition, so with the removal
    // left out the new line lands after the line that stays.
    assert_eq!(
        staged,
        numbered(30).replace("line 28\n", "line 28\nline 27b\n")
    );

    // Now stage the replacement in the first hunk as a pair (one removal, one addition).
    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;
    let pick = lines_with(&diff, &["line 3", "line three"]);
    assert_eq!(pick.len(), 2);
    let patch = build_patch(&diff, "f.txt", PatchDirection::Forward, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    let staged = repo.staged("f.txt").await;
    assert!(staged.contains("line three\n") && staged.contains("line 28\n"));

    // Unstage just the `line three` addition: the index gets neither old nor new line 3.
    let staged_diff = repo.diff("f.txt", DiffSource::Staged).await;
    let pick = lines_with(&staged_diff, &["line three"]);
    let patch = build_patch(&staged_diff, "f.txt", PatchDirection::Reverse, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::IndexReverse)
        .await
        .unwrap();
    let staged = repo.staged("f.txt").await;
    assert!(!staged.contains("line three") && !staged.contains("line 3\n"));
    assert!(staged.contains("line 27b\n"));
    // Through all of it the working tree file never changed.
    assert!(repo.read("f.txt").contains("line three\n"));
}

#[tokio::test]
async fn crlf_content_round_trips() {
    let repo = TestRepo::init().await;
    // No autocrlf in the isolated config: the CRs are part of the committed content.
    repo.commit_file("win.txt", "one\r\ntwo\r\nthree\r\n", "base")
        .await;
    repo.write("win.txt", "one\r\nTWO\r\nthree\r\nfour\r\n");

    let diff = repo.diff("win.txt", DiffSource::Unstaged).await;
    assert!(diff.line_bytes(&diff.lines[0]).ends_with(b"\r"));
    let pick = lines_with(&diff, &["two\r", "TWO\r"]);
    assert_eq!(pick.len(), 2);
    let patch = build_patch(&diff, "win.txt", PatchDirection::Forward, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    assert_eq!(repo.staged("win.txt").await, "one\r\nTWO\r\nthree\r\n");
}

#[tokio::test]
async fn files_without_a_final_newline() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "a\nb\nlast", "base").await;
    repo.write("f.txt", "A\nb\nlast\nmore");

    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;
    assert!(diff.lines.iter().any(|line| line.no_newline));

    // A line away from the end stages on its own.
    let pick = lines_with(&diff, &["a", "A"]);
    let patch = build_patch(&diff, "f.txt", PatchDirection::Forward, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    assert_eq!(repo.staged("f.txt").await, "A\nb\nlast");

    // The end-of-file change cannot be split …
    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;
    let pick = lines_with(&diff, &["more"]);
    assert_eq!(
        build_patch(&diff, "f.txt", PatchDirection::Forward, |i| pick
            .contains(&i)),
        Err(PatchError::EndOfFile)
    );
    // … but stages as a whole.
    let patch = build_patch(&diff, "f.txt", PatchDirection::Forward, |_| true).unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    assert_eq!(repo.staged("f.txt").await, "A\nb\nlast\nmore");
}

#[tokio::test]
async fn part_of_a_new_file_and_part_of_a_deletion() {
    let repo = TestRepo::init().await;
    repo.commit_file("old.txt", "one\ntwo\nthree\n", "base")
        .await;

    // Untracked file: stage the middle line only. The file enters the index with that line.
    repo.write("dir/new file.txt", "alpha\nbeta\ngamma\n");
    let diff = repo.diff("dir/new file.txt", DiffSource::Untracked).await;
    let pick = lines_with(&diff, &["beta"]);
    let patch = build_patch(&diff, "dir/new file.txt", PatchDirection::Forward, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    assert_eq!(repo.staged("dir/new file.txt").await, "beta\n");

    // Stage the rest, then unstage one line of the (now fully staged) new file.
    stage_paths(&repo.git, &repo.path, &paths(&["dir/new file.txt"]))
        .await
        .unwrap();
    let staged = repo.diff("dir/new file.txt", DiffSource::Staged).await;
    assert!(staged.new_file);
    let pick = lines_with(&staged, &["gamma"]);
    let patch = build_patch(&staged, "dir/new file.txt", PatchDirection::Reverse, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::IndexReverse)
        .await
        .unwrap();
    assert_eq!(repo.staged("dir/new file.txt").await, "alpha\nbeta\n");

    // Unstaging every line removes the file from the index again.
    let staged = repo.diff("dir/new file.txt", DiffSource::Staged).await;
    let patch = build_patch(&staged, "dir/new file.txt", PatchDirection::Reverse, |_| {
        true
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::IndexReverse)
        .await
        .unwrap();
    let status = repo.status().await;
    assert_eq!(
        status
            .entries
            .iter()
            .find(|e| e.path == "dir/new file.txt")
            .unwrap()
            .kind,
        EntryKind::Untracked
    );

    // Deleted file: stage the removal of one line only; the file stays in the index.
    repo.remove("old.txt");
    let diff = repo.diff("old.txt", DiffSource::Unstaged).await;
    assert!(diff.deleted_file);
    let pick = lines_with(&diff, &["two"]);
    let patch = build_patch(&diff, "old.txt", PatchDirection::Forward, |i| {
        pick.contains(&i)
    })
    .unwrap();
    apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index)
        .await
        .unwrap();
    assert_eq!(repo.staged("old.txt").await, "one\nthree\n");
}

#[tokio::test]
async fn a_stale_patch_is_rejected_and_changes_nothing() {
    let repo = two_hunk_repo().await;
    let diff = repo.diff("f.txt", DiffSource::Unstaged).await;
    let patch = build_patch(&diff, "f.txt", PatchDirection::Forward, |_| true).unwrap();

    // The index moves on underneath the patch.
    repo.git_ok(&["add", "f.txt"]).await;
    let before = repo.staged("f.txt").await;
    let result = apply_patch(&repo.git, &repo.path, &patch, ApplyTo::Index).await;
    assert!(
        matches!(result, Err(facet_git::GitError::Failed { .. })),
        "{result:?}"
    );
    assert_eq!(repo.staged("f.txt").await, before);
}
