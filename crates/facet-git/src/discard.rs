//! Discarding working tree changes, undoably.
//!
//! Nothing is ever deleted by a discard. What is about to be discarded is first *moved* into
//! `<git dir>/facet/undo/<id>/` (a rename on the same volume, so it is instant even for
//! multi-gigabyte assets), and only then does git restore the index version. Undo swaps the
//! saved files back in, saving what it replaces as a "redo" entry, so undo is itself undoable
//! and no step can lose data.
//!
//! Layout of one entry:
//!
//! ```text
//! <git dir>/facet/undo/<millis>-<n>/
//!   label          what was discarded, for display
//!   redo           (marker) this entry re-does a discard that was undone
//!   tree/<path>    files moved out of the working tree
//!   absent         NUL-separated paths that did not exist (undo removes them again)
//!   patch          for hunk/line discards: the patch that was reverse-applied
//! ```

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::stage::{apply_patch, nul_list, ApplyTo};
use crate::{Git, GitError, RepoInfo};

const UNDO_DIR: &str = "facet/undo";
const TREE: &str = "tree";
const ABSENT: &str = "absent";
const PATCH: &str = "patch";
const LABEL: &str = "label";
const REDO: &str = "redo";

/// A saved state that [`undo`] can bring back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    /// Sortable: later entries compare greater.
    pub id: String,
    pub dir: PathBuf,
    pub label: String,
    /// This entry was produced by an undo; applying it discards again.
    pub redo: bool,
}

/// One file to discard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardFile {
    pub path: String,
    /// Tracked files are restored from the index; untracked files are simply removed.
    pub tracked: bool,
}

/// Discard the working tree changes of whole files. Returns the entry that undoes it.
pub async fn discard_files(
    git: &Git,
    repo: &RepoInfo,
    files: &[DiscardFile],
    label: &str,
) -> Result<UndoEntry, GitError> {
    let workdir = workdir(repo)?;
    let (id, dir) = new_entry(repo)?;
    let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();

    let captured = {
        let (workdir, dir, label) = (workdir.clone(), dir.clone(), label.to_owned());
        blocking(move || {
            write_meta(&dir, &label, false)?;
            capture(&workdir, &dir, &paths)
        })
        .await
    };
    if let Err(error) = captured {
        rollback(&workdir, &dir).await;
        return Err(error.into());
    }

    let tracked: Vec<&str> = files
        .iter()
        .filter(|file| file.tracked)
        .map(|file| file.path.as_str())
        .collect();
    if !tracked.is_empty() {
        let restored = git
            .command(["--literal-pathspecs", "checkout"])
            .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
            .current_dir(&workdir)
            .stdin(nul_list(tracked))
            .run()
            .await;
        if let Err(error) = restored {
            rollback(&workdir, &dir).await;
            return Err(error);
        }
    }

    Ok(UndoEntry {
        id,
        dir,
        label: label.to_owned(),
        redo: false,
    })
}

/// Discard part of a file: reverse-apply `patch` (from [`crate::build_patch`] with
/// [`crate::PatchDirection::Reverse`]) to the working tree. The patch itself is the backup.
pub async fn discard_patch(
    git: &Git,
    repo: &RepoInfo,
    patch: &[u8],
    label: &str,
) -> Result<UndoEntry, GitError> {
    let workdir = workdir(repo)?;
    let (id, dir) = new_entry(repo)?;
    {
        let (dir, label, patch) = (dir.clone(), label.to_owned(), patch.to_vec());
        blocking(move || {
            write_meta(&dir, &label, false)?;
            std::fs::write(dir.join(PATCH), patch)
        })
        .await?;
    }
    if let Err(error) = apply_patch(git, &workdir, patch, ApplyTo::WorktreeReverse).await {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(error);
    }
    Ok(UndoEntry {
        id,
        dir,
        label: label.to_owned(),
        redo: false,
    })
}

