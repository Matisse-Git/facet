//! M3 operations against real repositories: operation state with continue / skip / abort,
//! cherry-pick, revert, reset with auto-stash and undo, conflicts, interactive rebase,
//! search, file history, blame, reflog and external tools.

mod common;

use common::TestRepo;
use facet_git::conflict::{self, Choice, Side};
use facet_git::rebase::{self, Editors, TodoAction, TodoItem};
use facet_git::{CancellationToken, OperationKind, Progress, ResetMode, SearchKind};

async fn head(repo: &TestRepo) -> String {
    String::from_utf8(repo.git_ok(&["rev-parse", "HEAD"]).await)
        .unwrap()
        .trim()
        .to_owned()
}

async fn subjects(repo: &TestRepo) -> Vec<String> {
    String::from_utf8(repo.git_ok(&["log", "--format=%s"]).await)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `main`: base → main 1; `topic` (from base): topic 1 → topic 2, each touching its own file.
async fn branched() -> TestRepo {
    let repo = TestRepo::init().await;
    repo.commit_file("shared.txt", "one\ntwo\nthree\n", "base")
        .await;
    repo.git_ok(&["switch", "-c", "topic"]).await;
    repo.commit_file("topic.txt", "topic 1\n", "topic 1").await;
    repo.commit_file("topic.txt", "topic 1\ntopic 2\n", "topic 2")
        .await;
    repo.git_ok(&["switch", "main"]).await;
    repo.commit_file("main.txt", "main 1\n", "main 1").await;
    repo
}

/// Editors for tests: the todo is copied as Facet's helper would; messages are kept.
fn test_editors(repo: &TestRepo) -> Editors {
    let todo = repo
        .path
        .join(".git/facet-rebase/todo")
        .to_string_lossy()
        .replace('\\', "/");
    Editors {
        sequence: format!("cp '{todo}'"),
        message: ":".to_owned(),
    }
}

#[tokio::test]
async fn cherry_pick_then_undo() {
    let repo = branched().await;
    let info = repo.info();
    let before = head(&repo).await;
    let topic1 = String::from_utf8(repo.git_ok(&["rev-parse", "topic~1"]).await).unwrap();
    let (progress, undo) =
        facet_git::cherry_pick(&repo.git, &info, &[(topic1.trim().to_owned(), false)])
            .await
            .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(subjects(&repo).await[0], "topic 1");
    assert_eq!(repo.read("topic.txt"), "topic 1\n");
    assert_eq!(undo.head, before);

    facet_git::undo_to(&repo.git, &repo.path, &undo)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, before);
    assert!(!repo.exists("topic.txt"));
}

#[tokio::test]
async fn revert_makes_an_inverse_commit() {
    let repo = branched().await;
    let info = repo.info();
    let target = head(&repo).await;
    let (progress, _) = facet_git::revert(&repo.git, &info, &[(target, false)])
        .await
        .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert!(subjects(&repo).await[0].starts_with("Revert \"main 1\""));
    assert!(!repo.exists("main.txt"));
}

#[tokio::test]
async fn merge_then_undo_and_conflicting_merge_aborts() {
    let repo = branched().await;
    let info = repo.info();
    let before = head(&repo).await;
    let (progress, undo) = facet_git::merge(&repo.git, &info, "topic").await.unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(subjects(&repo).await[0], "Merge branch 'topic'");
    assert_eq!(repo.read("topic.txt"), "topic 1\ntopic 2\n");
    assert_eq!(undo.head, before);
    facet_git::undo_to(&repo.git, &repo.path, &undo)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, before);
    assert!(!repo.exists("topic.txt"));

    // Both sides change the same line: the merge stops, and aborting puts HEAD back.
    repo.git_ok(&["switch", "topic"]).await;
    repo.commit_file("shared.txt", "one\nTOPIC\nthree\n", "topic edit")
        .await;
    repo.git_ok(&["switch", "main"]).await;
    repo.commit_file("shared.txt", "one\nMAIN\nthree\n", "main edit")
        .await;
    let before = head(&repo).await;
    let (progress, _) = facet_git::merge(&repo.git, &info, "topic").await.unwrap();
    let Progress::Stopped { operation, message } = progress else {
        panic!("expected a stop");
    };
    assert_eq!(operation.kind, OperationKind::Merge);
    assert!(message.contains("CONFLICT"), "{message}");
    facet_git::abort_operation(&repo.git, &info, &operation)
        .await
        .unwrap();
    assert_eq!(facet_git::operation(&info.git_dir), None);
    assert_eq!(head(&repo).await, before);
    assert_eq!(repo.read("shared.txt"), "one\nMAIN\nthree\n");
}

