//! Parser for the diff of one file as produced by `git diff --numstat -z -p`.
//!
//! The output is the `--numstat -z` record(s), one empty NUL-terminated record as separator,
//! then the patch. `--numstat` is what tells text from binary (`-\t-\t`) without reading any
//! sentence, and the patch is git's own machine format (extended headers + unified hunks).
//!
//! The patch bytes are kept as-is and lines are indexed by offset, so a 10 MB diff costs one
//! buffer plus 20 bytes per line and nothing is decoded until a row is actually shown.

use std::ops::Range;

use crate::GitError;

const COMMAND: &str = "diff";
const NO_NEWLINE_MARKER: u8 = b'\\';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk. `start..end` is the content inside [`FileDiff::data`]: no `+`/`-`/space
/// prefix and no trailing `\n`, but a `\r` (CRLF file) is kept so patches round-trip exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub start: u32,
    pub end: u32,
    /// 1-based line number in the old file (0 for added lines).
    pub old_no: u32,
    /// 1-based line number in the new file (0 for removed lines).
    pub new_no: u32,
    /// Followed by `\ No newline at end of file`.
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The whole `@@ -a,b +c,d @@ section` line inside [`FileDiff::data`] (no newline).
    pub header: Range<u32>,
    /// Indices into [`FileDiff::lines`].
    pub lines: Range<u32>,
}

/// The parsed diff of a single file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    /// Raw patch text that `hunks` and `lines` point into.
    pub data: Vec<u8>,
    /// Git considers the content binary: there are no hunks.
    pub binary: bool,
    pub new_file: bool,
    pub deleted_file: bool,
    pub old_mode: Option<u32>,
    pub new_mode: Option<u32>,
    /// Rename/copy similarity in percent.
    pub similarity: Option<u8>,
    /// Line counts from `--numstat` (0 for binary files).
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<Hunk>,
    pub lines: Vec<DiffLine>,
}

impl FileDiff {
    /// Nothing differs (git printed nothing for the path).
    pub fn is_empty(&self) -> bool {
        !self.binary
            && self.hunks.is_empty()
            && !self.new_file
            && !self.deleted_file
            && self.old_mode == self.new_mode
            && self.similarity.is_none()
    }

    pub fn line_bytes(&self, line: &DiffLine) -> &[u8] {
        &self.data[line.start as usize..line.end as usize]
    }

    pub fn header_bytes(&self, hunk: &Hunk) -> &[u8] {
        &self.data[hunk.header.start as usize..hunk.header.end as usize]
    }

    /// The hunk that contains line `index`.
    pub fn hunk_of_line(&self, index: usize) -> Option<usize> {
        let index = index as u32;
        let at = self.hunks.partition_point(|hunk| hunk.lines.end <= index);
        (at < self.hunks.len() && self.hunks[at].lines.contains(&index)).then_some(at)
    }
}

