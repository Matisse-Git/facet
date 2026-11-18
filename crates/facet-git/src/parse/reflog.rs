//! `git log -g -z --date=unix --format=REFLOG_FORMAT <ref>`: where a ref (usually `HEAD`)
//! pointed, newest first. Each entry carries the commit as the history list shows it (with
//! its real parents), so a reflog row can be selected like any history row.

use super::log::{record, Authors, FIELDS};
use super::LogCommit;
use crate::GitError;

/// The history list's fields (`LOG_FORMAT`), then the selector with the entry's time
/// (`HEAD@{1700000000}` thanks to `--date=unix`) and the reflog message ("checkout: moving
/// from a to b").
pub const REFLOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%at%x00%s%x00%gd%x00%gs";
const REFLOG_FIELDS: usize = FIELDS + 2;
const COMMAND: &str = "log -g";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    /// Position in the reflog: `HEAD@{index}`.
    pub index: usize,
    /// Where the ref pointed after this entry.
    pub commit: LogCommit,
    /// When the ref moved (not the commit's own time), seconds since the epoch.
    pub time: i64,
    /// What moved it, split from git's message: "checkout", "commit (amend)", "reset", …
    pub action: String,
    /// The rest of git's message: "moving from main to topic", the commit subject, …
    pub message: String,
}

impl ReflogEntry {
    /// `HEAD@{n}`-style selector for this entry of `refname`'s reflog.
    pub fn selector(&self, refname: &str) -> String {
        format!("{refname}@{{{}}}", self.index)
    }
}

pub fn parse_reflog(data: &[u8]) -> Result<Vec<ReflogEntry>, GitError> {
    let mut entries = Vec::new();
    let mut authors = Authors::default();
    let mut fields = data.split(|byte| *byte == 0);
    loop {
        let record_fields: Vec<&[u8]> = fields.by_ref().take(REFLOG_FIELDS).collect();
        match record_fields.as_slice() {
            [] => break,
            [last] if last.iter().all(u8::is_ascii_whitespace) => break,
            _ if record_fields.len() < REFLOG_FIELDS => {
                return Err(GitError::parse(COMMAND, "truncated reflog record"));
            }
            _ => {}
        }
        let mut commit_fields: [&[u8]; FIELDS] = [&[]; FIELDS];
        commit_fields.copy_from_slice(&record_fields[..FIELDS]);
        let commit = record(&commit_fields, &mut authors)?;
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
        let selector = text(record_fields[FIELDS]);
        let time = selector
            .rsplit_once("@{")
            .and_then(|(_, rest)| rest.strip_suffix('}'))
            .and_then(|time| time.parse().ok())
            .ok_or_else(|| GitError::parse(COMMAND, format!("bad selector {selector:?}")))?;
        let reflog = text(record_fields[FIELDS + 1]);
        let (action, message) = match reflog.split_once(": ") {
            Some((action, message)) => (action.to_owned(), message.to_owned()),
            None => (reflog.clone(), String::new()),
        };
        entries.push(ReflogEntry {
            index: entries.len(),
            commit,
            time,
            action,
            message,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn entries_split_action_and_message() {
        let data = format!(
            "{A}\0{B}\0Ann\x001\0Fix: it\0HEAD@{{1700000100}}\0commit (amend): Fix: it\0\
             {B}\0\0Ann\x001\0Base\0HEAD@{{1700000000}}\0reset: moving to HEAD~1\0"
        );
        let entries = parse_reflog(data.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "commit (amend)");
        assert_eq!(entries[0].message, "Fix: it");
        assert_eq!(entries[0].time, 1_700_000_100);
        assert_eq!(entries[0].commit.parents, [B]);
        assert_eq!(entries[1].index, 1);
        assert_eq!(entries[1].selector("HEAD"), "HEAD@{1}");
        assert_eq!(entries[1].message, "moving to HEAD~1");
        assert_eq!(&*entries[1].commit.subject, "Base");
        assert!(entries[1].commit.parents.is_empty());
    }

    #[test]
    fn empty_and_truncated() {
        assert!(parse_reflog(b"").unwrap().is_empty());
        assert!(parse_reflog(format!("{A}\0\0Ann\0").as_bytes()).is_err());
    }
}