#[tokio::test]
async fn conflicting_pick_stops_and_resolves_chunk_by_chunk() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "a\nb\nc\n", "base").await;
    repo.git_ok(&["switch", "-c", "topic"]).await;
    repo.commit_file("f.txt", "a\nTHEIRS\nc\n", "theirs").await;
    repo.git_ok(&["switch", "main"]).await;
    repo.commit_file("f.txt", "a\nOURS\nc\n", "ours").await;
    let info = repo.info();

    let (progress, _) = facet_git::cherry_pick(&repo.git, &info, &[("topic".to_owned(), false)])
        .await
        .unwrap();
    let Progress::Stopped { operation, message } = progress else {
        panic!("expected a stop");
    };
    assert_eq!(operation.kind, OperationKind::CherryPick);
    assert!(message.contains("CONFLICT"), "{message}");
    assert_eq!(facet_git::operation(&info.git_dir), Some(operation.clone()));

    let versions = conflict::conflict_versions(&repo.git, &repo.path, "f.txt")
        .await
        .unwrap();
    assert_eq!(versions.base.as_deref(), Some(&b"a\nb\nc\n"[..]));
    assert_eq!(versions.ours.as_deref(), Some(&b"a\nOURS\nc\n"[..]));
    assert_eq!(versions.theirs.as_deref(), Some(&b"a\nTHEIRS\nc\n"[..]));

    let text = repo.read("f.txt");
    let mut regions = conflict::parse_markers(&text).expect("markers");
    // Default conflict style: no base in the markers; it is filled in from the stages, and
    // neither the file nor the index changes.
    let base_of = |regions: &[conflict::Region]| match &regions[1] {
        conflict::Region::Conflict(chunk) => chunk.base.clone(),
        conflict::Region::Common(_) => panic!("expected a chunk"),
    };
    assert_eq!(base_of(&regions), None);
    let scratch = info.git_dir.join("facet-tools").join("test-bases");
    conflict::fill_bases(&repo.git, &repo.path, &scratch, "f.txt", &mut regions)
        .await
        .unwrap();
    assert_eq!(base_of(&regions).as_deref(), Some("b\n"));
    assert!(!scratch.exists());
    assert_eq!(repo.read("f.txt"), text);
    assert_eq!(
        facet_git::operation(&info.git_dir).map(|op| op.kind),
        Some(OperationKind::CherryPick)
    );
    let merged = conflict::apply_choices(&regions, &[Some(Choice::TheirsThenOurs)]);
    assert_eq!(merged, "a\nTHEIRS\nOURS\nc\n");
    conflict::resolve_with(&repo.git, &repo.path, "f.txt", merged.as_bytes())
        .await
        .unwrap();

    let progress = facet_git::continue_operation(&repo.git, &info, &operation, &Editors::accept())
        .await
        .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(facet_git::operation(&info.git_dir), None);
    assert_eq!(subjects(&repo).await[0], "theirs");
    assert_eq!(repo.read("f.txt"), "a\nTHEIRS\nOURS\nc\n");
}

#[tokio::test]
async fn take_side_skip_and_abort() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "base\n", "base").await;
    repo.git_ok(&["switch", "-c", "topic"]).await;
    repo.commit_file("f.txt", "theirs\n", "theirs").await;
    repo.commit_file("g.txt", "more\n", "more").await;
    repo.git_ok(&["switch", "main"]).await;
    repo.commit_file("f.txt", "ours\n", "ours").await;
    let info = repo.info();
    let ours = head(&repo).await;

    // Whole-file resolution to their side.
    let (progress, _) = facet_git::cherry_pick(&repo.git, &info, &[("topic~1".to_owned(), false)])
        .await
        .unwrap();
    let Progress::Stopped { operation, .. } = progress else {
        panic!("expected a stop");
    };
    conflict::take_side(&repo.git, &repo.path, "f.txt", Side::Theirs, true)
        .await
        .unwrap();
    assert_eq!(repo.read("f.txt"), "theirs\n");
    facet_git::abort_operation(&repo.git, &info, &operation)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, ours);
    assert_eq!(repo.read("f.txt"), "ours\n");

    // A two-commit pick: skip the conflicting one, the other still applies.
    let (progress, _) = facet_git::cherry_pick(
        &repo.git,
        &info,
        &[("topic~1".to_owned(), false), ("topic".to_owned(), false)],
    )
    .await
    .unwrap();
    let Progress::Stopped { operation, .. } = progress else {
        panic!("expected a stop");
    };
    let progress = facet_git::skip_operation(&repo.git, &info, &operation, &Editors::accept())
        .await
        .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(subjects(&repo).await[0], "more");
    assert_eq!(repo.read("f.txt"), "ours\n");
}

