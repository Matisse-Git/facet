//! One commit in detail: its full message and metadata (`git log -1 -z
//! --format=DETAIL_FORMAT`) and the files it changed (`git diff-tree -r -z -M --raw
//! --numstat`).

use crate::GitError;

/// Hash, parents, author name/email/time, committer name/email/time, then the raw body.
/// The body is the last field, so whatever it contains needs no escaping.
pub const DETAIL_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B";
const DETAIL_FIELDS: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    /// The full message, trailing whitespace removed.
    pub message: String,
}

impl CommitDetail {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }

    /// Everything after the subject and the blank line that follows it.
    pub fn body(&self) -> &str {
        match self.message.split_once('\n') {
            Some((_, rest)) => rest.trim_start_matches(['\n', '\r']),
            None => "",
        }
    }
}

pub fn parse_commit_detail(data: &[u8]) -> Result<CommitDetail, GitError> {
    const COMMAND: &str = "log";
    let text = String::from_utf8_lossy(data);
    let fields: Vec<&str> = text.splitn(DETAIL_FIELDS, '\0').collect();
    if fields.len() != DETAIL_FIELDS {
        return Err(GitError::parse(COMMAND, "truncated commit record"));
    }
    let oid = fields[0].trim().to_owned();
    if oid.len() < 40 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GitError::parse(COMMAND, format!("bad commit id {oid:?}")));
    }
    let number = |field: &str| field.trim().parse::<i64>().unwrap_or(0);
    Ok(CommitDetail {
        oid,
        parents: fields[1]
            .split(' ')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
        author: fields[2].to_owned(),
        author_email: fields[3].to_owned(),
        author_time: number(fields[4]),
        committer: fields[5].to_owned(),
        committer_email: fields[6].to_owned(),
        commit_time: number(fields[7]),
        // `-z` ends the record with NUL.
        message: fields[8].trim_end_matches('\0').trim_end().to_owned(),
    })
}

/// A file changed by a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// Where a rename or copy came from.
    pub orig_path: Option<String>,
    /// `A`dded, `M`odified, `D`eleted, `R`enamed, `C`opied, `T`ype changed.
    pub status: char,
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
    /// Lines added / removed; `None` for binary files.
    pub added: Option<u32>,
    pub removed: Option<u32>,
    /// A gitlink (mode 160000).
    pub submodule: bool,
}

/// `000…0`: the side that doesn't exist (added or deleted file).
fn is_zero_oid(oid: &str) -> bool {
    oid.bytes().all(|b| b == b'0')
}

/// Parse `diff-tree -r -z -M --raw --numstat` output: all raw records, then one numstat
/// record per file in the same order.
pub fn parse_changed_files(data: &[u8]) -> Result<Vec<ChangedFile>, GitError> {
    const COMMAND: &str = "diff-tree";
    let text = String::from_utf8_lossy(data);
    let mut tokens = text.split('\0').peekable();
    let mut files: Vec<ChangedFile> = Vec::new();

    while let Some(token) = tokens.peek() {
        let Some(raw) = token.strip_prefix(':') else {
            break;
        };
        tokens.next();
        // `old_mode new_mode old_oid new_oid status[score]`
        let parts: Vec<&str> = raw.split(' ').collect();
        if parts.len() != 5 {
            return Err(GitError::parse(COMMAND, format!("bad raw record {raw:?}")));
        }
        let status = parts[4]
            .chars()
            .next()
            .ok_or_else(|| GitError::parse(COMMAND, "missing status"))?;
        let first = tokens
            .next()
            .ok_or_else(|| GitError::parse(COMMAND, "missing path"))?
            .to_owned();
        let (path, orig_path) = if matches!(status, 'R' | 'C') {
            let second = tokens
                .next()
                .ok_or_else(|| GitError::parse(COMMAND, "missing rename target"))?
                .to_owned();
            (second, Some(first))
        } else {
            (first, None)
        };
        let oid = |oid: &str| (!is_zero_oid(oid)).then(|| oid.to_owned());
        files.push(ChangedFile {
            path,
            orig_path,
            status,
            old_oid: oid(parts[2]),
            new_oid: oid(parts[3]),
            added: None,
            removed: None,
            submodule: parts[0] == "160000" || parts[1] == "160000",
        });
    }

    // numstat: `added\tremoved\tpath` or, for renames, `added\tremoved\t` + `src` + `dst`.
    for file in &mut files {
        let Some(token) = tokens.next() else {
            break;
        };
        let mut parts = token.splitn(3, '\t');
        let (added, removed, path) = (parts.next(), parts.next(), parts.next());
        if path == Some("") {
            // Rename: two more tokens.
            tokens.next();
            tokens.next();
        }
        file.added = added.and_then(|n| n.parse().ok());
        file.removed = removed.and_then(|n| n.parse().ok());
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_with_a_multiline_body() {
        let data = "1111111111111111111111111111111111111111\0aaa bbb\0Ada\0ada@x\x001700000000\0\
                    Bob\0bob@x\x001700000100\0Subject line\n\nBody line 1\nBody\0with NUL? no\n\n\0";
        let detail = parse_commit_detail(data.as_bytes()).unwrap();
        assert_eq!(detail.parents, ["aaa", "bbb"]);
        assert_eq!(detail.author_email, "ada@x");
        assert_eq!(detail.commit_time, 1_700_000_100);
        assert_eq!(detail.subject(), "Subject line");
        assert!(detail.body().starts_with("Body line 1"));
        assert!(parse_commit_detail(b"abc\0").is_err());
    }

    #[test]
    fn changed_files_with_renames_and_binaries() {
        let z = "0000000000000000000000000000000000000000";
        let data = format!(
            ":100644 100644 aaa bbb M\0src/a.rs\0\
             :000000 100644 {z} ccc A\0new file.txt\0\
             :100644 100644 ddd eee R087\0old/name.rs\0new/name.rs\0\
             :100644 000000 fff {z} D\0gone.bin\0\
             3\t1\tsrc/a.rs\0\
             10\t0\tnew file.txt\0\
             2\t2\t\0old/name.rs\0new/name.rs\0\
             -\t-\tgone.bin\0"
        );
        let files = parse_changed_files(data.as_bytes()).unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].status, 'M');
        assert_eq!((files[0].added, files[0].removed), (Some(3), Some(1)));
        assert_eq!(files[1].old_oid, None);
        assert_eq!(files[1].path, "new file.txt");
        assert_eq!(files[2].status, 'R');
        assert_eq!(files[2].orig_path.as_deref(), Some("old/name.rs"));
        assert_eq!(files[2].path, "new/name.rs");
        assert_eq!(files[2].added, Some(2));
        assert_eq!(files[3].new_oid, None);
        assert_eq!(files[3].added, None, "binary");
        assert!(parse_changed_files(b"").unwrap().is_empty());
        assert!(parse_changed_files(b":100644 bad\0x\0").is_err());
    }
}
