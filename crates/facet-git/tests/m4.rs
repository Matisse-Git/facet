//! M4 end to end: worktrees, submodules, Git LFS (attributes, locks against a local lock
//! server, track), and that sparse checkouts and partial clones keep working without
//! surprise blob fetches.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::TestRepo;
use facet_git::{
    add_worktree, checked_out_elsewhere, lfs_attributes, lfs_lock, lfs_locks, lfs_track,
    lfs_unlock, lfs_usage, lfs_version, prune_worktrees, refs, remove_worktree, same_dir, status,
    stream_log, submodule_range, submodules, sync_submodules, update_submodules, worktrees,
    CancellationToken, GitError, Network, WorktreeTarget,
};

async fn rev(repo: &TestRepo, dir: &Path, rev: &str) -> String {
    let output = repo
        .git
        .command(["rev-parse", rev])
        .current_dir(dir)
        .run()
        .await
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
async fn worktrees_add_list_refuse_and_remove() {
    let repo = TestRepo::init().await;
    repo.commit_file("a.txt", "one\n", "first").await;
    repo.git_ok(&["branch", "feature"]).await;

    let list = worktrees(&repo.git, &repo.path).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].main && same_dir(Path::new(&list[0].path), &repo.path));
    assert_eq!(list[0].branch_name(), Some("main"));

    let parent = repo.path.parent().unwrap();
    let feature = parent.join("wt feature");
    add_worktree(
        &repo.git,
        &repo.path,
        &feature,
        &WorktreeTarget::Branch("feature".into()),
    )
    .await
    .unwrap();
    let fresh = parent.join("wt-new");
    add_worktree(
        &repo.git,
        &repo.path,
        &fresh,
        &WorktreeTarget::NewBranch {
            name: "topic".into(),
            start: Some("main".into()),
        },
    )
    .await
    .unwrap();
    let detached = parent.join("wt-detached");
    add_worktree(
        &repo.git,
        &repo.path,
        &detached,
        &WorktreeTarget::Detached("HEAD".into()),
    )
    .await
    .unwrap();

    // The refs snapshot carries them; the branch is known to be checked out elsewhere.
    let snapshot = refs(&repo.git, &repo.path).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 4);
    let other = checked_out_elsewhere(&snapshot.worktrees, &repo.path, "feature").unwrap();
    assert_eq!(other.name(), "wt feature");
    assert!(checked_out_elsewhere(&snapshot.worktrees, &repo.path, "main").is_none());
    assert!(snapshot.worktrees.iter().any(|w| w.detached));
    // git itself refuses to check the branch out twice.
    let output = repo
        .git
        .command(["switch", "feature"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert!(!output.success());

    // A worktree with unsaved work is never removed.
    std::fs::write(feature.join("scratch.txt"), "unsaved").unwrap();
    let error = remove_worktree(&repo.git, &repo.path, &feature.to_string_lossy())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, GitError::Unsupported(text) if text.contains("never removes")),
        "{error:?}"
    );
    assert!(feature.join("scratch.txt").exists());
    std::fs::remove_file(feature.join("scratch.txt")).unwrap();
    remove_worktree(&repo.git, &repo.path, &feature.to_string_lossy())
        .await
        .unwrap();
    assert!(!feature.exists());

    // A worktree whose folder vanished is pruned.
    std::fs::remove_dir_all(&detached).unwrap();
    let list = worktrees(&repo.git, &repo.path).await.unwrap();
    assert!(list.iter().any(|w| w.prunable.is_some()));
    prune_worktrees(&repo.git, &repo.path).await.unwrap();
    let list = worktrees(&repo.git, &repo.path).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].branch_name(), Some("topic"));
}

/// Allow `file://` submodule URLs for this test only (git ≥ 2.38 refuses them by default),
/// through the environment: no config file is written.
fn allow_file_protocol(repo: &TestRepo) -> facet_git::Git {
    repo.git
        .with_env("GIT_CONFIG_COUNT", "1")
        .with_env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .with_env("GIT_CONFIG_VALUE_0", "always")
}