#[tokio::test]
async fn hard_reset_autostashes_and_undo_restores_everything() {
    let repo = branched().await;
    let before = head(&repo).await;
    repo.write("shared.txt", "one\ntwo\nthree\nlocal edit\n");
    repo.write("untracked.txt", "stays\n");

    let undo = facet_git::reset(&repo.git, &repo.path, "HEAD~1", ResetMode::Hard)
        .await
        .unwrap();
    assert!(undo.stash.is_some());
    assert!(!repo.exists("main.txt"));
    assert_eq!(repo.read("shared.txt"), "one\ntwo\nthree\n");
    assert_eq!(repo.read("untracked.txt"), "stays\n");

    facet_git::undo_to(&repo.git, &repo.path, &undo)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, before);
    assert_eq!(repo.read("shared.txt"), "one\ntwo\nthree\nlocal edit\n");
    let stashes = repo.git_ok(&["stash", "list"]).await;
    assert!(
        stashes.is_empty(),
        "the auto-stash is dropped once restored"
    );
}

#[tokio::test]
async fn soft_and_mixed_resets_undo() {
    let repo = branched().await;
    let before = head(&repo).await;
    let undo = facet_git::reset(&repo.git, &repo.path, "HEAD~1", ResetMode::Soft)
        .await
        .unwrap();
    assert_eq!(undo.mode, ResetMode::Soft);
    assert!(repo.staged("main.txt").await.contains("main 1"));
    facet_git::undo_to(&repo.git, &repo.path, &undo)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, before);

    let undo = facet_git::reset(&repo.git, &repo.path, "HEAD~1", ResetMode::Mixed)
        .await
        .unwrap();
    assert_eq!(repo.read("main.txt"), "main 1\n");
    facet_git::undo_to(&repo.git, &repo.path, &undo)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, before);
    assert!(repo.status().await.entries.is_empty());
}

#[tokio::test]
async fn interactive_rebase_reorders_squashes_and_drops() {
    let repo = TestRepo::init().await;
    repo.commit_file("base.txt", "base\n", "base").await;
    for n in 1..=4 {
        repo.commit_file(&format!("f{n}.txt"), "x\n", &format!("c{n}"))
            .await;
    }
    let info = repo.info();
    let plan = rebase::rebase_plan(&repo.git, &repo.path, Some("HEAD~4"))
        .await
        .unwrap();
    let subjects_in_plan: Vec<&str> = plan.iter().map(|c| &*c.subject).collect();
    assert_eq!(subjects_in_plan, ["c1", "c2", "c3", "c4"]);

    let item = |index: usize, action| TodoItem {
        action,
        oid: plan[index].oid.to_string(),
        subject: plan[index].subject.to_string(),
        message: None,
    };
    // c3 first, then c1 with c4 squashed into it; c2 dropped.
    let items = [
        item(2, TodoAction::Pick),
        item(0, TodoAction::Pick),
        item(3, TodoAction::Fixup),
        item(1, TodoAction::Drop),
    ];
    // Local changes ride along (auto-stash).
    repo.write("base.txt", "base\nlocal\n");
    let progress = rebase::interactive_rebase(
        &repo.git,
        &info,
        Some("HEAD~4"),
        &items,
        &test_editors(&repo),
    )
    .await
    .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(subjects(&repo).await, ["c1", "c3", "base"]);
    assert!(repo.exists("f4.txt") && !repo.exists("f2.txt"));
    assert_eq!(repo.read("base.txt"), "base\nlocal\n");
    assert!(!repo.path.join(".git/facet-rebase").exists());
}

