//! Parser for `git status --porcelain=v2 -z --branch`.
//!
//! With `-z` every record (including the `# branch.*` headers) ends in NUL and paths are never
//! quoted. A rename/copy record (`2 …`) is followed by one extra NUL-terminated field holding
//! the original path.

use crate::GitError;

const COMMAND: &str = "status";

/// What the `# branch.*` headers say about `HEAD`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BranchInfo {
    /// The commit `HEAD` resolves to; `None` on an unborn branch.
    pub oid: Option<String>,
    /// Branch name; `None` when `HEAD` is detached.
    pub head: Option<String>,
    pub upstream: Option<String>,
    /// Commits ahead of / behind the upstream, when there is one and it still exists.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

/// One side (index or working tree) of an entry's `XY` status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Unmodified,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl Change {
    fn from_byte(byte: u8) -> Result<Self, GitError> {
        Ok(match byte {
            b'.' => Self::Unmodified,
            b'M' => Self::Modified,
            b'T' => Self::TypeChanged,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'U' => Self::Unmerged,
            other => {
                return Err(GitError::parse(
                    COMMAND,
                    format!("unknown status letter `{}`", char::from(other)),
                ))
            }
        })
    }

    pub fn is_changed(self) -> bool {
        self != Self::Unmodified
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A tracked file with staged and/or unstaged changes.
    Tracked,
    Untracked,
    Ignored,
    /// A merge conflict; `conflict` holds the two-letter conflict type.
    Unmerged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Path relative to the repository root, with forward slashes.
    pub path: String,
    /// For renames and copies: the path it came from.
    pub orig_path: Option<String>,
    pub kind: EntryKind,
    /// `X`: HEAD → index.
    pub index: Change,
    /// `Y`: index → working tree.
    pub worktree: Change,
    /// For unmerged entries, the raw conflict type (`UU`, `AA`, `DU`, …).
    pub conflict: Option<[u8; 2]>,
    pub submodule: bool,
    /// Blob in `HEAD` (stage 2 "ours" for conflicts); `None` when the file is not there.
    pub head_oid: Option<String>,
    /// Blob in the index; `None` when absent (and for conflicts).
    pub index_oid: Option<String>,
}

impl StatusEntry {
    /// Has changes recorded in the index (shown under "Staged").
    pub fn is_staged(&self) -> bool {
        self.kind == EntryKind::Tracked && self.index.is_changed()
    }

