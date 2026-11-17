use super::utf8;
use crate::GitError;

const COMMAND: &str = "version";

/// Version of the git executable, used to gate features (fsmonitor, `--path-format`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parse the output of `git version`.
///
/// This is the one place a "sentence" is read: the `git version X.Y.Z…` line is git's documented,
/// stable way to report its version. Vendor suffixes (`.windows.1`, `-rc0`, ` (Apple Git-154)`)
/// are ignored.
pub fn parse_version(bytes: &[u8]) -> Result<GitVersion, GitError> {
    let text = utf8(COMMAND, bytes)?;
    let line = text.lines().next().unwrap_or_default();
    let rest = line
        .strip_prefix("git version ")
        .ok_or_else(|| GitError::parse(COMMAND, format!("unrecognized line `{line}`")))?;

    let mut numbers = rest
        .split(|c: char| !c.is_ascii_digit())
        .take_while(|part| !part.is_empty())
        .map(str::parse::<u32>);
    let mut next = || match numbers.next() {
        Some(Ok(n)) => Ok(Some(n)),
        Some(Err(_)) => Err(GitError::parse(COMMAND, format!("bad number in `{line}`"))),
        None => Ok(None),
    };

    let major =
        next()?.ok_or_else(|| GitError::parse(COMMAND, format!("no version in `{line}`")))?;
    let minor = next()?.unwrap_or(0);
    let patch = next()?.unwrap_or(0);
    Ok(GitVersion {
        major,
        minor,
        patch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_versions() {
        let cases: &[(&str, GitVersion)] = &[
            ("git version 2.45.1.windows.1\n", GitVersion::new(2, 45, 1)),
            (
                "git version 2.39.5 (Apple Git-154)\n",
                GitVersion::new(2, 39, 5),
            ),
            ("git version 2.47.0\n", GitVersion::new(2, 47, 0)),
            ("git version 2.48.0-rc1\n", GitVersion::new(2, 48, 0)),
            ("git version 2.50.GIT\n", GitVersion::new(2, 50, 0)),
            ("git version 3\r\n", GitVersion::new(3, 0, 0)),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_version(raw.as_bytes()).unwrap(), *expected, "{raw:?}");
        }
    }

    #[test]
    fn rejects_other_output() {
        assert!(parse_version(b"").is_err());
        assert!(parse_version(b"hg version 6.0\n").is_err());
        assert!(parse_version(b"git version unknown\n").is_err());
    }

    #[test]
    fn versions_order_naturally() {
        assert!(GitVersion::new(2, 45, 1) > GitVersion::new(2, 37, 0));
        assert!(GitVersion::new(2, 9, 9) < GitVersion::new(2, 10, 0));
        assert_eq!(GitVersion::new(2, 45, 1).to_string(), "2.45.1");
    }
}
