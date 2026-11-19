//! Golden tests: parsers run against byte-exact output captured from real git.
//!
//! Each fixture in `tests/golden/` was produced by the command noted beside it. To add a
//! parser, capture its real output into a new fixture and assert the full parsed value here.

use facet_git::graph::{ascii, layout};
use facet_git::parse::{
    parse_blame, parse_changed_files, parse_check_attr, parse_commit_detail, parse_commit_summary,
    parse_file_diff, parse_file_log, parse_gitlinks, parse_gitmodules, parse_locks, parse_log,
    parse_new_lock, parse_reflog, parse_refs, parse_remotes, parse_stashes, parse_status,
    parse_verified_locks, parse_worktrees, BranchInfo, Change, ChangedFile, CommitDetail,
    CommitSummary, EntryKind, GitRef, LfsAttributes, LineKind, RefKind, Remote, StatusEntry,
    Submodule, Track,
};

// ----- M2 fixtures -------------------------------------------------------------------------
//
// All captured from one repository (fixed identities and clock, see each test):
//   Initial — été 光  →  main: Main work, topic: Topic work (with a body)  →  Merge 'topic'
//   → pushed to `origin` → Rename and binary (keep.txt→kept.txt + bin.dat) → tag v1.0
//   (annotated) on it, tag `light` on the merge → two stashes (one with untracked files).
// Remotes: `origin` (../server.git) and `team/fork` (separate push URL).

const MERGE: &str = "4659738fb8a22648163a12e5091ac57eb2140565";
const HEAD: &str = "41c86cbbab5fdf1e7d6a35b3dbc6c99774fe53cc";
const TOPIC: &str = "5c9c643eb620fef76b650d6e471658056fedec6a";
const MAIN_WORK: &str = "2a0dca183f279d0c3c83c78c80bc2648d4f9a194";
const INITIAL: &str = "2a4d9db3aa046a3324a9155957c6627d95d8d0e8";

/// `git log --date-order -z --format=%H%x00%P%x00%an%x00%at%x00%s --no-show-signature
///  --exclude=refs/stash --all` (`facet_git::LOG_ARGS`)
#[test]
fn log_graph() {
    let commits = parse_log(include_bytes!("golden/log_graph.bin")).unwrap();
    let summary: Vec<(String, Vec<String>, &str, i64)> = commits
        .iter()
        .map(|c| {
            (
                c.oid.to_string(),
                c.parents.iter().map(ToString::to_string).collect(),
                &*c.subject,
                c.time,
            )
        })
        .collect();
    let summary: Vec<(&str, Vec<&str>, &str, i64)> = summary
        .iter()
        .map(|(oid, parents, subject, time)| {
            (
                oid.as_str(),
                parents.iter().map(String::as_str).collect(),
                *subject,
                *time,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (HEAD, vec![MERGE], "Rename and binary", 1_700_000_300),
            (
                MERGE,
                vec![MAIN_WORK, TOPIC],
                "Merge branch 'topic'",
                1_700_000_240
            ),
            (MAIN_WORK, vec![INITIAL], "Main work", 1_700_000_180),
            (TOPIC, vec![INITIAL], "Topic work", 1_700_000_120),
            (INITIAL, vec![], "Initial — été 光", 1_700_000_060),
        ]
    );
    assert!(commits.iter().all(|c| &*c.author == "Ada Lovelace"));
    assert_eq!(ascii(&layout(&commits)), vec!["*", "M\\", "*|", "|*", "*/"]);
}

/// `git for-each-ref --format=%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)%00
///  %(upstream)%00%(upstream:trackshort)%00%(symref)%00%(creatordate:unix)` (`REFS_FORMAT`)
#[test]
fn refs_listing() {
    let remotes = vec!["origin".to_owned(), "team/fork".to_owned()];
    let refs = parse_refs(include_bytes!("golden/refs.bin"), &remotes).unwrap();
    assert_eq!(
        refs,
        vec![
            GitRef {
                full_name: "refs/heads/main".into(),
                name: "main".into(),
                kind: RefKind::Local,
                oid: HEAD.into(),
                tag_object: None,
                head: true,
                upstream: Some("origin/main".into()),
                track: Track::Ahead,
                date: 1_700_000_300,
                author: String::new(),
            },
            GitRef {
                full_name: "refs/heads/topic".into(),
                name: "topic".into(),
                kind: RefKind::Local,
                oid: TOPIC.into(),
                tag_object: None,
                head: false,
                upstream: None,
                track: Track::None,
                date: 1_700_000_120,
                author: String::new(),
            },
            GitRef {
                full_name: "refs/remotes/origin/main".into(),
                name: "origin/main".into(),
                kind: RefKind::Remote {
                    remote: "origin".into()
                },
                oid: MERGE.into(),
                tag_object: None,
                head: false,
                upstream: None,
                track: Track::None,
                date: 1_700_000_240,
                author: String::new(),
            },
            // refs/stash is skipped: stashes have their own list.
            GitRef {
                full_name: "refs/tags/light".into(),
                name: "light".into(),
                kind: RefKind::Tag,
                oid: MERGE.into(),
                tag_object: None,
                head: false,
                upstream: None,
                track: Track::None,
                date: 1_700_000_240,
                author: String::new(),
            },
            GitRef {
                full_name: "refs/tags/v1.0".into(),
                name: "v1.0".into(),
                kind: RefKind::Tag,
                oid: HEAD.into(),
                tag_object: Some("8693ab1096bf5a05de599db8774cc0c512f2d2dd".into()),
                head: false,
                upstream: None,
                track: Track::None,
                date: 1_700_000_360,
                author: String::new(),
            },
        ]
    );
}

/// `git config -z --get-regexp ^remote\.`
#[test]
fn remotes_config() {
    let remotes = parse_remotes(include_bytes!("golden/remotes.bin")).unwrap();
    assert_eq!(
        remotes,
        vec![
            Remote {
                name: "origin".into(),
                url: Some("../server.git".into()),
                push_url: None,
            },
            Remote {
                name: "team/fork".into(),
                url: Some("https://example.invalid/fork.git".into()),
                push_url: Some("ssh://git@example.invalid/fork.git".into()),
            },
        ]
    );
}

/// `git stash list -z --format=%H%x00%gd%x00%gs%x00%ct%x00%P` (`STASH_FORMAT`)
#[test]
fn stash_list() {
    type Summary<'a> = (usize, &'a str, &'a str, Option<&'a str>, bool, i64);
    let stashes = parse_stashes(include_bytes!("golden/stash_list.bin")).unwrap();
    let summary: Vec<Summary> = stashes
        .iter()
        .map(|s| {
            (
                s.index,
                s.selector.as_str(),
                s.message.as_str(),
                s.base.as_deref(),
                s.untracked,
                s.time,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                0,
                "stash@{0}",
                "On main: with untracked",
                Some(HEAD),
                true,
                1_700_000_480
            ),
            (
                1,
                "stash@{1}",
                "On main: wip one",
                Some(HEAD),
                false,
                1_700_000_420
            ),
        ]
    );
    assert_eq!(stashes[0].oid, "74308997f7ea658b8af715aae3367d5aa229387e");
}