#[tokio::test]
async fn submodules_list_update_sync_and_range() {
    let library = TestRepo::init().await;
    library.commit_file("lib.txt", "v1\n", "lib v1").await;
    let v1 = rev(&library, &library.path, "HEAD").await;

    let repo = TestRepo::init().await;
    repo.commit_file("README.md", "app\n", "app").await;
    assert!(submodules(&repo.git, &repo.path).await.unwrap().is_empty());
    let git = allow_file_protocol(&repo);
    let url = format!(
        "file:///{}",
        library.path.to_string_lossy().replace('\\', "/")
    );
    let added = git
        .command(["submodule", "add", "--", &url, "libs/lib"])
        .current_dir(&repo.path)
        .output()
        .await
        .unwrap();
    assert!(added.success(), "{}", added.stderr_text());
    repo.git_ok(&["commit", "-m", "add lib"]).await;

    let list = submodules(&repo.git, &repo.path).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].path, "libs/lib");
    assert_eq!(list[0].short_name(), "lib");
    assert_eq!(list[0].recorded.as_deref(), Some(v1.as_str()));
    assert!(list[0].populated);
    assert_eq!(
        refs(&repo.git, &repo.path).await.unwrap().submodules,
        list,
        "the refs snapshot lists them too"
    );

    // Move the library on, bump the pointer, and read the range between the two.
    library.commit_file("lib.txt", "v2\n", "lib v2").await;
    library.commit_file("lib.txt", "v3\n", "lib v3").await;
    let sub = repo.path.join("libs/lib");
    let pulled = git
        .command(["pull", "--ff-only", "origin", "main"])
        .current_dir(&sub)
        .output()
        .await
        .unwrap();
    assert!(pulled.success(), "{}", pulled.stderr_text());
    let v3 = rev(&repo, &sub, "HEAD").await;
    let (added, removed) = submodule_range(&repo.git, &sub, &v1, &v3).await.unwrap();
    let subjects: Vec<&str> = added.iter().map(|c| &*c.subject).collect();
    assert_eq!(subjects, ["lib v3", "lib v2"]);
    assert!(removed.is_empty());
    let (added, removed) = submodule_range(&repo.git, &sub, &v3, &v1).await.unwrap();
    assert!(
        added.is_empty() && removed.len() == 2,
        "a rewind drops commits"
    );

    // A fresh clone has the submodule declared but not checked out; update brings it.
    let clone = TestRepo::empty_dir();
    let cloned = allow_file_protocol(&clone)
        .command(["clone", "--", &repo.path.to_string_lossy(), "."])
        .current_dir(&clone.path)
        .output()
        .await
        .unwrap();
    assert!(cloned.success(), "{}", cloned.stderr_text());
    let list = submodules(&clone.git, &clone.path).await.unwrap();
    assert!(!list[0].populated);
    let git = allow_file_protocol(&clone);
    let cancel = CancellationToken::new();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = lines.clone();
    let net = Network {
        git: &git,
        root: &clone.path,
        cancel: &cancel,
        progress: Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_owned())),
    };
    update_submodules(&net, Some("libs/lib")).await.unwrap();
    let list = submodules(&clone.git, &clone.path).await.unwrap();
    assert!(list[0].populated);
    assert_eq!(rev(&clone, &clone.path.join("libs/lib"), "HEAD").await, v1);
    sync_submodules(&clone.git, &clone.path, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn lfs_attributes_come_from_gitattributes() {
    let repo = TestRepo::init().await;
    repo.write(
        ".gitattributes",
        "*.psd filter=lfs diff=lfs merge=lfs -text lockable\n*.umap filter=lfs diff=lfs merge=lfs -text\n",
    );
    repo.commit_file("src/main.rs", "fn main() {}\n", "code")
        .await;
    let info = repo.info();
    let usage = lfs_usage(&repo.path, &info.git_dir);
    assert!(usage.lfs && usage.locking);
    let paths = vec![
        "Art/hero.psd".to_owned(),
        "Maps/a b.umap".to_owned(),
        "src/main.rs".to_owned(),
    ];
    let attrs = lfs_attributes(&repo.git, &repo.path, &paths).await.unwrap();
    assert_eq!(attrs.len(), 3);
    assert!(attrs[0].1.lfs && attrs[0].1.lockable);
    assert!(attrs[1].1.lfs && !attrs[1].1.lockable);
    assert!(!attrs[2].1.lfs);
    assert!(lfs_attributes(&repo.git, &repo.path, &[])
        .await
        .unwrap()
        .is_empty());

    let plain = TestRepo::init().await;
    assert!(!lfs_usage(&plain.path, &plain.info().git_dir).lfs);
}

/// The LFS locking API, just enough of it for `git lfs lock / locks / unlock`.
struct LockServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

fn lock_json(id: &str, path: &str, owner: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "path": path, "owner": {"name": owner}, "locked_at": "2026-11-18T10:00:00Z"})
}

