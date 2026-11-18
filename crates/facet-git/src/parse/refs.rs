//! Branches, tags and remote-tracking branches (`git for-each-ref --format=REFS_FORMAT`) and
//! remotes (`git config -z --get-regexp ^remote\.`).

use crate::GitError;

/// Full name, object, peeled object (annotated tags), `*` for the checked-out branch,
/// upstream, ahead/behind marker (`=`, `>`, `<`, `<>`), symbolic target, creation date.
/// Ref names cannot contain NUL or newlines, and none of the fields is free text, so each
/// ref is one line of NUL-separated fields.
pub const REFS_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)\
                               %00%(upstream)%00%(upstream:trackshort)%00%(symref)\
                               %00%(creatordate:unix)";
const FIELDS: usize = 8;
const COMMAND: &str = "for-each-ref";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    /// `refs/heads/…`
    Local,
    /// `refs/remotes/<remote>/…`
    Remote { remote: String },
    /// `refs/tags/…`
    Tag,
}

/// How a branch relates to its upstream (from `%(upstream:trackshort)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Track {
    /// No upstream, or the upstream is gone.
    #[default]
    None,
    InSync,
    Ahead,
    Behind,
    Diverged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRef {
    /// `refs/heads/main`
    pub full_name: String,
    /// What people call it: `main`, `origin/main`, `v1.0`.
    pub name: String,
    pub kind: RefKind,
    /// The commit it points at (annotated tags are peeled).
    pub oid: String,
    /// The tag object of an annotated tag.
    pub tag_object: Option<String>,
    /// The checked-out branch.
    pub head: bool,
    /// Upstream as a short name (`origin/main`).
    pub upstream: Option<String>,
    pub track: Track,
    /// Creation date (tagger date for annotated tags, committer date otherwise).
    pub date: i64,
}

impl GitRef {
    /// The branch name within its remote (`feature/x` for `origin/feature/x`).
    pub fn short_name(&self) -> &str {
        match &self.kind {
            RefKind::Remote { remote } => self
                .name
                .strip_prefix(remote.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(&self.name),
            _ => &self.name,
        }
    }
}

/// Parse `for-each-ref` output. `remotes` (configured remote names) split
/// `refs/remotes/<remote>/<branch>` correctly even when names contain slashes. Symbolic
/// refs (`origin/HEAD`), stashes and other namespaces are left out.
pub fn parse_refs(data: &[u8], remotes: &[String]) -> Result<Vec<GitRef>, GitError> {
    let text = super::utf8(COMMAND, data)?;
    let mut refs = Vec::new();
    for line in text.split('\n').filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line.split('\0').collect();
        if fields.len() != FIELDS {
            return Err(GitError::parse(
                COMMAND,
                format!("expected {FIELDS} fields, got {}: {line:?}", fields.len()),
            ));
        }
        let [full_name, object, peeled, head, upstream, track, symref, date] = [
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7],
        ];
        if !symref.is_empty() {
            continue;
        }
        let (kind, name) = if let Some(name) = full_name.strip_prefix("refs/heads/") {
            (RefKind::Local, name)
        } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
            (RefKind::Tag, name)
        } else if let Some(name) = full_name.strip_prefix("refs/remotes/") {
            // The longest configured remote name that prefixes the ref wins; refs of a
            // remote that is no longer configured fall back to the first path component.
            let remote = remotes
                .iter()
                .filter(|remote| {
                    name.strip_prefix(remote.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
                })
                .max_by_key(|remote| remote.len())
                .cloned()
                .unwrap_or_else(|| name.split('/').next().unwrap_or(name).to_owned());
            (RefKind::Remote { remote }, name)
        } else {
            continue;
        };
        let (oid, tag_object) = if peeled.is_empty() {
            (object.to_owned(), None)
        } else {
            (peeled.to_owned(), Some(object.to_owned()))
        };
        refs.push(GitRef {
            full_name: full_name.to_owned(),
            name: name.to_owned(),
            kind,
            oid,
            tag_object,
            head: head == "*",
            upstream: (!upstream.is_empty()).then(|| {
                upstream
                    .strip_prefix("refs/remotes/")
                    .or_else(|| upstream.strip_prefix("refs/heads/"))
                    .unwrap_or(upstream)
                    .to_owned()
            }),
            track: match track {
                "=" => Track::InSync,
                ">" => Track::Ahead,
                "<" => Track::Behind,
                "<>" => Track::Diverged,
                _ => Track::None,
            },
            date: date.trim().parse().unwrap_or(0),
        });
    }
    Ok(refs)
}