/// `git log -1 -z --no-show-signature --format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce
///  %x00%ct%x00%B <topic commit> --` (`DETAIL_FORMAT`)
#[test]
fn commit_detail() {
    let detail = parse_commit_detail(include_bytes!("golden/commit_detail.bin")).unwrap();
    assert_eq!(
        detail,
        CommitDetail {
            oid: TOPIC.into(),
            parents: vec![INITIAL.into()],
            author: "Ada Lovelace".into(),
            author_email: "ada@example.invalid".into(),
            author_time: 1_700_000_120,
            committer: "Grace Hopper".into(),
            committer_email: "grace@example.invalid".into(),
            commit_time: 1_700_000_120,
            message: "Topic work\n\nWith a body.".into(),
        }
    );
    assert_eq!(detail.subject(), "Topic work");
    assert_eq!(detail.body(), "With a body.");
}

/// `git diff-tree -r -z -M --raw --numstat --no-commit-id HEAD~1 HEAD` on "Rename and binary".
#[test]
fn diff_tree_changed_files() {
    let files = parse_changed_files(include_bytes!("golden/diff_tree.bin")).unwrap();
    assert_eq!(
        files,
        vec![
            ChangedFile {
                path: "bin.dat".into(),
                orig_path: None,
                status: 'A',
                old_oid: None,
                new_oid: Some("bdc955b7b2e610ad5a72302b139a2e6cb325519a".into()),
                added: None,
                removed: None,
                submodule: false,
            },
            ChangedFile {
                path: "kept.txt".into(),
                orig_path: Some("keep.txt".into()),
                status: 'R',
                old_oid: Some("6fe8acc87876b59a7c33d8f122933869f19f39c4".into()),
                new_oid: Some("07232412ef3f0b2239c3e3c9b40b6f9f38468275".into()),
                added: Some(2),
                removed: Some(1),
                submodule: false,
            },
        ]
    );
}

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

// ----- M3 fixtures -------------------------------------------------------------------------
//
// One repository, author "Ann Author" except where noted, committer clock = author clock:
//   Add notes (`notes été.txt`, t=…100) → Edit: uppercase two (…200) → Move notes into docs
//   (rename to docs/notes.txt, …300) → Prepend zero (Bob Builder, …400) → checkout topic and
//   back (…500/…600) → reset --soft HEAD~1 (…700) → commit "Prepend zero (again)" (…800) →
//   amend to "Prepend zero, amended" (…900).

