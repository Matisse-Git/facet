//! Golden tests: parsers run against byte-exact output captured from real git.
//!
//! Each fixture in `tests/golden/` was produced by the command noted beside it. To add a
//! parser, capture its real output into a new fixture and assert the full parsed value here.

use facet_git::parse::{
    parse_commit_summary, parse_file_diff, parse_status, BranchInfo, Change, CommitSummary,
    EntryKind, LineKind, StatusEntry,
};

/// `git --no-pager -c color.ui=never log -1 -z --no-show-signature
///  --format=%H%x00%h%x00%an%x00%at%x00%s HEAD`
/// in a repo whose only commit has a non-ASCII subject and a body (the body must not leak in).
#[test]
fn log_commit_summary() {
    let raw = include_bytes!("golden/log_commit_summary.bin");
    assert_eq!(
        parse_commit_summary(raw).unwrap(),
        CommitSummary {
            oid: "6d447b2e771578b2779214d187215b9ab5865e4c".into(),
            short_oid: "6d447b2".into(),
            author: "Ada Lovelace".into(),
            author_time: 1_700_000_000,
            subject: "Add \"Niveau été\" lighting pass — 光".into(),
        }
    );
}

/// `git --no-pager -c color.ui=never status --porcelain=v2 -z --branch --untracked-files=all`
/// in a repo one commit ahead of its upstream, with: a file deleted in the working tree, a
/// staged rename (names with spaces), a file with staged *and* unstaged changes, a modified
/// file with a non-ASCII path, and an untracked file in a new directory.
#[test]
fn status_porcelain_v2() {
    let raw = include_bytes!("golden/status_v2.bin");
    let status = parse_status(raw).unwrap();

    assert_eq!(
        status.branch,
        BranchInfo {
            oid: Some("ac5c141952bd05a6be2992907d0cd3026eafd18e".into()),
            head: Some("main".into()),
            upstream: Some("upstream-like".into()),
            ahead: Some(1),
            behind: Some(0),
        }
    );

    let tracked =
        |path: &str, index: Change, worktree: Change, head: &str, staged: &str| StatusEntry {
            path: path.into(),
            orig_path: None,
            kind: EntryKind::Tracked,
            index,
            worktree,
            conflict: None,
            submodule: false,
            head_oid: Some(head.into()),
            index_oid: Some(staged.into()),
        };
    assert_eq!(
        status.entries,
        vec![
            tracked(
                "gone.txt",
                Change::Unmodified,
                Change::Deleted,
                "b023018cabc396e7692c70bbf5784a93d3f738ab",
                "b023018cabc396e7692c70bbf5784a93d3f738ab",
            ),
            StatusEntry {
                orig_path: Some("old name.txt".into()),
                ..tracked(
                    "new name.txt",
                    Change::Renamed,
                    Change::Unmodified,
                    "2472d3e9407d03819431512e95a5df165ad9bbbf",
                    "2472d3e9407d03819431512e95a5df165ad9bbbf",
                )
            },
            tracked(
                "plain.txt",
                Change::Modified,
                Change::Modified,
                "de980441c3ab03a8c07dda1ad27b8a11f39deb1e",
                "7be73ce3c1b1cdaea86e8168dfee8575175953bf",
            ),
            tracked(
                "été/光.txt",
                Change::Unmodified,
                Change::Modified,
                "3d5dd131ab63db03f55cd1ae5f2668e927f2c293",
                "3d5dd131ab63db03f55cd1ae5f2668e927f2c293",
            ),
            StatusEntry {
                path: "new dir/[a] #1.txt".into(),
                orig_path: None,
                kind: EntryKind::Untracked,
                index: Change::Unmodified,
                worktree: Change::Unmodified,
                conflict: None,
                submodule: false,
                head_oid: None,
                index_oid: None,
            },
        ]
    );
}

/// `git --no-pager -c color.ui=never --literal-pathspecs diff --no-ext-diff --no-textconv
///  --no-color --numstat -z -p --src-prefix=a/ --dst-prefix=b/ -- "été/光.txt"`
/// for a file whose second line changed and which gained a last line without a newline. The
/// patch header quotes the non-ASCII path; the numstat record (thanks to `-z`) does not.
#[test]
fn diff_numstat_and_patch() {
    let raw = include_bytes!("golden/diff_numstat_patch.bin");
    let diff = parse_file_diff(raw.to_vec()).unwrap();

    assert!(!diff.binary && !diff.new_file && !diff.deleted_file);
    assert_eq!((diff.added, diff.removed), (2, 1));
    assert_eq!(
        (diff.old_mode, diff.new_mode),
        (Some(0o100644), Some(0o100644))
    );
    assert_eq!(diff.similarity, None);

    assert_eq!(diff.hunks.len(), 1);
    let hunk = &diff.hunks[0];
    assert_eq!(
        (
            hunk.old_start,
            hunk.old_count,
            hunk.new_start,
            hunk.new_count
        ),
        (1, 3, 1, 4)
    );
    assert_eq!(diff.header_bytes(hunk), b"@@ -1,3 +1,4 @@");
    assert_eq!(hunk.lines, 0..5);

    let lines: Vec<(LineKind, &str, u32, u32, bool)> = diff
        .lines
        .iter()
        .map(|line| {
            (
                line.kind,
                std::str::from_utf8(diff.line_bytes(line)).unwrap(),
                line.old_no,
                line.new_no,
                line.no_newline,
            )
        })
        .collect();
    assert_eq!(
        lines,
        vec![
            (LineKind::Context, "un", 1, 1, false),
            (LineKind::Removed, "deux", 2, 0, false),
            (LineKind::Added, "DEUX", 0, 2, false),
            (LineKind::Context, "trois", 3, 3, false),
            (LineKind::Added, "quatre", 0, 4, true),
        ]
    );
}