/// Bring back what `entry` saved. Returns the entry that reverses this again (redo after an
/// undo, undo after a redo). `entry` is consumed: its directory is gone afterwards.
pub async fn undo(git: &Git, repo: &RepoInfo, entry: &UndoEntry) -> Result<UndoEntry, GitError> {
    let workdir = workdir(repo)?;
    let patch_file = entry.dir.join(PATCH);
    let (id, dir) = new_entry(repo)?;
    let flipped = UndoEntry {
        id,
        dir: dir.clone(),
        label: entry.label.clone(),
        redo: !entry.redo,
    };

    if tokio::fs::try_exists(&patch_file).await.unwrap_or(false) {
        let patch = tokio::fs::read(&patch_file).await?;
        // The entry of a discard holds a patch to re-apply; a redo entry un-applies it again.
        let target = if entry.redo {
            ApplyTo::WorktreeReverse
        } else {
            ApplyTo::Worktree
        };
        if let Err(error) = apply_patch(git, &workdir, &patch, target).await {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(error);
        }
        let (label, redo, old) = (flipped.label.clone(), flipped.redo, entry.dir.clone());
        blocking(move || {
            write_meta(&dir, &label, redo)?;
            std::fs::write(dir.join(PATCH), patch)?;
            std::fs::remove_dir_all(old)
        })
        .await?;
        return Ok(flipped);
    }

    let (label, redo, old) = (flipped.label.clone(), flipped.redo, entry.dir.clone());
    let swap_dir = dir.clone();
    let swap_workdir = workdir.clone();
    let swapped = blocking(move || {
        write_meta(&swap_dir, &label, redo)?;
        let saved = saved_paths(&old)?;
        let absent = read_absent(&old)?;
        // Move the current state of every involved path out of the way (into the new
        // entry), then move the saved files in. Paths that were absent stay absent.
        let mut involved = saved.clone();
        involved.extend(absent);
        capture(&swap_workdir, &swap_dir, &involved)?;
        restore(&swap_workdir, &old, &saved)?;
        std::fs::remove_dir_all(&old)
    })
    .await;
    if let Err(error) = swapped {
        // Whatever was captured goes straight back; the original entry keeps what is left.
        rollback(&workdir, &dir).await;
        return Err(error.into());
    }
    Ok(flipped)
}

/// All undo entries of this repository, oldest first.
pub async fn list_undo(repo: &RepoInfo) -> Vec<UndoEntry> {
    let root = repo.git_dir.join(UNDO_DIR);
    blocking(move || {
        let mut entries = Vec::new();
        for item in std::fs::read_dir(&root)? {
            let item = item?;
            if !item.file_type()?.is_dir() {
                continue;
            }
            let dir = item.path();
            let Ok(label) = std::fs::read_to_string(dir.join(LABEL)) else {
                continue; // not (or not completely) written by us
            };
            entries.push(UndoEntry {
                id: item.file_name().to_string_lossy().into_owned(),
                redo: dir.join(REDO).exists(),
                dir,
                label,
            });
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entries)
    })
    .await
    .unwrap_or_default()
}

/// Delete undo entries older than `max_age`, and all but the newest `keep`. Returns how many
/// were removed. This is the only place saved files are ever really deleted.
pub async fn prune_undo(repo: &RepoInfo, keep: usize, max_age: Duration) -> usize {
    let entries = list_undo(repo).await;
    let now = now_millis();
    let count = entries.len();
    let mut removed = 0;
    for (index, entry) in entries.into_iter().enumerate() {
        let created = entry
            .id
            .split('-')
            .next()
            .and_then(|millis| millis.parse::<u128>().ok())
            .unwrap_or(0);
        let too_old = now.saturating_sub(created) > max_age.as_millis();
        let too_many = count - index > keep;
        if (too_old || too_many) && tokio::fs::remove_dir_all(&entry.dir).await.is_ok() {
            removed += 1;
        }
    }
    removed
}

// ----- helpers ------------------------------------------------------------------------------

