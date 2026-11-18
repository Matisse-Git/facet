//! Git LFS output: which paths LFS handles (`git check-attr -z filter lockable`: `path\0
//! attribute\0value\0` triples) and file locks (`git lfs locks --json`, `lock --json`).
//!
//! git-lfs 3.x has no machine-readable `ls-files`; `check-attr` is git's own plumbing and
//! answers exactly what the LFS filter will do with a path.

use serde::Deserialize;

use crate::GitError;

/// What `.gitattributes` says about one path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LfsAttributes {
    /// `filter=lfs`: stored in LFS.
    pub lfs: bool,
    /// `lockable`: should be locked before editing (read-only until locked).
    pub lockable: bool,
}

/// Attributes per path from `check-attr -z filter lockable`, in output order.
pub fn parse_check_attr(data: &[u8]) -> Result<Vec<(String, LfsAttributes)>, GitError> {
    let text = super::utf8("check-attr", data)?;
    let mut out: Vec<(String, LfsAttributes)> = Vec::new();
    // Every field ends in NUL: drop the last terminator so a cut-off record has too few fields.
    let mut fields = text.strip_suffix('\0').unwrap_or(text).split('\0');
    while let Some(path) = fields.next() {
        if path.is_empty() {
            break;
        }
        let (Some(attribute), Some(value)) = (fields.next(), fields.next()) else {
            return Err(GitError::parse("check-attr", "truncated record"));
        };
        let index = match out.last() {
            Some((last, _)) if last == path => out.len() - 1,
            _ => {
                out.push((path.to_owned(), LfsAttributes::default()));
                out.len() - 1
            }
        };
        let entry = &mut out[index].1;
        match attribute {
            "filter" => entry.lfs = value == "lfs",
            "lockable" => entry.lockable = value == "set",
            _ => {}
        }
    }
    Ok(out)
}

/// One LFS file lock on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsLock {
    pub id: String,
    /// Repository-relative, `/`-separated.
    pub path: String,
    pub owner: String,
    /// RFC 3339 as the server sent it.
    pub locked_at: String,
    /// Whether the lock is the current user's: known only when the server supports lock
    /// verification (`locks --verify`).
    pub ours: Option<bool>,
}

#[derive(Deserialize)]
struct RawOwner {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct RawLock {
    id: String,
    path: String,
    #[serde(default)]
    owner: Option<RawOwner>,
    #[serde(default)]
    locked_at: String,
}

impl RawLock {
    fn into_lock(self, ours: Option<bool>) -> LfsLock {
        LfsLock {
            id: self.id,
            path: self.path,
            owner: self.owner.map(|owner| owner.name).unwrap_or_default(),
            locked_at: self.locked_at,
            ours,
        }
    }
}

#[derive(Deserialize)]
struct Verified {
    #[serde(default)]
    ours: Vec<RawLock>,
    #[serde(default)]
    theirs: Vec<RawLock>,
}

fn json_error(command: &'static str, data: &[u8], error: serde_json::Error) -> GitError {
    let text = String::from_utf8_lossy(data);
    let start: String = text.chars().take(160).collect();
    GitError::parse(command, format!("unexpected JSON ({error}): {start}"))
}

/// `git lfs locks --json`: an array of locks (ownership unknown).
pub fn parse_locks(data: &[u8]) -> Result<Vec<LfsLock>, GitError> {
    let raw: Vec<RawLock> =
        serde_json::from_slice(trim(data)).map_err(|error| json_error("lfs locks", data, error))?;
    Ok(raw.into_iter().map(|lock| lock.into_lock(None)).collect())
}

/// `git lfs locks --verify --json`: `{"ours": [...], "theirs": [...]}`.
pub fn parse_verified_locks(data: &[u8]) -> Result<Vec<LfsLock>, GitError> {
    let raw: Verified = serde_json::from_slice(trim(data))
        .map_err(|error| json_error("lfs locks --verify", data, error))?;
    let mut locks: Vec<LfsLock> = raw
        .ours
        .into_iter()
        .map(|lock| lock.into_lock(Some(true)))
        .chain(
            raw.theirs
                .into_iter()
                .map(|lock| lock.into_lock(Some(false))),
        )
        .collect();
    locks.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(locks)
}

/// `git lfs lock --json <path>`: the new lock (it is ours). git-lfs 3.x prints it as a
/// one-element array; a bare object is accepted too.
pub fn parse_new_lock(data: &[u8]) -> Result<LfsLock, GitError> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<RawLock>),
        One(RawLock),
    }
    let raw: OneOrMany =
        serde_json::from_slice(trim(data)).map_err(|error| json_error("lfs lock", data, error))?;
    let raw = match raw {
        OneOrMany::One(lock) => lock,
        OneOrMany::Many(locks) => locks
            .into_iter()
            .next()
            .ok_or_else(|| GitError::parse("lfs lock", "no lock in the output"))?,
    };
    Ok(raw.into_lock(Some(true)))
}

fn trim(data: &[u8]) -> &[u8] {
    let start = data
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(data.len());
    let end = data
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |end| end + 1);
    &data[start..end.max(start)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_group_by_path() {
        let data = b"Art/hero.psd\0filter\0lfs\0Art/hero.psd\0lockable\0set\0\
src/main.rs\0filter\0unspecified\0src/main.rs\0lockable\0unspecified\0\
Maps/a b.umap\0filter\0lfs\0Maps/a b.umap\0lockable\0unset\0";
        let attrs = parse_check_attr(data).unwrap();
        assert_eq!(attrs.len(), 3);
        assert_eq!(
            attrs[0].1,
            LfsAttributes {
                lfs: true,
                lockable: true
            }
        );
        assert_eq!(attrs[1].1, LfsAttributes::default());
        assert_eq!(attrs[2].0, "Maps/a b.umap");
        assert!(attrs[2].1.lfs && !attrs[2].1.lockable);
        assert!(parse_check_attr(b"x\0filter\0").is_err());
        assert!(parse_check_attr(b"").unwrap().is_empty());
    }

    #[test]
    fn lock_lists_with_and_without_verification() {
        let plain = br#"[{"id":"1","path":"Art/hero.psd","owner":{"name":"Ada"},"locked_at":"2026-11-18T10:00:00Z"}]"#;
        let locks = parse_locks(plain).unwrap();
        assert_eq!(locks[0].owner, "Ada");
        assert_eq!(locks[0].ours, None);

        let verified =
            br#"{"ours":[{"id":"2","path":"b.uasset","owner":{"name":"Me"},"locked_at":"x"}],
            "theirs":[{"id":"1","path":"a.uasset","locked_at":"y"}],"next_cursor":""}"#;
        let locks = parse_verified_locks(verified).unwrap();
        assert_eq!(locks[0].path, "a.uasset");
        assert_eq!(locks[0].ours, Some(false));
        assert_eq!(locks[0].owner, "");
        assert_eq!(locks[1].ours, Some(true));

        let new = parse_new_lock(
            b" {\"id\":\"9\",\"path\":\"p\",\"owner\":{\"name\":\"Me\"},\"locked_at\":\"z\"}\n",
        )
        .unwrap();
        assert_eq!((new.id.as_str(), new.ours), ("9", Some(true)));
        assert!(parse_locks(b"Error: not json").is_err());
        assert!(parse_locks(b"[]").unwrap().is_empty());
    }
}
