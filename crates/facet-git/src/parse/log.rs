//! `git log -z --format=LOG_FORMAT` output, parsed incrementally as it streams in.

use std::collections::HashSet;
use std::sync::Arc;

use crate::oid::{Hex, Oid, Parents};
use crate::GitError;

/// Hash, parents (space separated), author name, author time, subject. With `-z` every
/// field and every record ends in NUL, so a record is exactly [`FIELDS`] NUL-terminated
/// fields and nothing in it needs unescaping.
pub const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%at%x00%s";
pub(super) const FIELDS: usize = 5;
const COMMAND: &str = "log";

/// One commit of the history list. Kept compact (decision 37): 100k of these stay resident,
/// so ids are inline [`Oid`]s, a single parent needs no allocation and author names are
/// shared between commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogCommit {
    pub oid: Oid,
    pub parents: Parents,
    pub author: Arc<str>,
    /// Author time, seconds since the epoch.
    pub time: i64,
    pub subject: Box<str>,
}

impl LogCommit {
    pub fn short_oid(&self) -> Hex {
        self.oid.short()
    }
}

/// Shares author names between the commits of one read (a history has few authors and
/// many commits).
#[derive(Debug, Default)]
pub struct Authors {
    names: HashSet<Arc<str>>,
}

impl Authors {
    pub fn intern(&mut self, name: &str) -> Arc<str> {
        if let Some(known) = self.names.get(name) {
            return known.clone();
        }
        let name: Arc<str> = Arc::from(name);
        self.names.insert(name.clone());
        name
    }
}

/// Parses records as bytes arrive; partial records wait for the next chunk.
#[derive(Debug, Default)]
pub struct LogParser {
    pending: Vec<u8>,
    authors: Authors,
}

impl LogParser {
    /// Parse every complete record in `pending + chunk`, appending to `out`.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<LogCommit>) -> Result<(), GitError> {
        self.pending.extend_from_slice(chunk);
        let mut fields: [&[u8]; FIELDS] = [&[]; FIELDS];
        let mut field = 0;
        let mut field_start = 0;
        let mut consumed = 0;
        for (at, byte) in self.pending.iter().enumerate() {
            if *byte != 0 {
                continue;
            }
            fields[field] = &self.pending[field_start..at];
            field += 1;
            field_start = at + 1;
            if field == FIELDS {
                out.push(record(&fields, &mut self.authors)?);
                field = 0;
                consumed = at + 1;
            }
        }
        self.pending.drain(..consumed);
        Ok(())
    }

    /// The stream ended: anything left over is a truncated record.
    pub fn finish(self) -> Result<(), GitError> {
        if self.pending.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(GitError::parse(
                COMMAND,
                "output ends in the middle of a commit",
            ))
        }
    }
}

pub(super) fn record(
    fields: &[&[u8]; FIELDS],
    authors: &mut Authors,
) -> Result<LogCommit, GitError> {
    // With `-z` nothing separates records but the NUL; tolerate stray whitespace anyway.
    let raw = String::from_utf8_lossy(fields[0]);
    let raw = raw.trim();
    let oid = Oid::from_hex(raw)
        .filter(|_| raw.len() >= 40)
        .ok_or_else(|| GitError::parse(COMMAND, format!("bad commit id {raw:?}")))?;
    let parents = std::str::from_utf8(fields[1])
        .map_err(|_| GitError::parse(COMMAND, "parent ids are not UTF-8"))?
        .split(' ')
        .filter(|parent| !parent.is_empty())
        .map(|parent| {
            Oid::from_hex(parent.trim())
                .ok_or_else(|| GitError::parse(COMMAND, format!("bad parent id {parent:?}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let time = std::str::from_utf8(fields[3])
        .ok()
        .and_then(|time| time.trim().parse().ok())
        .ok_or_else(|| GitError::parse(COMMAND, "bad author time"))?;
    Ok(LogCommit {
        oid,
        parents: Parents::from_vec(parents),
        author: authors.intern(&String::from_utf8_lossy(fields[2])),
        time,
        subject: String::from_utf8_lossy(fields[4]).into(),
    })
}

/// Parse a complete `git log -z --format=LOG_FORMAT` output.
pub fn parse_log(data: &[u8]) -> Result<Vec<LogCommit>, GitError> {
    let mut parser = LogParser::default();
    let mut out = Vec::new();
    parser.feed(data, &mut out)?;
    parser.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";

    fn bytes() -> Vec<u8> {
        format!(
            "{C}\0{A} {B}\0Ada Lovelace\01700000300\0Merge branch 'x'\0\
             {B}\0{A}\0Grace\01700000200\0Fix: ünïcode subject\0\
             {A}\0\0Grace\01700000100\0Initial\0"
        )
        .into_bytes()
    }

    #[test]
    fn records_parse_with_parents() {
        let commits = parse_log(&bytes()).unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].oid, C);
        assert_eq!(commits[0].parents, [A, B]);
        assert_eq!(&*commits[0].author, "Ada Lovelace");
        assert_eq!(commits[0].time, 1_700_000_300);
        assert_eq!(&*commits[1].subject, "Fix: ünïcode subject");
        assert!(commits[2].parents.is_empty());
        assert_eq!(&*commits[2].short_oid(), "1111111");
    }

    #[test]
    fn records_split_across_chunks_parse_the_same() {
        let data = bytes();
        let whole = parse_log(&data).unwrap();
        for size in [1, 3, 7, 41, 64] {
            let mut parser = LogParser::default();
            let mut out = Vec::new();
            for chunk in data.chunks(size) {
                parser.feed(chunk, &mut out).unwrap();
            }
            parser.finish().unwrap();
            assert_eq!(out, whole, "chunk size {size}");
        }
    }

    #[test]
    fn truncated_and_malformed_output_is_an_error() {
        let data = bytes();
        assert!(parse_log(&data[..data.len() - 3]).is_err());
        assert!(parse_log(b"nothex\0\0a\x001\0s\0").is_err());
        assert!(parse_log(b"").unwrap().is_empty());
    }
}
