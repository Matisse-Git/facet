//! File system watching for one repository: `notify` events are classified, debounced
//! (~150 ms of quiet) and handed to the app as one [`FsChange`], which triggers a targeted
//! status refresh. Nothing here runs git or touches the UI thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use facet_git::RepoInfo;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// Events stop arriving for this long before a batch is delivered.
const QUIET: Duration = Duration::from_millis(150);
/// A batch is delivered at the latest this long after its first event, however busy the
/// disk is (a build writing files continuously must not starve the refresh).
const MAX_WAIT: Duration = Duration::from_millis(1000);
/// Beyond this many distinct paths the batch just says "many things changed".
const MAX_PATHS: usize = 256;

/// `(modified, length)` of `.git/index`; `None` when there is no index file.
pub type IndexStamp = Option<(SystemTime, u64)>;

pub fn index_stamp(git_dir: &Path) -> IndexStamp {
    let meta = std::fs::metadata(git_dir.join("index")).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// One debounced batch of changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FsChange {
    /// Something in the working tree changed.
    pub worktree: bool,
    /// `.git/index` changed (staging from a terminal, another tool…).
    pub index: bool,
    /// `HEAD`, a ref, or an in-progress operation marker changed.
    pub refs: bool,
    /// Changed working tree paths, relative with forward slashes. Empty if `overflow`.
    pub paths: Vec<String>,
    /// Too many paths, or the OS dropped events: assume anything may have changed.
    pub overflow: bool,
    /// The index stamp when the batch was delivered (set only if `index`).
    pub index_stamp: IndexStamp,
}

impl FsChange {
    fn merge(&mut self, kind: Kind) {
        match kind {
            Kind::Worktree(path) => {
                self.worktree = true;
                if !self.overflow && !self.paths.contains(&path) {
                    if self.paths.len() >= MAX_PATHS {
                        self.overflow = true;
                        self.paths.clear();
                    } else {
                        self.paths.push(path);
                    }
                }
            }
            Kind::Index => self.index = true,
            Kind::Refs => self.refs = true,
            Kind::Rescan => {
                self.worktree = true;
                self.index = true;
                self.refs = true;
                self.overflow = true;
                self.paths.clear();
            }
        }
    }

    /// Does the batch touch this working tree path?
    pub fn touches(&self, path: &str) -> bool {
        self.overflow || self.paths.iter().any(|changed| changed == path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Worktree(String),
    Index,
    Refs,
    Rescan,
}

/// Decide what a changed path means for the repository; `None` for noise (object writes,
/// lock files, Facet's own undo store, logs).
fn classify(workdir: &Path, git_dir: &Path, path: &Path) -> Option<Kind> {
    if let Ok(inside) = path.strip_prefix(git_dir) {
        let relative = inside.to_string_lossy().replace('\\', "/");
        if relative.ends_with(".lock") {
            return None;
        }
        return match relative.as_str() {
            "index" => Some(Kind::Index),
            "HEAD" | "ORIG_HEAD" | "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD"
            | "BISECT_LOG" | "packed-refs" | "config" | "info/exclude" => Some(Kind::Refs),
            other
                if other.starts_with("refs/")
                    || other.starts_with("rebase-merge")
                    || other.starts_with("rebase-apply")
                    || other.starts_with("sequencer") =>
            {
                Some(Kind::Refs)
            }
            _ => None,
        };
    }
    let relative = path.strip_prefix(workdir).ok()?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    // The working tree root itself, or a `.git` file/dir reached through another spelling.
    if relative.is_empty() || relative == ".git" || relative.starts_with(".git/") {
        return None;
    }
    Some(Kind::Worktree(relative))
}

/// Keeps the OS watch alive; dropping it stops the watcher and its debounce task.
pub struct RepoWatcher {
    _watcher: RecommendedWatcher,
}

/// Watch a repository's working tree and git directory. `on_change` is called on a tokio
/// worker with each debounced batch.
pub fn watch(
    info: &RepoInfo,
    runtime: &tokio::runtime::Handle,
    on_change: impl Fn(FsChange) + Send + 'static,
) -> notify::Result<RepoWatcher> {
    let Some(workdir) = info.workdir.clone() else {
        return Err(notify::Error::generic("bare repositories are not watched"));
    };
    let git_dir = info.git_dir.clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<Kind>();

    let callback = {
        let (workdir, git_dir) = (workdir.clone(), git_dir.clone());
        move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                if event.need_rescan() {
                    let _ = tx.send(Kind::Rescan);
                }
                for path in &event.paths {
                    if let Some(kind) = classify(&workdir, &git_dir, path) {
                        let _ = tx.send(kind);
                    }
                }
            }
            // The OS buffer overflowed or the watch broke: refresh everything once.
            Err(_) => {
                let _ = tx.send(Kind::Rescan);
            }
        }
    };
    let mut watcher = notify::recommended_watcher(callback)?;
    watcher.watch(&workdir, RecursiveMode::Recursive)?;
    // Linked worktrees and submodules keep their git directory outside the working tree.
    if !git_dir.starts_with(&workdir) {
        watcher.watch(&git_dir, RecursiveMode::Recursive)?;
    }