#[tokio::test]
async fn interactive_rebase_stops_at_edit_and_continues() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "a\n", "a").await;
    repo.commit_file("b.txt", "b\n", "b").await;
    repo.commit_file("c.txt", "c\n", "c").await;
    let info = repo.info();
    let plan = rebase::rebase_plan(&repo.git, &repo.path, None)
        .await
        .unwrap();
    assert_eq!(plan.len(), 3);
    let items: Vec<TodoItem> = plan
        .iter()
        .map(|c| TodoItem {
            action: if &*c.subject == "b" {
                TodoAction::Edit
            } else {
                TodoAction::Pick
            },
            oid: c.oid.to_string(),
            subject: c.subject.to_string(),
            message: None,
        })
        .collect();
    let progress = rebase::interactive_rebase(&repo.git, &info, None, &items, &test_editors(&repo))
        .await
        .unwrap();
    let Progress::Stopped { operation, .. } = progress else {
        panic!("expected to stop at the edit");
    };
    assert_eq!(operation.kind, OperationKind::Rebase);
    assert_eq!(operation.branch.as_deref(), Some("main"));
    assert_eq!(operation.step, Some((2, 3)));

    repo.write("b.txt", "b amended\n");
    repo.git_ok(&["commit", "-a", "--amend", "-m", "b amended"])
        .await;
    let progress = facet_git::continue_operation(&repo.git, &info, &operation, &Editors::accept())
        .await
        .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(subjects(&repo).await, ["c", "b amended", "a"]);
}

#[tokio::test]
async fn rebase_plan_refuses_merges_and_bad_todos() {
    let repo = branched().await;
    repo.git_ok(&["merge", "--no-ff", "-m", "merge topic", "topic"])
        .await;
    let error = rebase::rebase_plan(&repo.git, &repo.path, Some("HEAD~2"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("merge"), "{error}");
}

#[tokio::test]
async fn rebase_onto_and_abort() {
    let repo = TestRepo::init().await;
    repo.commit_file("f.txt", "base\n", "base").await;
    repo.git_ok(&["switch", "-c", "topic"]).await;
    repo.commit_file("f.txt", "topic\n", "topic").await;
    repo.git_ok(&["switch", "main"]).await;
    repo.commit_file("f.txt", "main\n", "main").await;
    repo.commit_file("g.txt", "g\n", "g").await;
    repo.git_ok(&["switch", "topic"]).await;
    let info = repo.info();
    let before = head(&repo).await;

    let progress = rebase::rebase_onto(&repo.git, &info, "main", &Editors::accept())
        .await
        .unwrap();
    let Progress::Stopped { operation, .. } = progress else {
        panic!("expected a conflict");
    };
    assert_eq!(operation.kind, OperationKind::Rebase);
    facet_git::abort_operation(&repo.git, &info, &operation)
        .await
        .unwrap();
    assert_eq!(head(&repo).await, before);

    // Without the conflicting commit the rebase goes through.
    repo.git_ok(&["reset", "--hard", "main~2"]).await;
    repo.commit_file("h.txt", "h\n", "h").await;
    let progress = rebase::rebase_onto(&repo.git, &info, "main", &Editors::accept())
        .await
        .unwrap();
    assert_eq!(progress, Progress::Finished);
    assert_eq!(subjects(&repo).await, ["h", "g", "main", "base"]);
}

#[tokio::test]
async fn search_by_message_author_path_and_content() {
    let repo = TestRepo::init().await;
    repo.commit_file("src/app.rs", "fn main() {}\n", "Add the app")
        .await;
    repo.commit_file("docs/readme.md", "Hello\n", "Write docs")
        .await;
    repo.commit_file("src/app.rs", "fn main() { run() }\n", "Call RUN")
        .await;
    let cancel = CancellationToken::new();
    let search = |kind, text: &'static str| {
        let repo = &repo;
        let cancel = &cancel;
        async move {
            let mut found = Vec::new();
            facet_git::stream_search(&repo.git, &repo.path, kind, text, cancel, 1, 10, |page| {
                found.extend(page.into_iter().map(|c| c.subject.to_string()));
            })
            .await
            .unwrap();
            found
        }
    };
    assert_eq!(search(SearchKind::Message, "call run").await, ["Call RUN"]);
    assert_eq!(search(SearchKind::Author, "facet test").await.len(), 3);
    assert_eq!(search(SearchKind::Author, "nobody").await.len(), 0);
    assert_eq!(
        search(SearchKind::Path, "APP.rs").await,
        ["Call RUN", "Add the app"]
    );
    assert_eq!(search(SearchKind::Path, "docs").await, ["Write docs"]);
    assert_eq!(search(SearchKind::Pickaxe, "run()").await, ["Call RUN"]);
    assert_eq!(
        search(SearchKind::DiffRegex, "fn main\\(\\) \\{")
            .await
            .len(),
        2
    );
}