/// Parse the output of `git diff --numstat -z -p` for one file. Empty output is an empty diff.
pub fn parse_file_diff(output: Vec<u8>) -> Result<FileDiff, GitError> {
    let mut diff = FileDiff::default();
    if output.is_empty() {
        return Ok(diff);
    }

    // --- numstat records, up to the empty separator record ---------------------------------
    let mut pos = 0usize;
    let mut first = true;
    loop {
        let Some(len) = output[pos..].iter().position(|b| *b == 0) else {
            // No separator: numstat only (cannot happen with -p, but harmless).
            pos = output.len();
            break;
        };
        let record = &output[pos..pos + len];
        pos += len + 1;
        if record.is_empty() {
            break;
        }
        if first {
            first = false;
            parse_numstat(record, &mut diff)?;
        }
        // `added\tdeleted\t` with an empty path means a rename: two more path fields follow.
        if record.ends_with(b"\t") {
            for _ in 0..2 {
                match output[pos..].iter().position(|b| *b == 0) {
                    Some(len) => pos += len + 1,
                    None => return Err(GitError::parse(COMMAND, "truncated numstat rename")),
                }
            }
        }
    }

    // --- patch ------------------------------------------------------------------------------
    // Line offsets are stored as u32.
    if u32::try_from(output.len()).is_err() {
        return Err(GitError::parse(COMMAND, "diff larger than 4 GiB"));
    }
    let mut seen_file = false;
    let mut old_left = 0u32;
    let mut new_left = 0u32;
    let mut old_no = 0u32;
    let mut new_no = 0u32;

    while pos < output.len() {
        let end = output[pos..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(output.len(), |len| pos + len);
        let line = &output[pos..end];
        let next = (end + 1).min(output.len());

        if old_left > 0 || new_left > 0 {
            // Inside a hunk: the first byte is the line type. An entirely empty line is an
            // empty context line (`diff.suppressBlankEmpty`).
            let (kind, content_start) = match line.first() {
                Some(b'+') => (LineKind::Added, pos + 1),
                Some(b'-') => (LineKind::Removed, pos + 1),
                Some(b' ') => (LineKind::Context, pos + 1),
                None => (LineKind::Context, pos),
                Some(&NO_NEWLINE_MARKER) => {
                    mark_no_newline(&mut diff);
                    pos = next;
                    continue;
                }
                Some(_) => return Err(GitError::parse(COMMAND, "unexpected line inside a hunk")),
            };
            let (old, new) = match kind {
                LineKind::Context => (true, true),
                LineKind::Added => (false, true),
                LineKind::Removed => (true, false),
            };
            if (old && old_left == 0) || (new && new_left == 0) {
                return Err(GitError::parse(COMMAND, "hunk longer than its header says"));
            }
            diff.lines.push(DiffLine {
                kind,
                start: content_start as u32,
                end: end as u32,
                old_no: if old { old_no } else { 0 },
                new_no: if new { new_no } else { 0 },
                no_newline: false,
            });
            if old {
                old_left -= 1;
                old_no += 1;
            }
            if new {
                new_left -= 1;
                new_no += 1;
            }
            if let Some(hunk) = diff.hunks.last_mut() {
                hunk.lines.end = diff.lines.len() as u32;
            }
        } else if line.first() == Some(&NO_NEWLINE_MARKER) {
            mark_no_newline(&mut diff);
        } else if line.starts_with(b"@@ ") {
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
            old_left = old_count;
            new_left = new_count;
            old_no = old_start;
            new_no = new_start;
            let at = diff.lines.len() as u32;
            diff.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                header: pos as u32..end as u32,
                lines: at..at,
            });
        } else if line.starts_with(b"diff --git ") {
            if seen_file {
                break; // a second file: only the first one was asked for
            }
            seen_file = true;
        } else if let Some(mode) = line.strip_prefix(b"new file mode ") {
            diff.new_file = true;
            diff.new_mode = parse_mode(mode);
        } else if let Some(mode) = line.strip_prefix(b"deleted file mode ") {
            diff.deleted_file = true;
            diff.old_mode = parse_mode(mode);
        } else if let Some(mode) = line.strip_prefix(b"old mode ") {
            diff.old_mode = parse_mode(mode);
        } else if let Some(mode) = line.strip_prefix(b"new mode ") {
            diff.new_mode = parse_mode(mode);
        } else if let Some(rest) = line.strip_prefix(b"similarity index ") {
            diff.similarity = std::str::from_utf8(rest)
                .ok()
                .and_then(|text| text.trim_end_matches('%').parse().ok());
        } else if let Some(rest) = line.strip_prefix(b"index ") {
            // `index <old>..<new> <mode>`: the mode of an unchanged-mode modification.
            if let Some(space) = rest.iter().position(|b| *b == b' ') {
                let mode = parse_mode(&rest[space + 1..]);
                diff.old_mode = diff.old_mode.or(mode);
                diff.new_mode = diff.new_mode.or(mode);
            }
        }
        // Everything else before the first hunk (`---`, `+++`, rename lines, the binary
        // notice) carries nothing that numstat and the status entry do not already give.
        pos = next;
    }

    if old_left > 0 || new_left > 0 {
        return Err(GitError::parse(COMMAND, "truncated hunk"));
    }
    diff.data = output;
    Ok(diff)
}

