//! Web pages of a remote on GitHub, GitLab and Azure DevOps: commits, branches and "new
//! pull request" links, derived from the remote's URL. Pure string work; nothing is fetched.

/// A hosting service Facet knows how to link to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeKind {
    GitHub,
    GitLab,
    AzureDevOps,
}

impl ForgeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::AzureDevOps => "Azure DevOps",
        }
    }

    pub fn pull_request_label(self) -> &'static str {
        match self {
            Self::GitLab => "merge request",
            _ => "pull request",
        }
    }
}

/// The web home of a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forge {
    pub kind: ForgeKind,
    /// `https://github.com/owner/repo`, without a trailing slash.
    pub base: String,
}

/// Percent-encode a branch name for a URL path or query (slashes kept for paths).
fn encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let plain = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && byte == b'/');
        if plain {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl Forge {
    /// Recognise a remote URL: `https://`, `ssh://` and scp-like `git@host:path` forms.
    pub fn from_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        // Split into host and path.
        let (host, path) = if let Some(rest) = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .or_else(|| url.strip_prefix("ssh://"))
            .or_else(|| url.strip_prefix("git://"))
        {
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit('@').next()?;
            // Drop a port (`host:22`).
            let host = host.split(':').next()?;
            (host.to_ascii_lowercase(), path.to_owned())
        } else if let Some((user_host, path)) = url.split_once(':') {
            // scp-like syntax: `[user@]host:path` (not a Windows drive letter or local path).
            if user_host.len() < 2 || user_host.contains('/') || user_host.contains('\\') {
                return None;
            }
            let host = user_host.rsplit('@').next()?;
            (host.to_ascii_lowercase(), path.to_owned())
        } else {
            return None;
        };
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        if path.is_empty() {
            return None;
        }

        if host == "github.com" || host.ends_with(".github.com") && host != "gist.github.com" {
            let host = if host.starts_with("ssh.") {
                "github.com".to_owned()
            } else {
                host
            };
            return Some(Self {
                kind: ForgeKind::GitHub,
                base: format!("https://{host}/{path}"),
            });
        }
        if host == "dev.azure.com" || host == "ssh.dev.azure.com" {
            // https: org/project/_git/repo; ssh: v3/org/project/repo
            let parts: Vec<&str> = path.split('/').collect();
            let (org, project, repo) = match parts.as_slice() {
                ["v3", org, project, repo] => (*org, *project, *repo),
                [org, project, "_git", repo] => (*org, *project, *repo),
                _ => return None,
            };
            return Some(Self {
                kind: ForgeKind::AzureDevOps,
                base: format!("https://dev.azure.com/{org}/{project}/_git/{repo}"),
            });
        }
        if let Some(org) = host.strip_suffix(".visualstudio.com") {
            // https: org.visualstudio.com/[DefaultCollection/]project/_git/repo
            let parts: Vec<&str> = path
                .split('/')
                .filter(|part| *part != "DefaultCollection")
                .collect();
            if let [project, "_git", repo] = parts.as_slice() {
                return Some(Self {
                    kind: ForgeKind::AzureDevOps,
                    base: format!("https://dev.azure.com/{org}/{project}/_git/{repo}"),
                });
            }
            return None;
        }
        if host == "gitlab.com" || host.starts_with("gitlab.") {
            return Some(Self {
                kind: ForgeKind::GitLab,
                base: format!("https://{host}/{path}"),
            });
        }
        None
    }

    pub fn commit_url(&self, oid: &str) -> String {
        match self.kind {
            ForgeKind::GitLab => format!("{}/-/commit/{oid}", self.base),
            _ => format!("{}/commit/{oid}", self.base),
        }
    }

    pub fn branch_url(&self, branch: &str) -> String {
        match self.kind {
            ForgeKind::GitHub => format!("{}/tree/{}", self.base, encode(branch, true)),
            ForgeKind::GitLab => format!("{}/-/tree/{}", self.base, encode(branch, true)),
            ForgeKind::AzureDevOps => {
                format!("{}?version=GB{}", self.base, encode(branch, false))
            }
        }
    }

    pub fn tag_url(&self, tag: &str) -> String {
        match self.kind {
            ForgeKind::GitHub => format!("{}/releases/tag/{}", self.base, encode(tag, true)),
            ForgeKind::GitLab => format!("{}/-/tags/{}", self.base, encode(tag, true)),
            ForgeKind::AzureDevOps => format!("{}?version=GT{}", self.base, encode(tag, false)),
        }
    }

    /// Page for opening a pull/merge request from `branch`.
    pub fn new_pull_request_url(&self, branch: &str) -> String {
        match self.kind {
            ForgeKind::GitHub => {
                format!("{}/compare/{}?expand=1", self.base, encode(branch, true))
            }
            ForgeKind::GitLab => format!(
                "{}/-/merge_requests/new?merge_request%5Bsource_branch%5D={}",
                self.base,
                encode(branch, false)
            ),
            ForgeKind::AzureDevOps => format!(
                "{}/pullrequestcreate?sourceRef={}",
                self.base,
                encode(branch, false)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(url: &str) -> Option<(ForgeKind, String)> {
        Forge::from_remote_url(url).map(|forge| (forge.kind, forge.base))
    }

    #[test]
    fn remote_urls_of_each_forge() {
        let github = Some((
            ForgeKind::GitHub,
            "https://github.com/rust-lang/rust".into(),
        ));
        assert_eq!(base("https://github.com/rust-lang/rust.git"), github);
        assert_eq!(base("https://user@github.com/rust-lang/rust"), github);
        assert_eq!(base("git@github.com:rust-lang/rust.git"), github);
        assert_eq!(base("ssh://git@github.com:22/rust-lang/rust.git"), github);
        assert_eq!(
            base("ssh://git@ssh.github.com:443/rust-lang/rust.git"),
            github
        );

        assert_eq!(
            base("git@gitlab.com:group/sub/project.git"),
            Some((
                ForgeKind::GitLab,
                "https://gitlab.com/group/sub/project".into()
            ))
        );
        assert_eq!(
            base("https://git.example.org/team/app"),
            None,
            "self-hosted GitLab without a gitlab.* host is not guessed"
        );
        assert_eq!(
            base("https://gitlab.mycorp.com/team/app.git"),
            Some((
                ForgeKind::GitLab,
                "https://gitlab.mycorp.com/team/app".into()
            ))
        );

        let azure = Some((
            ForgeKind::AzureDevOps,
            "https://dev.azure.com/org/Project/_git/Repo".into(),
        ));
        assert_eq!(
            base("https://org@dev.azure.com/org/Project/_git/Repo"),
            azure
        );
        assert_eq!(base("git@ssh.dev.azure.com:v3/org/Project/Repo"), azure);
        assert_eq!(
            base("https://org.visualstudio.com/Project/_git/Repo"),
            azure
        );
        assert_eq!(
            base("https://org.visualstudio.com/DefaultCollection/Project/_git/Repo"),
            azure
        );

        assert_eq!(base(r"D:\repos\local.git"), None);
        assert_eq!(base("/srv/git/repo.git"), None);
        assert_eq!(base("https://bitbucket.org/a/b.git"), None);
    }

    #[test]
    fn page_urls() {
        let github = Forge::from_remote_url("git@github.com:o/r.git").unwrap();
        assert_eq!(
            github.commit_url("abc"),
            "https://github.com/o/r/commit/abc"
        );
        assert_eq!(
            github.branch_url("feature/a b"),
            "https://github.com/o/r/tree/feature/a%20b"
        );
        assert_eq!(
            github.new_pull_request_url("feature/x"),
            "https://github.com/o/r/compare/feature/x?expand=1"
        );
        let gitlab = Forge::from_remote_url("https://gitlab.com/g/p.git").unwrap();
        assert_eq!(
            gitlab.commit_url("abc"),
            "https://gitlab.com/g/p/-/commit/abc"
        );
        assert_eq!(
            gitlab.new_pull_request_url("f/x"),
            "https://gitlab.com/g/p/-/merge_requests/new?merge_request%5Bsource_branch%5D=f%2Fx"
        );
        let azure = Forge::from_remote_url("https://dev.azure.com/o/p/_git/r").unwrap();
        assert_eq!(
            azure.branch_url("f/x"),
            "https://dev.azure.com/o/p/_git/r?version=GBf%2Fx"
        );
        assert_eq!(
            azure.new_pull_request_url("f/x"),
            "https://dev.azure.com/o/p/_git/r/pullrequestcreate?sourceRef=f%2Fx"
        );
    }
}
