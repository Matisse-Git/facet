use super::utf8;
use crate::GitError;

/// `--format` string producing one [`CommitSummary`] record. Fields are NUL-separated; use it
/// together with `-z` so the record itself is NUL-terminated.
pub const COMMIT_SUMMARY_FORMAT: &str = "--format=%H%x00%h%x00%an%x00%at%x00%s";

const COMMAND: &str = "log";

/// The one-line identity of a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    /// Full object id.
    pub oid: String,
    /// Abbreviated object id, as git chose to abbreviate it for this repository.
    pub short_oid: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub author_time: i64,
    pub subject: String,
}

/// Parse a single record produced by `git log -1 -z` with [`COMMIT_SUMMARY_FORMAT`].
pub fn parse_commit_summary(bytes: &[u8]) -> Result<CommitSummary, GitError> {
    // `-z` terminates the record with one NUL; tolerate its absence.
    let record = bytes.strip_suffix(b"\0").unwrap_or(bytes);
    let mut fields = record.splitn(5, |b| *b == 0);
    let mut next = |name: &str| {
        fields
            .next()
            .ok_or_else(|| GitError::parse(COMMAND, format!("missing field `{name}`")))
            .and_then(|field| utf8(COMMAND, field))
    };

    let oid = next("oid")?;
    let short_oid = next("short oid")?;
    let author = next("author")?;
    let author_time = next("author time")?;
    let subject = next("subject")?;

    if oid.is_empty() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GitError::parse(COMMAND, format!("bad object id `{oid}`")));
    }
    let author_time = author_time
        .parse::<i64>()
        .map_err(|_| GitError::parse(COMMAND, format!("bad author time `{author_time}`")))?;

    Ok(CommitSummary {
        oid: oid.to_owned(),
        short_oid: short_oid.to_owned(),
        author: author.to_owned(),
        author_time,
        subject: subject.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_record() {
        let raw = b"0123456789abcdef0123456789abcdef01234567\x000123456\x00Ada Lovelace\x001700000000\x00Add engine\x00";
        let commit = parse_commit_summary(raw).unwrap();
        assert_eq!(commit.oid, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(commit.short_oid, "0123456");
        assert_eq!(commit.author, "Ada Lovelace");
        assert_eq!(commit.author_time, 1_700_000_000);
        assert_eq!(commit.subject, "Add engine");
    }

    #[test]
    fn empty_subject_and_author_are_valid() {
        let raw = b"abc123\x00abc\x00\x000\x00\x00";
        let commit = parse_commit_summary(raw).unwrap();
        assert_eq!(commit.author, "");
        assert_eq!(commit.subject, "");
        assert_eq!(commit.author_time, 0);
    }

    #[test]
    fn rejects_truncated_and_garbage_input() {
        assert!(parse_commit_summary(b"").is_err());
        assert!(parse_commit_summary(b"abc123\x00abc\x00author").is_err());
        assert!(parse_commit_summary(b"not-hex\x00abc\x00a\x001\x00s\x00").is_err());
        assert!(parse_commit_summary(b"abc123\x00abc\x00a\x00yesterday\x00s\x00").is_err());
        assert!(parse_commit_summary(b"abc123\x00abc\x00\xff\xfe\x001\x00s\x00").is_err());
    }
}