fn start_lock_server() -> LockServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/repo.git/info/lfs",
        listener.local_addr().unwrap()
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    // Someone else's lock exists from the start.
    let locks = Arc::new(Mutex::new(vec![(
        "100".to_owned(),
        "Maps/level.umap".to_owned(),
        "Ada".to_owned(),
    )]));
    std::thread::spawn(move || {
        let mut next = 1;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(at + 4);
                }
            };
            let Some(header_end) = header_end else {
                continue;
            };
            let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())?
                })
                .unwrap_or(0);
            while data.len() < header_end + length {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&data[header_end..]).unwrap_or(serde_json::Value::Null);
            let line = head.lines().next().unwrap_or_default().to_owned();
            log.lock().unwrap().push(line.clone());
            let mut parts = line.split(' ');
            let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let path = target.split('?').next().unwrap_or("");
            let mut held = locks.lock().unwrap();
            let (code, reply) = if method == "POST" && path.ends_with("/locks/verify") {
                let (ours, theirs): (Vec<_>, Vec<_>) = held
                    .iter()
                    .partition(|(_, _, owner)| owner == common::AUTHOR);
                let list = |set: Vec<&(String, String, String)>| {
                    set.iter()
                        .map(|(id, path, owner)| lock_json(id, path, owner))
                        .collect::<Vec<_>>()
                };
                (
                    200,
                    serde_json::json!({"ours": list(ours), "theirs": list(theirs), "next_cursor": ""}),
                )
            } else if method == "POST" && path.ends_with("/unlock") {
                let id = path.rsplit('/').nth(1).unwrap_or("").to_owned();
                let force = body["force"].as_bool().unwrap_or(false);
                match held.iter().position(|(lock, _, _)| *lock == id) {
                    Some(at) if force || held[at].2 == common::AUTHOR => {
                        let (id, path, owner) = held.remove(at);
                        (
                            200,
                            serde_json::json!({"lock": lock_json(&id, &path, &owner)}),
                        )
                    }
                    Some(_) => (
                        403,
                        serde_json::json!({"message": "lock owned by someone else"}),
                    ),
                    None => (404, serde_json::json!({"message": "no such lock"})),
                }
            } else if method == "POST" && path.ends_with("/locks") {
                let file = body["path"].as_str().unwrap_or_default().to_owned();
                if let Some((id, path, owner)) = held.iter().find(|(_, p, _)| *p == file) {
                    (
                        409,
                        serde_json::json!({"lock": lock_json(id, path, owner), "message": "already locked"}),
                    )
                } else {
                    let id = next.to_string();
                    next += 1;
                    held.push((id.clone(), file.clone(), common::AUTHOR.to_owned()));
                    (
                        201,
                        serde_json::json!({"lock": lock_json(&id, &file, common::AUTHOR)}),
                    )
                }
            } else if method == "GET" && path.ends_with("/locks") {
                let list: Vec<_> = held
                    .iter()
                    .map(|(id, path, owner)| lock_json(id, path, owner))
                    .collect();
                (200, serde_json::json!({"locks": list, "next_cursor": ""}))
            } else {
                (404, serde_json::json!({"message": "not found"}))
            };
            drop(held);
            let body = reply.to_string();
            let response = format!(
                "HTTP/1.1 {code} X\r\nContent-Type: application/vnd.git-lfs+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    LockServer { url, requests }
}

