//! Saved commit identities ("Work", "GitHub"…): which one a repository commits as, and whether
//! the repository's remote belongs to a different one. Facet only ever warns: picking an
//! identity is always the user's click, and it is written to the repository's own config.

use facet_git::parse::Remote;

use crate::settings::SavedIdentity;

/// How the author a commit would get fits the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Neither a name nor an email: git refuses to commit.
    Unset,
    Fine,
    /// The remote's host belongs to other saved identities (indices), not this author.
    WrongHost {
        host: String,
        owners: Vec<usize>,
    },
}

/// The saved identity is this author: the same name, and the same email ignoring case.
pub fn same_person(saved: &SavedIdentity, name: &str, email: &str) -> bool {
    saved.name == name && saved.email.eq_ignore_ascii_case(email)
}

/// The first saved identity that is this author.
pub fn find(identities: &[SavedIdentity], name: &str, email: &str) -> Option<usize> {
    identities
        .iter()
        .position(|saved| same_person(saved, name, email))
}

/// Hosts as typed: `github.com, *.example.co.jp  github-personal` → lower-case entries.
pub fn parse_hosts(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for host in text.split([',', ';', ' ', '\t']) {
        let host = host.trim().trim_end_matches('/').to_ascii_lowercase();
        if !host.is_empty() && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// `github.com` matches only `github.com`; `*.example.co.jp` matches `example.co.jp` and every
/// host under it. Both are already lower-case.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(domain) => {
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.ends_with('.'))
        }
        None => host == pattern,
    }
}

/// The host the repository is judged by: `origin`'s, else the only remote's. `None` with no
/// remote, several remotes and no `origin`, or a local path.
pub fn repository_host(remotes: &[Remote]) -> Option<String> {
    let remote = remotes
        .iter()
        .find(|remote| remote.name == "origin")
        .or_else(|| (remotes.len() == 1).then(|| &remotes[0]))?;
    facet_git::forge::remote_host(remote.url.as_deref()?)
}

/// Whether committing as `name` / `email` in a repository on `host` is what the saved
/// identities say.
pub fn standing(
    identities: &[SavedIdentity],
    name: &str,
    email: &str,
    host: Option<&str>,
) -> Standing {
    if name.is_empty() && email.is_empty() {
        return Standing::Unset;
    }
    let Some(host) = host else {
        return Standing::Fine;
    };
    let owners: Vec<usize> = identities
        .iter()
        .enumerate()
        .filter(|(_, saved)| {
            saved
                .hosts
                .iter()
                .any(|pattern| host_matches(pattern, host))
        })
        .map(|(index, _)| index)
        .collect();
    if owners.is_empty()
        || owners
            .iter()
            .any(|index| same_person(&identities[*index], name, email))
    {
        Standing::Fine
    } else {
        Standing::WrongHost {
            host: host.to_owned(),
            owners,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(label: &str, email: &str, hosts: &[&str]) -> SavedIdentity {
        SavedIdentity {
            label: label.to_owned(),
            name: "Me".to_owned(),
            email: email.to_owned(),
            hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
        }
    }

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_owned(),
            url: Some(url.to_owned()),
            push_url: None,
        }
    }

    #[test]
    fn hosts_are_parsed_and_matched() {
        assert_eq!(
            parse_hosts(" GitHub.com, *.corp.example ;github-personal  github.com"),
            ["github.com", "*.corp.example", "github-personal"]
        );
        assert!(host_matches("github.com", "github.com"));
        assert!(!host_matches("github.com", "gist.github.com"));
        assert!(host_matches("*.corp.example", "git.corp.example"));
        assert!(host_matches("*.corp.example", "corp.example"));
        assert!(!host_matches("*.corp.example", "notcorp.example"));
        assert!(host_matches("github-personal", "github-personal"));
    }

    #[test]
    fn the_repository_is_judged_by_origin_or_its_only_remote() {
        assert_eq!(repository_host(&[]), None);
        assert_eq!(
            repository_host(&[
                remote("upstream", "https://github.com/them/app"),
                remote("origin", "git@git.corp.example:me/app.git"),
            ])
            .as_deref(),
            Some("git.corp.example")
        );
        assert_eq!(
            repository_host(&[remote("mine", "https://github.com/me/app")]).as_deref(),
            Some("github.com")
        );
        assert_eq!(
            repository_host(&[
                remote("a", "https://github.com/me/app"),
                remote("b", "https://gitlab.com/me/app"),
            ]),
            None
        );
        assert_eq!(repository_host(&[remote("origin", "D:\\repos\\app")]), None);
    }

    #[test]
    fn identities_are_matched_by_name_and_email_ignoring_case() {
        let list = [
            saved("Work", "me@corp.example", &[]),
            saved("GitHub", "1+me@users.noreply.github.com", &[]),
        ];
        assert_eq!(find(&list, "Me", "ME@corp.example"), Some(0));
        assert_eq!(find(&list, "me", "me@corp.example"), None);
        assert_eq!(find(&list, "Me", "1+me@users.noreply.github.com"), Some(1));
    }

    #[test]
    fn standing_warns_only_when_the_host_belongs_to_someone_else() {
        let list = [
            saved("Work", "me@corp.example", &["*.corp.example"]),
            saved("GitHub", "1+me@users.noreply.github.com", &["github.com"]),
            saved(
                "Other GitHub",
                "2+alt@users.noreply.github.com",
                &["github.com"],
            ),
        ];
        assert_eq!(standing(&list, "", "", Some("github.com")), Standing::Unset);
        // No remote, or a host no identity claims.
        assert_eq!(
            standing(&list, "Me", "me@corp.example", None),
            Standing::Fine
        );
        assert_eq!(
            standing(&list, "Me", "me@corp.example", Some("gitlab.com")),
            Standing::Fine
        );
        // The right identity, or any of several that share the host.
        assert_eq!(
            standing(&list, "Me", "me@corp.example", Some("git.corp.example")),
            Standing::Fine
        );
        assert_eq!(
            standing(
                &list,
                "Me",
                "2+alt@users.noreply.github.com",
                Some("github.com")
            ),
            Standing::Fine
        );
        // Work on GitHub: both GitHub identities are offered.
        assert_eq!(
            standing(&list, "Me", "me@corp.example", Some("github.com")),
            Standing::WrongHost {
                host: "github.com".to_owned(),
                owners: vec![1, 2]
            }
        );
        // An author that is not saved at all is warned about too.
        assert_eq!(
            standing(
                &list,
                "Someone",
                "someone@example.com",
                Some("git.corp.example")
            ),
            Standing::WrongHost {
                host: "git.corp.example".to_owned(),
                owners: vec![0]
            }
        );
    }
}