    let stamp_dir: PathBuf = git_dir;
    runtime.spawn(async move {
        // Ends when the watcher (and with it the sender) is dropped.
        while let Some(first) = rx.recv().await {
            let mut batch = FsChange::default();
            batch.merge(first);
            let deadline = tokio::time::Instant::now() + MAX_WAIT;
            loop {
                tokio::select! {
                    more = rx.recv() => match more {
                        Some(kind) => batch.merge(kind),
                        None => break,
                    },
                    () = tokio::time::sleep(QUIET) => break,
                    () = tokio::time::sleep_until(deadline) => break,
                }
            }
            if batch.index {
                batch.index_stamp = index_stamp(&stamp_dir);
            }
            on_change(batch);
        }
    });

    Ok(RepoWatcher { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(path: &str) -> Option<Kind> {
        classify(
            Path::new(r"D:\repo"),
            Path::new(r"D:\repo\.git"),
            Path::new(path),
        )
    }

    #[test]
    fn working_tree_paths_are_relative_with_forward_slashes() {
        assert_eq!(
            kind(r"D:\repo\Content\Maps\Level.umap"),
            Some(Kind::Worktree("Content/Maps/Level.umap".into()))
        );
        assert_eq!(kind(r"D:\repo"), None);
        assert_eq!(kind(r"D:\elsewhere\file.txt"), None);
    }

    #[test]
    fn git_directory_noise_is_ignored() {
        assert_eq!(kind(r"D:\repo\.git\index"), Some(Kind::Index));
        assert_eq!(kind(r"D:\repo\.git\HEAD"), Some(Kind::Refs));
        assert_eq!(kind(r"D:\repo\.git\refs\heads\main"), Some(Kind::Refs));
        assert_eq!(kind(r"D:\repo\.git\packed-refs"), Some(Kind::Refs));
        assert_eq!(kind(r"D:\repo\.git\MERGE_HEAD"), Some(Kind::Refs));
        assert_eq!(kind(r"D:\repo\.git\sequencer\todo"), Some(Kind::Refs));
        assert_eq!(kind(r"D:\repo\.git\facet-rebase\todo"), None);
        for noise in [
            r"D:\repo\.git\index.lock",
            r"D:\repo\.git\HEAD.lock",
            r"D:\repo\.git\objects\ab\cdef",
            r"D:\repo\.git\logs\HEAD",
            r"D:\repo\.git\facet\undo\1\tree\a.txt",
            r"D:\repo\.git\COMMIT_EDITMSG",
            r"D:\repo\.git",
        ] {
            assert_eq!(kind(noise), None, "{noise}");
        }
    }

    #[test]
    fn batches_merge_and_overflow() {
        let mut batch = FsChange::default();
        batch.merge(Kind::Worktree("a.txt".into()));
        batch.merge(Kind::Worktree("a.txt".into()));
        batch.merge(Kind::Index);
        assert!(batch.worktree && batch.index && !batch.refs);
        assert_eq!(batch.paths, ["a.txt"]);
        assert!(batch.touches("a.txt") && !batch.touches("b.txt"));

        for n in 0..MAX_PATHS + 10 {
            batch.merge(Kind::Worktree(format!("f{n}")));
        }
        assert!(batch.overflow && batch.paths.is_empty());
        assert!(batch.touches("anything"));

        let mut rescan = FsChange::default();
        rescan.merge(Kind::Rescan);
        assert!(rescan.worktree && rescan.index && rescan.refs && rescan.overflow);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_burst_of_writes_arrives_as_one_batch() {
        let dir = tempfile::tempdir().unwrap();
        let workdir = dir.path().canonicalize().unwrap();
        // Strip the `\\?\` prefix canonicalize adds on Windows.
        let workdir = PathBuf::from(workdir.to_string_lossy().trim_start_matches(r"\\?\"));
        std::fs::create_dir_all(workdir.join(".git/objects")).unwrap();
        let info = RepoInfo {
            git_dir: workdir.join(".git"),
            workdir: Some(workdir.clone()),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _watcher = watch(&info, &tokio::runtime::Handle::current(), move |change| {
            let _ = tx.send(change);
        })
        .unwrap();

        for n in 0..20 {
            std::fs::write(workdir.join(format!("file{n}.txt")), "x").unwrap();
        }
        std::fs::write(workdir.join(".git/objects/noise"), "x").unwrap();
        std::fs::write(workdir.join(".git/index"), "x").unwrap();

        let batch = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a batch within 10 s")
            .unwrap();
        assert!(batch.worktree && batch.index);
        assert!(batch.touches("file0.txt") && batch.touches("file19.txt"));
        assert!(!batch.paths.iter().any(|path| path.contains(".git")));
        assert!(batch.index_stamp.is_some());
        // Nothing else follows: the burst was coalesced.
        assert!(tokio::time::timeout(Duration::from_millis(600), rx.recv())
            .await
            .is_err());
    }
}
