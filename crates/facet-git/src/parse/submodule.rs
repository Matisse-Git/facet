//! Submodules: their declarations in `.gitmodules` (`git config -z --file .gitmodules
//! --get-regexp ^submodule\.`: `key\nvalue\0` records) and the commits the superproject
//! records for them (`git ls-files -z --stage -- <paths>`: `mode oid stage\tpath\0`, gitlinks
//! have mode 160000).

use crate::GitError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Submodule {
    /// The name in `.gitmodules` (usually the path).
    pub name: String,
    /// Path in the superproject's working tree, `/`-separated.
    pub path: String,
    pub url: Option<String>,
    /// `submodule.<name>.branch`, when set.
    pub branch: Option<String>,
    /// The commit recorded in the superproject's index; `None` when the gitlink is missing
    /// (declared but never added, or removed from the index).
    pub recorded: Option<String>,
    /// The submodule's working tree exists (`<path>/.git` is present): it was initialized
    /// and checked out.
    pub populated: bool,
}

impl Submodule {
    /// The last path component, for display.
    pub fn short_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Declarations from `.gitmodules`, in file order. Entries without a path are skipped (git
/// ignores them too).
pub fn parse_gitmodules(data: &[u8]) -> Result<Vec<Submodule>, GitError> {
    let text = super::utf8("config --file .gitmodules", data)?;
    let mut modules: Vec<Submodule> = Vec::new();
    for record in text.split('\0').filter(|record| !record.is_empty()) {
        let (key, value) = record.split_once('\n').unwrap_or((record, ""));
        let Some(rest) = key.strip_prefix("submodule.") else {
            continue;
        };
        let Some((name, variable)) = rest.rsplit_once('.') else {
            continue;
        };
        let index = match modules.iter().position(|module| module.name == name) {
            Some(index) => index,
            None => {
                modules.push(Submodule {
                    name: name.to_owned(),
                    ..Submodule::default()
                });
                modules.len() - 1
            }
        };
        let module = &mut modules[index];
        match variable.to_ascii_lowercase().as_str() {
            "path" => module.path = value.trim_end_matches('/').to_owned(),
            "url" => module.url = Some(value.to_owned()),
            "branch" => module.branch = Some(value.to_owned()),
            _ => {}
        }
    }
    modules.retain(|module| !module.path.is_empty());
    Ok(modules)
}

/// `(path, oid)` of every gitlink in `ls-files --stage -z` output (other entries ignored).
pub fn parse_gitlinks(data: &[u8]) -> Result<Vec<(String, String)>, GitError> {
    let text = super::utf8("ls-files --stage", data)?;
    let mut links = Vec::new();
    for record in text.split('\0').filter(|record| !record.is_empty()) {
        let (info, path) = record
            .split_once('\t')
            .ok_or_else(|| GitError::parse("ls-files --stage", "entry without a tab"))?;
        let mut parts = info.split(' ');
        let (Some(mode), Some(oid)) = (parts.next(), parts.next()) else {
            return Err(GitError::parse("ls-files --stage", "short entry"));
        };
        if mode == "160000" {
            links.push((path.to_owned(), oid.to_owned()));
        }
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_merge_by_name() {
        let data = b"submodule.libs/engine.path\nlibs/engine\0\
submodule.libs/engine.url\nhttps://example.com/engine.git\0\
submodule.Art.path\nart/\0submodule.Art.branch\nmain\0\
submodule.orphan.url\n../orphan\0";
        let modules = parse_gitmodules(data).unwrap();
        assert_eq!(modules.len(), 2, "the entry without a path is dropped");
        assert_eq!(modules[0].name, "libs/engine");
        assert_eq!(modules[0].short_name(), "engine");
        assert_eq!(
            modules[0].url.as_deref(),
            Some("https://example.com/engine.git")
        );
        assert_eq!(modules[1].path, "art");
        assert_eq!(modules[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn only_gitlinks_are_kept() {
        let data = b"100644 1111111111111111111111111111111111111111 0\tREADME.md\0\
160000 2222222222222222222222222222222222222222 0\tlibs/engine\0";
        assert_eq!(
            parse_gitlinks(data).unwrap(),
            [(
                "libs/engine".to_owned(),
                "2222222222222222222222222222222222222222".to_owned()
            )]
        );
        assert!(parse_gitlinks(b"garbage\0").is_err());
    }
}