const M3_AMENDED: &str = "431abd0893e305f53638a04cc110afd6ac7479d8";
const M3_MOVE: &str = "f698faec8ff80f69191c28bb67ce71185b5b6242";
const M3_EDIT: &str = "4c786b6765fff3de1bcd5fcd4b7e9c4d384c215b";
const M3_ADD: &str = "f7d00a3fbd1c610c2fadeaacf9b4aceba67d665b";

/// `git log -g -z --date=unix --no-show-signature
///  --format=%H%x00%P%x00%an%x00%at%x00%s%x00%gd%x00%gs -n50 HEAD --`
#[test]
fn reflog_entries() {
    let entries = parse_reflog(include_bytes!("golden/reflog.bin")).unwrap();
    assert_eq!(entries.len(), 9);
    let first = &entries[0];
    assert_eq!(first.commit.oid, M3_AMENDED);
    assert_eq!(first.commit.parents, [M3_MOVE]);
    assert_eq!(first.commit.time, 1_700_000_800);
    assert_eq!(first.time, 1_700_000_900);
    assert_eq!(first.action, "commit (amend)");
    assert_eq!(first.message, "Prepend zero, amended");
    assert_eq!(&*first.commit.subject, "Prepend zero, amended");
    let reset = &entries[2];
    assert_eq!(
        (reset.index, reset.commit.oid.to_string()),
        (2, M3_MOVE.to_owned())
    );
    assert_eq!(&*reset.commit.author, "Ann Author");
    assert_eq!(reset.action, "reset");
    assert_eq!(reset.message, "moving to HEAD~1");
    assert_eq!(reset.selector("HEAD"), "HEAD@{2}");
    assert_eq!(entries[3].message, "moving from topic to main");
    let initial = entries.last().unwrap();
    assert_eq!(initial.commit.oid, M3_ADD);
    assert!(initial.commit.parents.is_empty());
    assert_eq!(initial.action, "commit (initial)");
    assert_eq!(initial.time, 1_700_000_100);
}

/// `git --literal-pathspecs log --follow -z -M --name-status --no-show-signature
///  --format=%H%x00%P%x00%an%x00%at%x00%s HEAD -- docs/notes.txt`
#[test]
fn file_log_follows_renames() {
    let entries = parse_file_log(include_bytes!("golden/file_log.bin"), "docs/notes.txt").unwrap();
    let summary: Vec<(String, char, &str, Option<&str>)> = entries
        .iter()
        .map(|e| {
            (
                e.commit.oid.to_string(),
                e.status,
                e.path.as_str(),
                e.orig_path.as_deref(),
            )
        })
        .collect();
    let summary: Vec<(&str, char, &str, Option<&str>)> = summary
        .iter()
        .map(|(oid, status, path, orig)| (oid.as_str(), *status, *path, *orig))
        .collect();
    assert_eq!(
        summary,
        vec![
            (M3_AMENDED, 'M', "docs/notes.txt", None),
            (M3_MOVE, 'R', "docs/notes.txt", Some("notes été.txt")),
            (M3_EDIT, 'M', "notes été.txt", None),
            (M3_ADD, 'A', "notes été.txt", None),
        ]
    );
    assert_eq!(&*entries[0].commit.subject, "Prepend zero, amended");
    assert_eq!(entries[1].commit.parents, [M3_EDIT]);
    assert!(entries[3].commit.parents.is_empty());
    assert_eq!(entries[3].commit.time, 1_700_000_100);
}

/// `git blame --incremental -M HEAD -- docs/notes.txt`
#[test]
fn blame_incremental() {
    let (commits, ranges) = parse_blame(include_bytes!("golden/blame.bin")).unwrap();
    let oids: Vec<&str> = commits.iter().map(|c| c.oid.as_str()).collect();
    assert_eq!(oids, vec![M3_AMENDED, M3_EDIT, M3_ADD]);
    assert_eq!(commits[0].summary, "Prepend zero, amended");
    assert_eq!(commits[0].author, "Ann Author");
    assert_eq!(commits[0].time, 1_700_000_800);
    assert!(!commits[0].boundary && commits[2].boundary);
    assert!(commits.iter().all(|c| !c.uncommitted));
    // Every one of the five lines is covered exactly once.
    let mut lines: Vec<(u32, usize)> = ranges
        .iter()
        .flat_map(|r| (r.line..r.line + r.count).map(move |line| (line, r.commit)))
        .collect();
    lines.sort_unstable();
    assert_eq!(lines, vec![(1, 0), (2, 2), (3, 1), (4, 2), (5, 1)]);
    let renamed = ranges.iter().find(|r| r.commit == 1).unwrap();
    assert_eq!(renamed.filename, "notes été.txt");
    assert_eq!((renamed.line, renamed.orig_line), (3, 2));
}

