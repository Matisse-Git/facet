//! `git stash list -z --format=STASH_FORMAT`.

use crate::GitError;

/// Stash commit, selector (`stash@{0}`), reflog message, creation time, parents.
pub const STASH_FORMAT: &str = "--format=%H%x00%gd%x00%gs%x00%ct%x00%P";
const FIELDS: usize = 5;
const COMMAND: &str = "stash list";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// Position in the stash list (0 is the newest).
    pub index: usize,
    /// `stash@{0}`
    pub selector: String,
    /// The stash commit.
    pub oid: String,
    /// The commit the stash was made on (its first parent).
    pub base: Option<String>,
    /// "On main: message" or "WIP on main: abc123 subject", as git wrote it.
    pub message: String,
    pub time: i64,
    /// Untracked files were stashed too (the stash has a third parent).
    pub untracked: bool,
}

pub fn parse_stashes(data: &[u8]) -> Result<Vec<StashEntry>, GitError> {
    let mut fields = data.split(|byte| *byte == 0).peekable();
    let mut entries = Vec::new();
    loop {
        // Skip the empty tail after the last record's NUL.
        match fields.peek() {
            None => break,
            Some([]) => {
                fields.next();
                if fields.peek().is_none() {
                    break;
                }
                return Err(GitError::parse(COMMAND, "empty stash record"));
            }
            Some(_) => {}
        }
        let record: Vec<&[u8]> = fields.by_ref().take(FIELDS).collect();
        if record.len() != FIELDS {
            return Err(GitError::parse(COMMAND, "truncated stash record"));
        }
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
        let parents: Vec<String> = text(record[4])
            .split(' ')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        entries.push(StashEntry {
            index: entries.len(),
            oid: text(record[0]),
            selector: text(record[1]),
            message: text(record[2]),
            time: text(record[3]).parse().unwrap_or(0),
            base: parents.first().cloned(),
            untracked: parents.len() >= 3,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_records() {
        let data =
            b"aaa\0stash@{0}\0On main: wip with spaces\x001700000000\0base1 idx1 untracked1\0\
                     bbb\0stash@{1}\0WIP on topic: 1234567 subject\x001600000000\0base2 idx2\0";
        let stashes = parse_stashes(data).unwrap();
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].selector, "stash@{0}");
        assert_eq!(stashes[0].message, "On main: wip with spaces");
        assert!(stashes[0].untracked);
        assert_eq!(stashes[0].base.as_deref(), Some("base1"));
        assert_eq!(stashes[1].index, 1);
        assert!(!stashes[1].untracked);
        assert_eq!(stashes[1].time, 1_600_000_000);
        assert!(parse_stashes(b"").unwrap().is_empty());
        assert!(parse_stashes(b"aaa\0stash@{0}\0").is_err());
    }
}
