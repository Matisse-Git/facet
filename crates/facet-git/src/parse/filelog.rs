//! `git log --follow -z --format=LOG_FORMAT --name-status -M -- <path>`: one file's history
//! across renames, with the file's name in every commit.
//!
//! Each record is the five [`LOG_FORMAT`](super::LOG_FORMAT) fields, then — when the commit
//! touched the file — a newline and NUL-terminated name-status entries (`M\0path\0`,
//! `R100\0old\0new\0`). A status token starts with an upper-case letter and a commit id with
//! a lower-case hex digit, which is what tells the next commit from another entry.

use super::log::{record, Authors, FIELDS};
use super::LogCommit;
use crate::GitError;

const COMMAND: &str = "log --follow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLogEntry {
    pub commit: LogCommit,
    /// `A`, `M`, `D`, `R`, `C`, `T`; `' '` when git listed no change (a merge).
    pub status: char,
    /// The file's path in this commit.
    pub path: String,
    /// Its path before this commit, for a rename or copy.
    pub orig_path: Option<String>,
}

pub fn parse_file_log(data: &[u8], path: &str) -> Result<Vec<FileLogEntry>, GitError> {
    let mut tokens = data.split(|byte| *byte == 0).peekable();
    let mut entries: Vec<FileLogEntry> = Vec::new();
    let mut authors = Authors::default();
    // The path the file has in the commit being read: entries walk back in time, so a rename
    // in one commit gives the older name for the commits after it in the list.
    let mut current = path.to_owned();
    loop {
        match tokens.peek() {
            None => break,
            Some(token) if token.iter().all(u8::is_ascii_whitespace) => {
                tokens.next();
                continue;
            }
            Some(_) => {}
        }
        let mut fields: [&[u8]; FIELDS] = [&[]; FIELDS];
        for field in &mut fields {
            *field = tokens
                .next()
                .ok_or_else(|| GitError::parse(COMMAND, "truncated commit record"))?;
        }
        let commit = record(&fields, &mut authors)?;
        let mut entry = FileLogEntry {
            commit,
            status: ' ',
            path: current.clone(),
            orig_path: None,
        };
        // Name-status entries: the first carries the newline git puts after the header.
        while let Some(token) = tokens.peek() {
            let status = token.strip_prefix(b"\n").unwrap_or(token);
            let Some(&letter) = status.first().filter(|b| b.is_ascii_uppercase()) else {
                break;
            };
            tokens.next();
            let mut name = || {
                tokens
                    .next()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| GitError::parse(COMMAND, "truncated name-status entry"))
            };
            let first = name()?;
            let (path, orig) = if matches!(letter, b'R' | b'C') {
                (name()?, Some(first))
            } else {
                (first, None)
            };
            entry.status = char::from(letter);
            entry.path = path;
            entry.orig_path = orig;
        }
        if let Some(orig) = &entry.orig_path {
            current.clone_from(orig);
        } else {
            current.clone_from(&entry.path);
        }
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";

    #[test]
    fn follows_a_rename() {
        let data = format!(
            "{C}\0{B}\0Ada\x001700000300\0Edit\0\nM\0new.txt\0\
             {B}\0{A}\0Ada\x001700000200\0Rename\0\nR100\0old.txt\0new.txt\0\
             {A}\0\0Ada\x001700000100\0Add\0\nA\0old.txt\0"
        );
        let entries = parse_file_log(data.as_bytes(), "new.txt").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            (entries[0].status, entries[0].path.as_str()),
            ('M', "new.txt")
        );
        assert_eq!(entries[1].status, 'R');
        assert_eq!(entries[1].orig_path.as_deref(), Some("old.txt"));
        assert_eq!(
            (entries[2].status, entries[2].path.as_str()),
            ('A', "old.txt")
        );
        assert!(entries[2].commit.parents.is_empty());
    }

    #[test]
    fn commit_without_entries_keeps_the_current_name() {
        let data = format!(
            "{B}\0{A} {C}\0Ada\x001700000200\0Merge\0{A}\0\0Ada\x001700000100\0Add\0\nA\0f\0"
        );
        let entries = parse_file_log(data.as_bytes(), "f").unwrap();
        assert_eq!(entries[0].status, ' ');
        assert_eq!(entries[0].path, "f");
        assert_eq!(entries[1].status, 'A');
    }

    #[test]
    fn empty_and_truncated() {
        assert!(parse_file_log(b"", "f").unwrap().is_empty());
        assert!(parse_file_log(format!("{A}\0\0Ada\0").as_bytes(), "f").is_err());
    }
}