#[tokio::test]
async fn lfs_locks_lock_and_unlock_against_a_server() {
    let repo = TestRepo::init().await;
    if lfs_version(&repo.git).await.is_none() {
        eprintln!("git-lfs is not installed; skipping");
        return;
    }
    let server = start_lock_server();
    repo.git_ok(&["config", "--local", "lfs.url", &server.url])
        .await;
    repo.git_ok(&["config", "--local", "lfs.locksverify", "false"])
        .await;
    repo.write(
        ".gitattributes",
        "*.psd filter=lfs diff=lfs merge=lfs -text lockable\n",
    );
    repo.commit_file("Art/hero.psd", "psd\n", "art").await;

    let locks = lfs_locks(&repo.git, &repo.path).await.unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].path, "Maps/level.umap");
    assert_eq!(locks[0].owner, "Ada");
    assert_eq!(locks[0].ours, Some(false), "verified: someone else's");

    let mine = lfs_lock(&repo.git, &repo.path, "Art/hero.psd")
        .await
        .unwrap();
    assert_eq!(mine.path, "Art/hero.psd");
    assert_eq!(mine.ours, Some(true));
    let locks = lfs_locks(&repo.git, &repo.path).await.unwrap();
    assert_eq!(locks.len(), 2);
    assert_eq!(locks[0].path, "Art/hero.psd");
    assert_eq!(locks[0].ours, Some(true));

    // Locking a file someone holds fails with the server's message.
    assert!(lfs_lock(&repo.git, &repo.path, "Maps/level.umap")
        .await
        .is_err());
    // Unlocking theirs without force is refused; with force it goes.
    assert!(lfs_unlock(&repo.git, &repo.path, "100", false)
        .await
        .is_err());
    lfs_unlock(&repo.git, &repo.path, "100", true)
        .await
        .unwrap();
    lfs_unlock(&repo.git, &repo.path, &mine.id, false)
        .await
        .unwrap();
    assert!(lfs_locks(&repo.git, &repo.path).await.unwrap().is_empty());
    let requests = server.requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .any(|r| r.starts_with("POST /repo.git/info/lfs/locks/verify")),
        "{requests:?}"
    );

    // Track: .gitattributes gains the pattern (git-lfs writes it).
    lfs_track(&repo.git, &repo.path, "*.uasset", true)
        .await
        .unwrap();
    let attributes = repo.read(".gitattributes");
    assert!(attributes.contains("*.uasset filter=lfs"), "{attributes}");
    assert!(attributes
        .lines()
        .any(|l| l.starts_with("*.uasset") && l.contains("lockable")));
}

async fn missing_objects(repo: &TestRepo, dir: &Path) -> usize {
    let output = repo
        .git
        .command(["rev-list", "--objects", "--all", "--missing=print"])
        .current_dir(dir)
        .run()
        .await
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with('?'))
        .count()
}

#[tokio::test]
async fn partial_clones_are_not_fetched_from_by_status_log_or_refs() {
    let source = TestRepo::init().await;
    for n in 0..4 {
        source
            .commit_file("data.txt", &format!("version {n}\n"), &format!("v{n}"))
            .await;
        source
            .commit_file(&format!("assets/{n}.bin"), &"x".repeat(1000 + n), "asset")
            .await;
    }
    source
        .git_ok(&["config", "--local", "uploadpack.allowFilter", "true"])
        .await;
    let clone = TestRepo::empty_dir();
    let url = format!(
        "file:///{}",
        source.path.to_string_lossy().replace('\\', "/")
    );
    clone
        .git_ok(&["clone", "--filter=blob:none", "--", &url, "."])
        .await;
    let before = missing_objects(&clone, &clone.path).await;
    assert!(before > 0, "old blobs stay on the server");

    clone.write("data.txt", "edited\n");
    let status = status(&clone.git, &clone.path, true, None).await.unwrap();
    assert_eq!(status.entries.len(), 1);
    let cancel = CancellationToken::new();
    let mut count = 0;
    stream_log(&clone.git, &clone.path, &cancel, 2, 2, |page| {
        count += page.len()
    })
    .await
    .unwrap();
    assert_eq!(count, 8);
    let snapshot = refs(&clone.git, &clone.path).await.unwrap();
    assert!(!snapshot.refs.is_empty());
    assert_eq!(
        missing_objects(&clone, &clone.path).await,
        before,
        "no blob was fetched lazily"
    );
}

#[tokio::test]
async fn sparse_checkouts_show_only_the_cone() {
    let repo = TestRepo::init().await;
    repo.write("game/a.txt", "a\n");
    repo.write("engine/b.txt", "b\n");
    repo.commit_file("top.txt", "top\n", "all").await;
    repo.git_ok(&["sparse-checkout", "set", "--cone", "game"])
        .await;
    assert!(!repo.exists("engine/b.txt"));
    let clean = status(&repo.git, &repo.path, true, None).await.unwrap();
    assert!(clean.entries.is_empty(), "{:?}", clean.entries);

    repo.write("game/a.txt", "changed\n");
    facet_git::stage_all(&repo.git, &repo.path).await.unwrap();
    let staged = status(&repo.git, &repo.path, true, None).await.unwrap();
    assert_eq!(staged.entries.len(), 1);
    assert_eq!(staged.entries[0].path, "game/a.txt");
    facet_git::commit(&repo.git, &repo.path, "edit", false)
        .await
        .unwrap();
    // Files outside the cone are still in the commit.
    let tree =
        String::from_utf8(repo.git_ok(&["ls-tree", "-r", "--name-only", "HEAD"]).await).unwrap();
    assert!(tree.contains("engine/b.txt"));
}