fn mark_no_newline(diff: &mut FileDiff) {
    if let Some(line) = diff.lines.last_mut() {
        line.no_newline = true;
    }
}

/// `<added>\t<deleted>\t<path>`; binary files report `-\t-`.
fn parse_numstat(record: &[u8], diff: &mut FileDiff) -> Result<(), GitError> {
    let mut parts = record.splitn(3, |b| *b == b'\t');
    let (Some(added), Some(removed), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(GitError::parse(COMMAND, "bad numstat record"));
    };
    if added == b"-" && removed == b"-" {
        diff.binary = true;
        return Ok(());
    }
    let number = |field: &[u8]| {
        std::str::from_utf8(field)
            .ok()
            .and_then(|text| text.parse::<u32>().ok())
            .ok_or_else(|| GitError::parse(COMMAND, "bad numstat count"))
    };
    diff.added = number(added)?;
    diff.removed = number(removed)?;
    Ok(())
}

fn parse_mode(bytes: &[u8]) -> Option<u32> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| u32::from_str_radix(text.trim(), 8).ok())
}

/// `@@ -<start>[,<count>] +<start>[,<count>] @@[ section]`
fn parse_hunk_header(line: &[u8]) -> Result<(u32, u32, u32, u32), GitError> {
    let bad = || GitError::parse(COMMAND, "bad hunk header");
    // Only the section text after the second `@@` can be non-UTF-8; lossy decoding is fine.
    let text = String::from_utf8_lossy(line.get(3..).ok_or_else(bad)?);
    let ranges = text.split(" @@").next().ok_or_else(bad)?;
    let mut parts = ranges.split(' ');
    let range = |part: Option<&str>, sign: char| -> Result<(u32, u32), GitError> {
        let part = part.and_then(|p| p.strip_prefix(sign)).ok_or_else(bad)?;
        let (start, count) = match part.split_once(',') {
            Some((start, count)) => (start, count),
            None => (part, "1"),
        };
        Ok((
            start.parse().map_err(|_| bad())?,
            count.parse().map_err(|_| bad())?,
        ))
    };
    let (old_start, old_count) = range(parts.next(), '-')?;
    let (new_start, new_count) = range(parts.next(), '+')?;
    Ok((old_start, old_count, new_start, new_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> FileDiff {
        parse_file_diff(text.as_bytes().to_vec()).unwrap()
    }

    fn texts(diff: &FileDiff) -> Vec<(LineKind, String, u32, u32)> {
        diff.lines
            .iter()
            .map(|line| {
                (
                    line.kind,
                    String::from_utf8_lossy(diff.line_bytes(line)).into_owned(),
                    line.old_no,
                    line.new_no,
                )
            })
            .collect()
    }

    const MODIFIED: &str = "2\t1\tf.txt\0\0diff --git a/f.txt b/f.txt\nindex de98044..f8f7a32 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,4 @@ fn main()\n a\n-b\n+B\n c\n+d\n\\ No newline at end of file\n";

    #[test]
    fn modified_text_file() {
        let diff = parse(MODIFIED);
        assert!(!diff.binary && !diff.new_file && !diff.deleted_file);
        assert_eq!((diff.added, diff.removed), (2, 1));
        assert_eq!(
            (diff.old_mode, diff.new_mode),
            (Some(0o100644), Some(0o100644))
        );
        assert_eq!(diff.hunks.len(), 1);
        let hunk = &diff.hunks[0];
        assert_eq!(
            (
                hunk.old_start,
                hunk.old_count,
                hunk.new_start,
                hunk.new_count
            ),
            (1, 3, 1, 4)
        );
        assert_eq!(diff.header_bytes(hunk), b"@@ -1,3 +1,4 @@ fn main()");
        assert_eq!(hunk.lines, 0..5);
        assert_eq!(
            texts(&diff),
            vec![
                (LineKind::Context, "a".into(), 1, 1),
                (LineKind::Removed, "b".into(), 2, 0),
                (LineKind::Added, "B".into(), 0, 2),
                (LineKind::Context, "c".into(), 3, 3),
                (LineKind::Added, "d".into(), 0, 4),
            ]
        );
        assert!(diff.lines[4].no_newline);
        assert!(!diff.lines[3].no_newline);
        assert_eq!(diff.hunk_of_line(4), Some(0));
        assert_eq!(diff.hunk_of_line(5), None);
    }

    #[test]
    fn binary_new_deleted_and_rename() {
        let binary = parse("-\t-\tb.bin\0\0diff --git a/b.bin b/b.bin\nindex 8352675..00ffac9 100644\nBinary files a/b.bin and b/b.bin differ\n");
        assert!(binary.binary);
        assert!(binary.hunks.is_empty());
        assert!(!binary.is_empty());

        let new = parse("1\t0\t\0nul\0u.txt\0\0diff --git a/u.txt b/u.txt\nnew file mode 100755\nindex 0000000..3e75765\n--- /dev/null\n+++ b/u.txt\n@@ -0,0 +1 @@\n+new\n");
        assert!(new.new_file);
        assert_eq!(new.new_mode, Some(0o100755));
        assert_eq!(texts(&new), vec![(LineKind::Added, "new".into(), 0, 1)]);

        let deleted = parse("0\t2\tgone.txt\0\0diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 3e75765..0000000\n--- a/gone.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n");
        assert!(deleted.deleted_file);
        assert_eq!(deleted.lines.len(), 2);
        assert_eq!(deleted.lines[1].old_no, 2);

        let rename = parse("0\t0\t\0sp ace.txt\0re named.txt\0\0diff --git a/sp ace.txt b/re named.txt\nsimilarity index 100%\nrename from sp ace.txt\nrename to re named.txt\n");
        assert_eq!(rename.similarity, Some(100));
        assert!(rename.hunks.is_empty() && !rename.is_empty());
    }

    #[test]
    fn tricky_content_is_not_mistaken_for_structure() {
        // A removed line `-- x` prints as `--- x`; an added `++ y` as `+++ y`; a line that is
        // literally `@@ -1 +1 @@` inside a hunk is content. Counts, not prefixes, end a hunk.
        let diff = parse("3\t3\tt\0\0diff --git a/t b/t\n--- a/t\n+++ b/t\n@@ -1,3 +1,3 @@\n--- x\n-diff --git a/q b/q\n-@@ -1 +1 @@\n+++ y\n+\\ not a marker\n+\n@@ -10 +10 @@\n-p\r\n+q\r\n");
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(diff.hunks[0].lines, 0..6);
        let lines = texts(&diff);
        assert_eq!(lines[0].1, "-- x");
        assert_eq!(lines[1].1, "diff --git a/q b/q");
        assert_eq!(lines[2].1, "@@ -1 +1 @@");
        assert_eq!(lines[3].1, "++ y");
        assert_eq!(lines[4].1, "\\ not a marker");
        assert_eq!(lines[5].1, "");
        // CRLF content keeps its `\r`; counts default to 1.
        assert_eq!(lines[6], (LineKind::Removed, "p\r".into(), 10, 0));
        assert_eq!(lines[7], (LineKind::Added, "q\r".into(), 0, 10));
    }

    #[test]
    fn blank_context_lines_and_a_second_file() {
        let diff = parse("1\t0\ta\0\0diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,3 @@\n x\n\n+y\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-1\n+2\n");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.lines.len(), 3);
        assert_eq!(diff.lines[1].kind, LineKind::Context);
        assert!(diff.line_bytes(&diff.lines[1]).is_empty());
    }

    #[test]
    fn empty_and_broken_input() {
        assert!(parse("").is_empty());
        assert!(
            parse_file_diff(b"1\t1\tf\0\0diff --git a/f b/f\n@@ -1,2 +1,2 @@\n a\n".to_vec())
                .is_err()
        );
        assert!(parse_file_diff(b"1\t1\tf\0\0diff --git a/f b/f\n@@ nope @@\n".to_vec()).is_err());
        assert!(parse_file_diff(b"x\ty\tf\0\0".to_vec()).is_err());
        assert!(parse_file_diff(b"1\t1\tf\0\0@@ -1 +1 @@\n?bad\n".to_vec()).is_err());
    }
}
