//! Everything the sidebar lists: branches, remote-tracking branches, tags, remotes and
//! stashes, worktrees and submodules, read with concurrent commands (G5).

use std::path::Path;

use crate::parse::{
    parse_refs, parse_remotes, parse_stashes, GitRef, RefKind, Remote, StashEntry, Submodule,
    Worktree, REFS_FORMAT, STASH_FORMAT,
};
use crate::{Git, GitError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefsSnapshot {
    pub refs: Vec<GitRef>,
    pub remotes: Vec<Remote>,
    pub stashes: Vec<StashEntry>,
    /// Every worktree, the main one first (one entry when there are no linked worktrees).
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
}

impl RefsSnapshot {
    pub fn local(&self) -> impl Iterator<Item = &GitRef> {
        self.refs.iter().filter(|r| r.kind == RefKind::Local)
    }

    pub fn tags(&self) -> impl Iterator<Item = &GitRef> {
        self.refs.iter().filter(|r| r.kind == RefKind::Tag)
    }

    pub fn remote_branches<'a>(&'a self, remote: &'a str) -> impl Iterator<Item = &'a GitRef> {
        self.refs
            .iter()
            .filter(move |r| matches!(&r.kind, RefKind::Remote { remote: name } if name == remote))
    }

    pub fn find(&self, full_name: &str) -> Option<&GitRef> {
        self.refs.iter().find(|r| r.full_name == full_name)
    }

    pub fn remote(&self, name: &str) -> Option<&Remote> {
        self.remotes.iter().find(|remote| remote.name == name)
    }
}

/// Read all refs, remotes and stashes. Tags come newest first; branches by name.
pub async fn refs(git: &Git, root: &Path) -> Result<RefsSnapshot, GitError> {
    let for_each_ref = git
        .command(["for-each-ref", REFS_FORMAT])
        .current_dir(root)
        .background()
        .run();
    let config = git
        .command(["config", "-z", "--get-regexp", r"^remote\."])
        .current_dir(root)
        .background()
        .output();
    let stash = git
        .command(["stash", "list", "-z", STASH_FORMAT])
        .current_dir(root)
        .background()
        .output();
    let worktrees = crate::worktree::worktrees(git, root);
    let submodules = crate::submodule::submodules(git, root);
    let (refs, config, stash, worktrees, submodules) =
        tokio::join!(for_each_ref, config, stash, worktrees, submodules);

    // `config --get-regexp` exits 1 when nothing matches: no remotes.
    let config = config?;
    let remotes = match config.status {
        0 => parse_remotes(&config.stdout)?,
        1 => Vec::new(),
        code => {
            return Err(GitError::Failed {
                command: "git config --get-regexp".to_owned(),
                code,
                stderr: config.stderr_text(),
            })
        }
    };
    let names: Vec<String> = remotes.iter().map(|remote| remote.name.clone()).collect();
    let mut refs = parse_refs(&refs?.stdout, &names)?;
    let rank = |kind: &RefKind| match kind {
        RefKind::Local => 0,
        RefKind::Remote { .. } => 1,
        RefKind::Tag => 2,
    };
    refs.sort_by(|a, b| {
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| match a.kind {
                RefKind::Tag => b.date.cmp(&a.date).then_with(|| a.name.cmp(&b.name)),
                _ => a.full_name.cmp(&b.full_name),
            })
    });

    // No stash at all is an empty list; `git stash list` succeeds either way.
    let stash = stash?;
    let stashes = if stash.success() {
        parse_stashes(&stash.stdout)?
    } else {
        Vec::new()
    };
    Ok(RefsSnapshot {
        refs,
        remotes,
        stashes,
        worktrees: worktrees?,
        submodules: submodules?,
    })
}