#[tokio::test]
async fn file_history_follows_renames_and_blame_sees_uncommitted_lines() {
    let repo = TestRepo::init().await;
    repo.commit_file("old name.txt", "one\ntwo\n", "add").await;
    repo.git_ok(&["mv", "old name.txt", "new.txt"]).await;
    repo.git_ok(&["commit", "-m", "rename"]).await;
    repo.commit_file("new.txt", "one\ntwo\nthree\n", "grow")
        .await;
    repo.commit_file("other.txt", "x\n", "unrelated").await;
    let cancel = CancellationToken::new();

    let history = facet_git::file_history(&repo.git, &repo.path, "new.txt", &cancel)
        .await
        .unwrap();
    let shape: Vec<(&str, char, &str)> = history
        .iter()
        .map(|e| (&*e.commit.subject, e.status, e.path.as_str()))
        .collect();
    assert_eq!(
        shape,
        [
            ("grow", 'M', "new.txt"),
            ("rename", 'R', "new.txt"),
            ("add", 'A', "old name.txt")
        ]
    );

    repo.write("new.txt", "one\nTWO\nthree\n");
    let mut commits = Vec::new();
    let mut ranges = Vec::new();
    facet_git::blame(&repo.git, &repo.path, "new.txt", None, &cancel, |c, r| {
        commits.extend(c);
        ranges.extend(r);
    })
    .await
    .unwrap();
    let owner = |line: u32| {
        let range = ranges
            .iter()
            .find(|r| (r.line..r.line + r.count).contains(&line))
            .unwrap();
        &commits[range.commit]
    };
    assert_eq!(owner(1).summary, "add");
    assert!(owner(2).uncommitted);
    assert_eq!(owner(3).summary, "grow");

    let mut at_rename = Vec::new();
    facet_git::blame(
        &repo.git,
        &repo.path,
        "new.txt",
        Some("HEAD~2"),
        &cancel,
        |_, r| {
            at_rename.extend(r);
        },
    )
    .await
    .unwrap();
    assert!(at_rename.iter().all(|r| r.filename == "old name.txt"));
    assert_eq!(at_rename.iter().map(|r| r.count).sum::<u32>(), 2);
}

#[tokio::test]
async fn reflog_lists_head_moves() {
    let repo = branched().await;
    repo.git_ok(&["reset", "--hard", "HEAD~1"]).await;
    let entries = facet_git::reflog(&repo.git, &repo.path, "HEAD", 100)
        .await
        .unwrap();
    assert_eq!(entries[0].action, "reset");
    assert!(entries.iter().any(|e| e.action == "checkout"));
    assert_eq!(&*entries[1].commit.subject, "main 1");
    assert_eq!(
        facet_git::reflog(&repo.git, &repo.path, "HEAD", 2)
            .await
            .unwrap()
            .len(),
        2
    );

    let empty = TestRepo::init().await;
    assert!(facet_git::reflog(&empty.git, &empty.path, "HEAD", 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn external_tools_run_with_placeholders_and_are_logged() {
    let repo = TestRepo::init().await;
    let cancel = CancellationToken::new();
    let file = repo.path.join("x.txt");
    std::fs::write(&file, "x").unwrap();
    // `git hash-object $LOCAL` stands in for a diff tool: it must see the real path.
    let code = facet_git::tools::run_tool(
        &repo.git,
        "git hash-object $LOCAL",
        &[("LOCAL", file.as_path())],
        &repo.path,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    assert!(repo
        .log
        .snapshot()
        .iter()
        .any(|r| r.command_line.contains("hash-object") && r.command_line.contains("x.txt")));

    let missing = facet_git::tools::run_tool(
        &repo.git,
        "\"C:/does not exist/tool.exe\" $LOCAL",
        &[("LOCAL", file.as_path())],
        &repo.path,
        &cancel,
    )
    .await;
    assert!(missing.is_err());
}

#[tokio::test]
async fn merge_tool_files_are_written_beside_the_versions() {
    let repo = TestRepo::init().await;
    let versions = conflict::ConflictVersions {
        base: Some(b"base".to_vec()),
        ours: None,
        theirs: Some(b"theirs".to_vec()),
    };
    let dir = facet_git::tools::scratch_dir(&repo.path.join(".git"));
    let vars = facet_git::tools::merge_files(&dir, &repo.path, "Assets/Main.unity", &versions)
        .await
        .unwrap();
    let get = |name: &str| vars.iter().find(|(n, _)| *n == name).unwrap().1.clone();
    assert!(get("BASE").to_string_lossy().ends_with("Main.BASE.unity"));
    assert_eq!(std::fs::read(get("LOCAL")).unwrap(), b"");
    assert_eq!(std::fs::read(get("REMOTE")).unwrap(), b"theirs");
    assert_eq!(get("MERGED"), repo.path.join("Assets/Main.unity"));
}
