//! `git blame --incremental`: which commit last touched each line, streamed as git finds
//! out. Every entry is a header `<oid> <orig-line> <final-line> <count>`, the commit's
//! metadata the first time that commit appears (`author …`, `summary …`, `boundary`,
//! `previous …`), and always a closing `filename <path>`. The file's text is not part of the
//! output; it is read separately.

use std::collections::HashMap;

use crate::GitError;

const COMMAND: &str = "blame --incremental";

/// A commit that owns lines of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlameCommit {
    pub oid: String,
    pub author: String,
    /// Author time, seconds since the epoch.
    pub time: i64,
    pub summary: String,
    /// The history was cut here (a root commit, or the edge of a revision range).
    pub boundary: bool,
    /// Lines that are not committed yet (the all-zero id).
    pub uncommitted: bool,
}

/// `count` lines starting at `line` (1-based, in the blamed file) come from `commit`, where
/// they started at `orig_line` of `filename`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameRange {
    /// Index into [`BlameParser::commits`].
    pub commit: usize,
    pub line: u32,
    pub count: u32,
    pub orig_line: u32,
    /// The file's name in that commit (differs after a rename).
    pub filename: String,
}

/// Incremental parser: feed chunks, collect completed ranges.
#[derive(Debug, Default)]
pub struct BlameParser {
    pending: Vec<u8>,
    commits: Vec<BlameCommit>,
    index: HashMap<String, usize>,
    current: Option<(usize, u32, u32, u32)>,
    /// Commits whose metadata is complete (their first entry has ended).
    complete: usize,
}

impl BlameParser {
    /// Commits seen so far; ranges refer to them by index.
    pub fn commits(&self) -> &[BlameCommit] {
        &self.commits
    }

    /// The commits whose metadata has been read completely, a prefix of [`Self::commits`]:
    /// every range returned so far refers to one of them.
    pub fn complete_commits(&self) -> &[BlameCommit] {
        &self.commits[..self.complete]
    }

    pub fn into_commits(self) -> Vec<BlameCommit> {
        self.commits
    }

    /// Parse every complete line of `pending + chunk`, appending finished ranges to `out`.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<BlameRange>) -> Result<(), GitError> {
        self.pending.extend_from_slice(chunk);
        let mut consumed = 0;
        while let Some(end) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&self.pending[consumed..consumed + end])
                .trim_end_matches('\r')
                .to_owned();
            consumed += end + 1;
            self.line(&line, out)?;
        }
        self.pending.drain(..consumed);
        Ok(())
    }

    /// The stream ended: an unfinished entry means the output was cut short.
    pub fn finish(&mut self, out: &mut Vec<BlameRange>) -> Result<(), GitError> {
        if !self.pending.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned();
            self.line(&line, out)?;
        }
        if self.current.is_some() {
            return Err(GitError::parse(COMMAND, "output ends inside an entry"));
        }
        Ok(())
    }

    fn line(&mut self, line: &str, out: &mut Vec<BlameRange>) -> Result<(), GitError> {
        if line.is_empty() {
            return Ok(());
        }
        let Some((commit, orig_line, final_line, count)) = self.current else {
            return self.header(line);
        };
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let entry = &mut self.commits[commit];
        match key {
            "author" => value.clone_into(&mut entry.author),
            "author-time" => entry.time = value.parse().unwrap_or(0),
            "summary" => value.clone_into(&mut entry.summary),
            "boundary" => entry.boundary = true,
            "filename" => {
                out.push(BlameRange {
                    commit,
                    line: final_line,
                    count,
                    orig_line,
                    filename: unquote(value),
                });
                self.current = None;
                self.complete = self.complete.max(commit + 1);
            }
            // author-mail, committer*, previous, …: not shown.
            _ => {}
        }
        Ok(())
    }

    fn header(&mut self, line: &str) -> Result<(), GitError> {
        let bad = || GitError::parse(COMMAND, format!("bad entry header {line:?}"));
        let mut parts = line.split(' ');
        let oid = parts
            .next()
            .filter(|oid| oid.len() >= 40 && oid.bytes().all(|b| b.is_ascii_hexdigit()));
        let numbers: Vec<u32> = parts
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        let (Some(oid), [orig, line_no, count]) = (oid, numbers.as_slice()) else {
            return Err(bad());
        };
        let commit = match self.index.get(oid) {
            Some(&commit) => commit,
            None => {
                self.commits.push(BlameCommit {
                    oid: oid.to_owned(),
                    uncommitted: oid.bytes().all(|b| b == b'0'),
                    ..BlameCommit::default()
                });
                self.index.insert(oid.to_owned(), self.commits.len() - 1);
                self.commits.len() - 1
            }
        };
        self.current = Some((commit, *orig, *line_no, *count));
        Ok(())
    }
}

/// Undo git's C-style quoting of unusual file names (`"notes \303\251t\303\251.txt"`),
/// which `blame` applies even where `-z` commands don't.
pub(crate) fn unquote(name: &str) -> String {
    let Some(inner) = name
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return name.to_owned();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(byte) = chars.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    match chars.peek() {
                        Some(next @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(next - b'0');
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(u8::try_from(value).unwrap_or(b'?'));
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Parse a complete `git blame --incremental` output.
pub fn parse_blame(data: &[u8]) -> Result<(Vec<BlameCommit>, Vec<BlameRange>), GitError> {
    let mut parser = BlameParser::default();
    let mut ranges = Vec::new();
    parser.feed(data, &mut ranges)?;
    parser.finish(&mut ranges)?;
    Ok((parser.into_commits(), ranges))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn data() -> String {
        format!(
            "{ZERO} 3 3 1\nauthor Not Committed Yet\nauthor-time 1800000000\n\
             summary Version of f from f\nfilename f\n\
             {A} 1 1 2\nauthor Ada\nauthor-time 1700000000\nsummary First\nboundary\n\
             filename old name.txt\n"
        )
    }

    #[test]
    fn ranges_and_commits() {
        let (commits, ranges) = parse_blame(data().as_bytes()).unwrap();
        assert_eq!(commits.len(), 2);
        assert!(commits[0].uncommitted);
        assert_eq!(commits[1].author, "Ada");
        assert!(commits[1].boundary);
        assert_eq!(ranges[1].line, 1);
        assert_eq!(ranges[1].count, 2);
        assert_eq!(ranges[1].filename, "old name.txt");
    }

    #[test]
    fn chunk_boundaries_do_not_matter() {
        let data = data();
        let mut parser = BlameParser::default();
        let mut ranges = Vec::new();
        for chunk in data.as_bytes().chunks(5) {
            parser.feed(chunk, &mut ranges).unwrap();
        }
        parser.finish(&mut ranges).unwrap();
        assert_eq!((parser.commits().len(), ranges.len()), (2, 2));
    }

    #[test]
    fn quoted_names_are_unquoted() {
        assert_eq!(unquote(r#""notes \303\251t\303\251.txt""#), "notes été.txt");
        assert_eq!(unquote(r#""a\"b\\c\td""#), "a\"b\\c\td");
        assert_eq!(unquote("plain name.txt"), "plain name.txt");
    }

    #[test]
    fn truncated_entry_is_an_error() {
        let data = format!("{A} 1 1 2\nauthor Ada\n");
        assert!(parse_blame(data.as_bytes()).is_err());
        assert!(parse_blame(b"not a header\n").is_err());
    }
}
