//! What would be lost if a repository's folder went away: the check shown before Facet moves a
//! repository to the Recycle Bin. It never changes anything.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::parse::{self, EntryKind};
use crate::worktree::same_dir;
use crate::{Git, GitError};

/// Unpushed commits are counted up to this many; more is reported as "at least".
const UNPUSHED_LIMIT: usize = 10_000;

/// Everything about a repository that exists only in its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovalCheck {
    /// A bare repository (no working tree, so no uncommitted files).
    pub bare: bool,
    /// Tracked files with staged or unstaged changes, conflicts included.
    pub changed_files: usize,
    /// Files git does not track and does not ignore.
    pub untracked_files: usize,
    /// Local branches with commits no remote-tracking branch contains: name and count, the
    /// most commits first.
    pub unpushed: Vec<(String, usize)>,
    /// More unpushed commits exist than were counted (`UNPUSHED_LIMIT`).
    pub unpushed_capped: bool,
    pub stashes: usize,
    /// The repository has at least one remote configured.
    pub has_remote: bool,
    /// This is a linked worktree; the folder of the repository it belongs to (where `git
    /// worktree prune` runs once this folder is gone).
    pub main_worktree: Option<PathBuf>,
    /// Linked worktrees of this repository whose folders still exist: deleting this folder
    /// would break them.
    pub linked_worktrees: Vec<PathBuf>,
}

impl RemovalCheck {
    /// Nothing here exists only in this folder.
    pub fn is_safe(&self) -> bool {
        self.changed_files == 0
            && self.untracked_files == 0
            && self.unpushed.is_empty()
            && self.stashes == 0
            && self.has_remote
    }
}

/// Check the repository whose folder is `path`. Fails with `NotARepository` when `path` is not
/// the root of a repository (a plain folder, or a folder *inside* another repository: its
/// check would describe the wrong repository).
pub async fn removal_check(git: &Git, path: &Path) -> Result<RemovalCheck, GitError> {
    let info = crate::discover(git, path).await?;
    if !same_dir(info.root(), path) {
        return Err(GitError::NotARepository {
            path: path.to_owned(),
            detail: format!("it is inside the repository at {}", info.root().display()),
        });
    }
    let root = info.root().to_owned();
    let bare = info.is_bare();

    let snapshot = crate::refs(git, &root);
    let status = async {
        if bare {
            Ok(None)
        } else {
            crate::status(git, &root, true, None).await.map(Some)
        }
    };
    // Every commit reachable from a local branch but from no remote-tracking branch, with its
    // parents, so the count per branch can be worked out here instead of one process each
    // (G5).
    let limit = format!("--max-count={UNPUSHED_LIMIT}");
    let unpushed = git
        .command([
            "rev-list",
            "--parents",
            limit.as_str(),
            "--branches",
            "--not",
            "--remotes",
        ])
        .current_dir(&root)
        .background()
        .output();
    let (snapshot, status, unpushed) = tokio::join!(snapshot, status, unpushed);
    let snapshot = snapshot?;
    let unpushed = unpushed?;

    let mut check = RemovalCheck {
        bare,
        stashes: snapshot.stashes.len(),
        has_remote: !snapshot.remotes.is_empty(),
        ..RemovalCheck::default()
    };
    if let Some(status) = status? {
        for entry in &status.entries {
            match entry.kind {
                EntryKind::Untracked => check.untracked_files += 1,
                EntryKind::Ignored => {}
                EntryKind::Tracked | EntryKind::Unmerged => check.changed_files += 1,
            }
        }
    }

    // Without a remote every commit is "unpushed"; the missing remote says it better.
    if check.has_remote && unpushed.success() {
        let text = parse::utf8("rev-list", &unpushed.stdout)?;
        let tips: Vec<(String, String)> = snapshot
            .local()
            .map(|branch| (branch.name.clone(), branch.oid.clone()))
            .collect();
        let (counts, capped) = unpushed_per_branch(text, &tips, UNPUSHED_LIMIT);
        check.unpushed = counts;
        check.unpushed_capped = capped;
    }

    let main = snapshot.worktrees.iter().find(|worktree| worktree.main);
    let is_linked = snapshot
        .worktrees
        .iter()
        .any(|worktree| !worktree.main && same_dir(Path::new(&worktree.path), &root));
    if is_linked {
        check.main_worktree = main.map(|main| native(&main.path));
    } else {
        check.linked_worktrees = snapshot
            .worktrees
            .iter()
            .filter(|worktree| !worktree.main && worktree.prunable.is_none())
            .map(|worktree| native(&worktree.path))
            .filter(|path| path.is_dir())
            .collect();
    }
    Ok(check)
}

/// Git prints worktree paths with forward slashes.
fn native(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', "\\"))
}

/// Count, for every branch tip, the commits of `rev_list` (`git rev-list --parents` output:
/// a commit and its parents per line) reachable from it. `rev_list` holds exactly the commits
/// no remote contains, and a commit reachable from a tip but from no remote is reached through
/// such commits only, so the walk stays inside the list. Returns the branches with at least
/// one commit, most first (then by name), and whether the list was cut at `limit`.
fn unpushed_per_branch(
    rev_list: &str,
    tips: &[(String, String)],
    limit: usize,
) -> (Vec<(String, usize)>, bool) {
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for line in rev_list.lines() {
        let mut ids = line.split_whitespace();
        if let Some(commit) = ids.next() {
            parents.insert(commit, ids.collect());
        }
    }
    let capped = parents.len() >= limit;

    let mut counts: Vec<(String, usize)> = tips
        .iter()
        .filter_map(|(name, tip)| {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut stack: Vec<&str> = parents
                .get_key_value(tip.as_str())
                .map(|(commit, _)| *commit)
                .into_iter()
                .collect();
            while let Some(commit) = stack.pop() {
                if !seen.insert(commit) {
                    continue;
                }
                for parent in parents.get(commit).into_iter().flatten() {
                    if parents.contains_key(parent) && !seen.contains(parent) {
                        stack.push(parent);
                    }
                }
            }
            (!seen.is_empty()).then(|| (name.clone(), seen.len()))
        })
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    (counts, capped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tips(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(name, tip)| ((*name).to_owned(), (*tip).to_owned()))
            .collect()
    }

    #[test]
    fn unpushed_commits_are_counted_per_branch() {
        // main: c3 -> c2 -> (pushed p1); topic: t1 -> c2; merged: m1 -> (c3, t1).
        let rev_list = "m1 c3 t1\nc3 c2\nt1 c2\nc2 p1\n";
        let (counts, capped) = unpushed_per_branch(
            rev_list,
            &tips(&[
                ("main", "c3"),
                ("topic", "t1"),
                ("merged", "m1"),
                ("pushed", "p1"),
            ]),
            100,
        );
        assert!(!capped);
        assert_eq!(
            counts,
            vec![
                ("merged".to_owned(), 4),
                ("main".to_owned(), 2),
                ("topic".to_owned(), 2),
            ]
        );
    }

    #[test]
    fn a_full_list_is_reported_as_capped() {
        let (counts, capped) = unpushed_per_branch("a b\nb c\n", &tips(&[("main", "a")]), 2);
        assert!(capped);
        assert_eq!(counts, vec![("main".to_owned(), 2)]);
    }

    #[test]
    fn nothing_unpushed_means_no_branches() {
        let (counts, capped) = unpushed_per_branch("", &tips(&[("main", "a")]), 10);
        assert!(counts.is_empty() && !capped);
    }
}