/// One configured remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: Option<String>,
    pub push_url: Option<String>,
}

/// Parse `git config -z --get-regexp ^remote\.`: `key\nvalue\0` records. A remote name may
/// contain dots, the variable name never does, so the name is everything between `remote.`
/// and the last dot. Remotes keep the order of the configuration file.
pub fn parse_remotes(data: &[u8]) -> Result<Vec<Remote>, GitError> {
    let text = super::utf8("config", data)?;
    let mut remotes: Vec<Remote> = Vec::new();
    for record in text.split('\0').filter(|record| !record.is_empty()) {
        let (key, value) = record.split_once('\n').unwrap_or((record, ""));
        let Some(rest) = key.strip_prefix("remote.") else {
            continue;
        };
        let Some((name, variable)) = rest.rsplit_once('.') else {
            continue;
        };
        let index = match remotes.iter().position(|remote| remote.name == name) {
            Some(index) => index,
            None => {
                remotes.push(Remote {
                    name: name.to_owned(),
                    url: None,
                    push_url: None,
                });
                remotes.len() - 1
            }
        };
        // Config keys are case-insensitive; git prints them lowercased.
        match variable.to_ascii_lowercase().as_str() {
            "url" if remotes[index].url.is_none() => remotes[index].url = Some(value.to_owned()),
            "pushurl" if remotes[index].push_url.is_none() => {
                remotes[index].push_url = Some(value.to_owned());
            }
            _ => {}
        }
    }
    Ok(remotes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_of_every_kind() {
        let data = "refs/heads/main\0aaa\0\0*\0refs/remotes/origin/main\0>\0\x001700000000\n\
                    refs/heads/topic\0bbb\0\0 \0\0\0\x001700000001\n\
                    refs/remotes/my/fork/feature/x\0ccc\0\0 \0\0\0\x001700000002\n\
                    refs/remotes/origin/HEAD\0aaa\0\0 \0\0\0refs/remotes/origin/main\x000\n\
                    refs/stash\0ddd\0\0 \0\0\0\x001700000003\n\
                    refs/tags/v1.0\0tagobj\0ccc\0 \0\0\0\x001700000004\n\
                    refs/tags/light\0aaa\0\0 \0\0\0\x001700000005\n";
        let remotes = vec!["my".to_owned(), "my/fork".to_owned(), "origin".to_owned()];
        let refs = parse_refs(data.as_bytes(), &remotes).unwrap();
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["main", "topic", "my/fork/feature/x", "v1.0", "light"]
        );
        assert!(refs[0].head && !refs[1].head);
        assert_eq!(refs[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(refs[0].track, Track::Ahead);
        assert_eq!(refs[1].track, Track::None);
        assert_eq!(
            refs[2].kind,
            RefKind::Remote {
                remote: "my/fork".into()
            }
        );
        assert_eq!(refs[2].short_name(), "feature/x");
        assert_eq!(refs[3].oid, "ccc");
        assert_eq!(refs[3].tag_object.as_deref(), Some("tagobj"));
        assert_eq!(refs[4].tag_object, None);
        assert_eq!(refs[4].date, 1_700_000_005);
    }

    #[test]
    fn malformed_refs_are_rejected() {
        assert!(parse_refs(b"refs/heads/x\0aaa\n", &[]).is_err());
        assert!(parse_refs(b"", &[]).unwrap().is_empty());
    }

    #[test]
    fn remotes_with_dots_in_their_names() {
        let data = "remote.origin.url\nhttps://github.com/a/b.git\0\
                    remote.origin.fetch\n+refs/heads/*:refs/remotes/origin/*\0\
                    remote.my.fork.url\ngit@github.com:me/b.git\0\
                    remote.my.fork.pushurl\ngit@github.com:me/b-push.git\0\
                    remote.bare.fetch\n+refs/heads/*:refs/remotes/bare/*\0";
        let remotes = parse_remotes(data.as_bytes()).unwrap();
        assert_eq!(remotes.len(), 3);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(
            remotes[0].url.as_deref(),
            Some("https://github.com/a/b.git")
        );
        assert_eq!(remotes[1].name, "my.fork");
        assert_eq!(
            remotes[1].push_url.as_deref(),
            Some("git@github.com:me/b-push.git")
        );
        assert_eq!(remotes[2].url, None);
    }
}