    /// Has something in the working tree that is not in the index (shown under "Changes").
    pub fn is_unstaged(&self) -> bool {
        match self.kind {
            EntryKind::Tracked => self.worktree.is_changed(),
            EntryKind::Untracked | EntryKind::Unmerged => true,
            EntryKind::Ignored => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub branch: BranchInfo,
    /// In git's order: tracked entries sorted by path, then untracked, then ignored.
    pub entries: Vec<StatusEntry>,
}

/// Parse the complete output of `git status --porcelain=v2 -z --branch`.
pub fn parse_status(bytes: &[u8]) -> Result<Status, GitError> {
    let mut status = Status::default();
    let mut records = bytes.split(|b| *b == 0);

    while let Some(record) = records.next() {
        let Some((&tag, rest)) = record.split_first() else {
            continue; // the empty piece after the final NUL
        };
        let rest = rest.strip_prefix(b" ").unwrap_or(rest);
        match tag {
            b'#' => parse_header(rest, &mut status.branch)?,
            b'1' => status.entries.push(parse_ordinary(rest)?),
            b'2' => {
                let mut entry = parse_renamed(rest)?;
                let orig = records
                    .next()
                    .filter(|orig| !orig.is_empty())
                    .ok_or_else(|| GitError::parse(COMMAND, "rename record without origin"))?;
                entry.orig_path = Some(path(orig));
                status.entries.push(entry);
            }
            b'u' => status.entries.push(parse_unmerged(rest)?),
            b'?' => status.entries.push(simple(rest, EntryKind::Untracked)),
            b'!' => status.entries.push(simple(rest, EntryKind::Ignored)),
            other => {
                return Err(GitError::parse(
                    COMMAND,
                    format!("unknown record type `{}`", char::from(other)),
                ))
            }
        }
    }
    Ok(status)
}

fn parse_header(rest: &[u8], branch: &mut BranchInfo) -> Result<(), GitError> {
    let text = String::from_utf8_lossy(rest);
    let (key, value) = text.split_once(' ').unwrap_or((&text, ""));
    match key {
        "branch.oid" => branch.oid = (value != "(initial)").then(|| value.to_owned()),
        "branch.head" => branch.head = (value != "(detached)").then(|| value.to_owned()),
        "branch.upstream" => branch.upstream = Some(value.to_owned()),
        "branch.ab" => {
            let mut parts = value.split(' ');
            let mut count = |sign: char| {
                parts
                    .next()
                    .and_then(|part| part.strip_prefix(sign))
                    .and_then(|digits| digits.parse::<u32>().ok())
                    .ok_or_else(|| GitError::parse(COMMAND, format!("bad branch.ab `{value}`")))
            };
            branch.ahead = Some(count('+')?);
            branch.behind = Some(count('-')?);
        }
        // Headers added by newer gits (stash count, …) are not needed yet.
        _ => {}
    }
    Ok(())
}

/// Split off `count` space-separated fields; the remainder (the path) may contain spaces.
fn fields(rest: &[u8], count: usize) -> Result<(Vec<&[u8]>, &[u8]), GitError> {
    let mut parts = rest.splitn(count + 1, |b| *b == b' ');
    let mut head = Vec::with_capacity(count);
    for _ in 0..count {
        head.push(
            parts
                .next()
                .ok_or_else(|| GitError::parse(COMMAND, "truncated record"))?,
        );
    }
    let tail = parts
        .next()
        .ok_or_else(|| GitError::parse(COMMAND, "record without a path"))?;
    Ok((head, tail))
}

fn xy(field: &[u8]) -> Result<(Change, Change), GitError> {
    match field {
        [x, y] => Ok((Change::from_byte(*x)?, Change::from_byte(*y)?)),
        _ => Err(GitError::parse(COMMAND, "bad XY field")),
    }
}

/// `N...` for a normal file, `S<c><m><u>` for a submodule.
fn is_submodule(field: &[u8]) -> bool {
    field.first() == Some(&b'S')
}

fn oid(field: &[u8]) -> Option<String> {
    if field.iter().all(|b| *b == b'0') {
        None
    } else {
        Some(String::from_utf8_lossy(field).into_owned())
    }
}

fn path(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
fn parse_ordinary(rest: &[u8]) -> Result<StatusEntry, GitError> {
    let (f, tail) = fields(rest, 7)?;
    let (index, worktree) = xy(f[0])?;
    Ok(StatusEntry {
        path: path(tail),
        orig_path: None,
        kind: EntryKind::Tracked,
        index,
        worktree,
        conflict: None,
        submodule: is_submodule(f[1]),
        head_oid: oid(f[5]),
        index_oid: oid(f[6]),
    })
}

/// `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>` (+ NUL + `<origPath>`)
fn parse_renamed(rest: &[u8]) -> Result<StatusEntry, GitError> {
    let (f, tail) = fields(rest, 8)?;
    let (index, worktree) = xy(f[0])?;
    Ok(StatusEntry {
        path: path(tail),
        orig_path: None,
        kind: EntryKind::Tracked,
        index,
        worktree,
        conflict: None,
        submodule: is_submodule(f[1]),
        head_oid: oid(f[5]),
        index_oid: oid(f[6]),
    })
}

/// `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
fn parse_unmerged(rest: &[u8]) -> Result<StatusEntry, GitError> {
    let (f, tail) = fields(rest, 9)?;
    let conflict = match f[0] {
        [x, y] => [*x, *y],
        _ => return Err(GitError::parse(COMMAND, "bad XY field")),
    };
    Ok(StatusEntry {
        path: path(tail),
        orig_path: None,
        kind: EntryKind::Unmerged,
        index: Change::Unmerged,
        worktree: Change::Unmerged,
        conflict: Some(conflict),
        submodule: is_submodule(f[1]),
        head_oid: oid(f[7]),
        index_oid: None,
    })
}

fn simple(rest: &[u8], kind: EntryKind) -> StatusEntry {
    StatusEntry {
        path: path(rest),
        orig_path: None,
        kind,
        index: Change::Unmodified,
        worktree: Change::Unmodified,
        conflict: None,
        submodule: false,
        head_oid: None,
        index_oid: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const Z: &str = "0000000000000000000000000000000000000000";

    fn parse(text: &str) -> Status {
        parse_status(text.as_bytes()).unwrap()
    }

    #[test]
    fn branch_headers() {
        let status = parse(&format!(
            "# branch.oid {A}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +3 -12\0"
        ));
        assert_eq!(
            status.branch,
            BranchInfo {
                oid: Some(A.into()),
                head: Some("main".into()),
                upstream: Some("origin/main".into()),
                ahead: Some(3),
                behind: Some(12),
            }
        );
        assert!(status.entries.is_empty());
    }

    #[test]
    fn unborn_and_detached() {
        let status = parse("# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(status.branch.oid, None);
        assert_eq!(status.branch.head.as_deref(), Some("main"));

        let status = parse(&format!("# branch.oid {A}\0# branch.head (detached)\0"));
        assert_eq!(status.branch.head, None);
        assert_eq!(status.branch.ahead, None);
    }

    #[test]
    fn ordinary_entries_keep_spaces_in_paths() {
        let status = parse(&format!(
            "1 .M N... 100644 100644 100644 {A} {A} Content/My Level.umap\0\
             1 A. N... 000000 100644 100644 {Z} {B} new file.txt\0\
             1 MD N... 100644 100644 000000 {A} {B} both.txt\0"
        ));
        let [modified, added, both] = &status.entries[..] else {
            panic!("expected three entries");
        };
        assert_eq!(modified.path, "Content/My Level.umap");
        assert_eq!(
            (modified.index, modified.worktree),
            (Change::Unmodified, Change::Modified)
        );
        assert!(!modified.is_staged() && modified.is_unstaged());
        assert_eq!(modified.head_oid.as_deref(), Some(A));

        assert_eq!(added.path, "new file.txt");
        assert_eq!(added.head_oid, None);
        assert_eq!(added.index_oid.as_deref(), Some(B));
        assert!(added.is_staged() && !added.is_unstaged());

        assert_eq!(
            (both.index, both.worktree),
            (Change::Modified, Change::Deleted)
        );
        assert!(both.is_staged() && both.is_unstaged());
    }

    #[test]
    fn renames_consume_the_origin_record() {
        let status = parse(&format!(
            "2 R. N... 100644 100644 100644 {A} {A} R100 new name.txt\0old name.txt\0? after.txt\0"
        ));
        assert_eq!(status.entries.len(), 2);
        assert_eq!(status.entries[0].path, "new name.txt");
        assert_eq!(status.entries[0].orig_path.as_deref(), Some("old name.txt"));
        assert_eq!(status.entries[0].index, Change::Renamed);
        assert_eq!(status.entries[1].kind, EntryKind::Untracked);
        assert_eq!(status.entries[1].path, "after.txt");
    }

    #[test]
    fn unmerged_untracked_ignored_and_submodules() {
        let status = parse(&format!(
            "u UU N... 100644 100644 100644 100644 {A} {B} {A} conflict.txt\0\
             1 .M SC.. 160000 160000 160000 {A} {A} libs/engine\0\
             ? été/光.txt\0\
             ! build/\0"
        ));
        let [conflict, submodule, untracked, ignored] = &status.entries[..] else {
            panic!("expected four entries");
        };
        assert_eq!(conflict.kind, EntryKind::Unmerged);
        assert_eq!(conflict.conflict, Some(*b"UU"));
        assert_eq!(conflict.head_oid.as_deref(), Some(B));
        assert!(conflict.is_unstaged() && !conflict.is_staged());
        assert!(submodule.submodule);
        assert_eq!(untracked.path, "été/光.txt");
        assert!(untracked.is_unstaged());
        assert_eq!(ignored.kind, EntryKind::Ignored);
        assert!(!ignored.is_unstaged() && !ignored.is_staged());
    }

    #[test]
    fn empty_output_is_a_clean_tree() {
        assert_eq!(parse_status(b"").unwrap(), Status::default());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_status(b"x what\0").is_err());
        assert!(parse_status(b"1 .M N... 100644\0").is_err());
        assert!(parse_status(b"1 .Q N... 100644 100644 100644 a b path\0").is_err());
        assert!(parse_status(b"2 R. N... 100644 100644 100644 a b R100 new\0").is_err());
        assert!(parse_status(b"# branch.ab 3 12\0").is_err());
    }
}
