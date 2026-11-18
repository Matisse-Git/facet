//! `git worktree list --porcelain -z`: one record per worktree, attributes NUL-terminated,
//! records ended by an empty attribute.
//!
//! ```text
//! worktree <path>\0HEAD <oid>\0branch refs/heads/<name>\0\0
//! worktree <path>\0HEAD <oid>\0detached\0locked <reason>\0\0
//! worktree <path>\0bare\0\0
//! ```

use crate::GitError;

const COMMAND: &str = "worktree list";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path as git prints it (forward slashes on Windows).
    pub path: String,
    /// Checked-out commit; `None` for a bare repository or an unborn branch.
    pub head: Option<String>,
    /// Full ref name of the checked-out branch (`refs/heads/x`); `None` when detached.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    /// Locked against pruning, with the reason when one was given (may be empty).
    pub locked: Option<String>,
    /// Its directory is gone; `git worktree prune` would remove it.
    pub prunable: Option<String>,
    /// The first record: the main worktree (or the bare repository itself).
    pub main: bool,
}

impl Worktree {
    /// The branch name without `refs/heads/`.
    pub fn branch_name(&self) -> Option<&str> {
        let branch = self.branch.as_deref()?;
        Some(branch.strip_prefix("refs/heads/").unwrap_or(branch))
    }

    /// The last path component, for display.
    pub fn name(&self) -> &str {
        self.path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.path)
    }
}

pub fn parse_worktrees(data: &[u8]) -> Result<Vec<Worktree>, GitError> {
    let text = super::utf8(COMMAND, data)?;
    let mut worktrees: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in text.split('\0') {
        if field.is_empty() {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        if key == "worktree" {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            current = Some(Worktree {
                path: value.to_owned(),
                main: worktrees.is_empty(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(worktree) = current.as_mut() else {
            return Err(GitError::parse(
                COMMAND,
                format!("attribute {key:?} before any worktree"),
            ));
        };
        match key {
            "HEAD" => worktree.head = Some(value.to_owned()),
            "branch" => worktree.branch = Some(value.to_owned()),
            "detached" => worktree.detached = true,
            "bare" => worktree.bare = true,
            "locked" => worktree.locked = Some(value.to_owned()),
            "prunable" => worktree.prunable = Some(value.to_owned()),
            // Future attributes are ignored, not errors.
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        worktrees.push(done);
    }
    // An unborn branch prints the null id.
    for worktree in &mut worktrees {
        if worktree
            .head
            .as_deref()
            .is_some_and(|head| head.bytes().all(|b| b == b'0'))
        {
            worktree.head = None;
        }
    }
    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_of_every_kind() {
        let data = b"worktree C:/r/main\0HEAD 1111111111111111111111111111111111111111\0\
branch refs/heads/main\0\0\
worktree C:/r/feature x\0HEAD 2222222222222222222222222222222222222222\0detached\0\
locked on a USB disk\0\0\
worktree C:/r/gone\0HEAD 0000000000000000000000000000000000000000\0branch refs/heads/new\0\
prunable gitdir file points to non-existent location\0\0";
        let list = parse_worktrees(data).unwrap();
        assert_eq!(list.len(), 3);
        assert!(list[0].main && !list[1].main);
        assert_eq!(list[0].branch_name(), Some("main"));
        assert_eq!(list[1].name(), "feature x");
        assert!(list[1].detached && list[1].branch.is_none());
        assert_eq!(list[1].locked.as_deref(), Some("on a USB disk"));
        assert_eq!(list[2].head, None, "unborn");
        assert!(list[2].prunable.is_some());
    }

    #[test]
    fn bare_main_and_garbage() {
        let list = parse_worktrees(b"worktree C:/r/x.git\0bare\0\0").unwrap();
        assert!(list[0].bare && list[0].main);
        assert!(parse_worktrees(b"HEAD abc\0\0").is_err());
        assert!(parse_worktrees(b"").unwrap().is_empty());
    }
}