fn workdir(repo: &RepoInfo) -> Result<PathBuf, GitError> {
    repo.workdir
        .clone()
        .ok_or_else(|| GitError::Unsupported("a bare repository has no working tree".to_owned()))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

fn new_entry(repo: &RepoInfo) -> Result<(String, PathBuf), GitError> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = format!(
        "{:013}-{:04}",
        now_millis(),
        COUNTER.fetch_add(1, Ordering::Relaxed) % 10_000
    );
    let dir = repo.git_dir.join(UNDO_DIR).join(&id);
    std::fs::create_dir_all(&dir)?;
    Ok((id, dir))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|join| Err(io::Error::other(join)))
}

fn write_meta(dir: &Path, label: &str, redo: bool) -> io::Result<()> {
    std::fs::write(dir.join(LABEL), label)?;
    if redo {
        std::fs::write(dir.join(REDO), b"")?;
    }
    Ok(())
}

fn native(relative: &str) -> PathBuf {
    relative.split('/').collect()
}

/// Move each of `paths` from the working tree into `dir/tree`, noting the ones that do not
/// exist in `dir/absent`. Directories left empty are removed, as git would.
fn capture(workdir: &Path, dir: &Path, paths: &[String]) -> io::Result<()> {
    let mut absent = Vec::new();
    for path in paths {
        let relative = native(path);
        let source = workdir.join(&relative);
        match std::fs::symlink_metadata(&source) {
            Ok(meta) if meta.is_dir() => {
                return Err(io::Error::other(format!(
                    "{path} is a directory (a submodule or nested repository); \
                     Facet does not discard those"
                )));
            }
            Ok(_) => {
                move_file(&source, &dir.join(TREE).join(&relative))?;
                remove_empty_parents(workdir, &source);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                absent.extend_from_slice(path.as_bytes());
                absent.push(0);
            }
            Err(error) => return Err(error),
        }
    }
    if !absent.is_empty() {
        std::fs::write(dir.join(ABSENT), absent)?;
    }
    Ok(())
}

/// Move the saved files of `dir` back into the working tree.
fn restore(workdir: &Path, dir: &Path, saved: &[String]) -> io::Result<()> {
    for path in saved {
        let relative = native(path);
        move_file(&dir.join(TREE).join(&relative), &workdir.join(&relative))?;
    }
    Ok(())
}

/// Put everything an unfinished entry captured back and delete the entry.
async fn rollback(workdir: &Path, dir: &Path) {
    let (workdir, dir) = (workdir.to_owned(), dir.to_owned());
    let _ = blocking(move || {
        let saved = saved_paths(&dir)?;
        for path in &saved {
            let relative = native(path);
            let target = workdir.join(&relative);
            // Git may have restored some files before failing; the saved copy wins.
            let _ = std::fs::remove_file(&target);
            move_file(&dir.join(TREE).join(&relative), &target)?;
        }
        std::fs::remove_dir_all(&dir)
    })
    .await;
}

/// Relative paths (forward slashes) of all files under `dir/tree`.
fn saved_paths(dir: &Path) -> io::Result<Vec<String>> {
    fn walk(base: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
        for item in std::fs::read_dir(base)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            // `file_type` does not follow symlinks: a saved link is moved as a link.
            if item.file_type()?.is_dir() {
                walk(&item.path(), &relative, out)?;
            } else {
                out.push(relative);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let tree = dir.join(TREE);
    if tree.is_dir() {
        walk(&tree, "", &mut out)?;
    }
    out.sort();
    Ok(out)
}

fn read_absent(dir: &Path) -> io::Result<Vec<String>> {
    match std::fs::read(dir.join(ABSENT)) {
        Ok(bytes) => Ok(bytes
            .split(|b| *b == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Rename, falling back to copy + delete when source and target are on different volumes.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

fn remove_empty_parents(workdir: &Path, file: &Path) {
    let mut current = file.parent();
    while let Some(dir) = current {
        // `remove_dir` only succeeds on an empty directory.
        if dir == workdir || std::fs::remove_dir(dir).is_err() {
            break;
        }
        current = dir.parent();
    }
}