// ----- M4 fixtures -------------------------------------------------------------------------
//
// One repository `app` (fixed identities and clock) with `.gitattributes` marking *.psd as LFS
// + lockable and *.umap as LFS, a submodule `libs/engine été` (url ../lib, branch main), and
// worktrees: `wt feature` (branch feature), `wt-detached` (detached, locked "on a USB disk"),
// `wt-gone` (branch gone, folder deleted → prunable). LFS lock JSON comes from git-lfs 3.5.1
// talking to the lock server in `tests/m4.rs` (one lock of ours, one of Ada's).

const APP_HEAD: &str = "72c9d845d642be58cd3c047d54bbbdd79f8eb82e";
const LIB_HEAD: &str = "cc1295c47a986e10e7d498a081b978d90c7e11ae";

/// `git worktree list --porcelain -z`
#[test]
fn worktree_list() {
    let list = parse_worktrees(include_bytes!("golden/worktrees.bin")).unwrap();
    type Summary<'a> = (&'a str, Option<&'a str>, bool, bool, Option<&'a str>, bool);
    let summary: Vec<Summary> = list
        .iter()
        .map(|w| {
            (
                w.name(),
                w.branch_name(),
                w.detached,
                w.main,
                w.locked.as_deref(),
                w.prunable.is_some(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("app", Some("main"), false, true, None, false),
            ("wt feature", Some("feature"), false, false, None, false),
            (
                "wt-detached",
                None,
                true,
                false,
                Some("on a USB disk"),
                false
            ),
            ("wt-gone", Some("gone"), false, false, None, true),
        ]
    );
    assert!(list.iter().all(|w| w.head.as_deref() == Some(APP_HEAD)));
    assert!(list[0].path.ends_with("/golden/app"));
}

/// `git config -z --file .gitmodules --get-regexp ^submodule\.` and
/// `git ls-files -z --stage -- README.md ":(literal)libs/engine été"`
#[test]
fn submodule_declarations_and_gitlinks() {
    let modules = parse_gitmodules(include_bytes!("golden/gitmodules.bin")).unwrap();
    assert_eq!(
        modules,
        vec![Submodule {
            name: "libs/engine été".into(),
            path: "libs/engine été".into(),
            url: Some("../lib".into()),
            branch: Some("main".into()),
            recorded: None,
            populated: false,
        }]
    );
    let links = parse_gitlinks(include_bytes!("golden/gitlinks.bin")).unwrap();
    assert_eq!(
        links,
        vec![("libs/engine été".to_owned(), LIB_HEAD.to_owned())]
    );
}

/// `printf 'Art/hero.psd\0Maps/a b.umap\0src/main.rs\0' | git check-attr -z --stdin filter
///  lockable`
#[test]
fn lfs_check_attr() {
    let attrs = parse_check_attr(include_bytes!("golden/check_attr.bin")).unwrap();
    let lfs = |lfs, lockable| LfsAttributes { lfs, lockable };
    assert_eq!(
        attrs,
        vec![
            ("Art/hero.psd".to_owned(), lfs(true, true)),
            ("Maps/a b.umap".to_owned(), lfs(true, false)),
            ("src/main.rs".to_owned(), lfs(false, false)),
        ]
    );
}

/// `git lfs locks --verify --json`, `git lfs locks --json`, `git lfs lock --json -- Art/other.psd`
#[test]
fn lfs_lock_json() {
    let verified = parse_verified_locks(include_bytes!("golden/lfs_locks_verify.bin")).unwrap();
    let summary: Vec<(&str, &str, &str, Option<bool>)> = verified
        .iter()
        .map(|l| (l.id.as_str(), l.path.as_str(), l.owner.as_str(), l.ours))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("1", "Art/hero.psd", "Facet Test", Some(true)),
            ("100", "Maps/level.umap", "Ada", Some(false)),
        ]
    );
    assert_eq!(verified[0].locked_at, "2026-11-18T10:00:00Z");
    let plain = parse_locks(include_bytes!("golden/lfs_locks.bin")).unwrap();
    assert_eq!(plain.len(), 2);
    assert!(plain.iter().all(|l| l.ours.is_none()));
    let new = parse_new_lock(include_bytes!("golden/lfs_lock.bin")).unwrap();
    assert_eq!((new.id.as_str(), new.path.as_str()), ("2", "Art/other.psd"));
}
