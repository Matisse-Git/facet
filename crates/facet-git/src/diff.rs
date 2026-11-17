//! Fetching the diff of one file, and the blobs behind it (for image and binary views).

use std::path::{Path, PathBuf};

use crate::parse::{parse_file_diff, FileDiff};
use crate::{CancellationToken, Git, GitError};

/// Which two versions of a file to compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffSource {
    /// Index → working tree: what "Changes" shows for a tracked file.
    Unstaged,
    /// `HEAD` → index: what "Staged" shows.
    Staged,
    /// Nothing → working tree file: an untracked file.
    Untracked,
    /// "Ours" (stage 2) → working tree, for a file with merge conflicts.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffRequest {
    /// Path relative to the repository root, forward slashes.
    pub path: String,
    /// The path a staged rename came from.
    pub orig_path: Option<String>,
    pub source: DiffSource,
}

/// Options shared by every diff Facet shows. The user's diff drivers, text conversions,
/// colors and prefix settings are neutralized so the output is always the plain patch that
/// `git apply` accepts back.
const DIFF_ARGS: &[&str] = &[
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "--numstat",
    "-z",
    "-p",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// Diff one file. An unchanged file gives an empty [`FileDiff`].
pub async fn diff_file(
    git: &Git,
    root: &Path,
    request: &DiffRequest,
    cancel: Option<&CancellationToken>,
) -> Result<FileDiff, GitError> {
    let mut command = match request.source {
        DiffSource::Untracked => git
            .command(["diff", "--no-index"])
            .args(DIFF_ARGS)
            .args(["--", "/dev/null"])
            .arg(&request.path),
        source => {
            // `--literal-pathspecs`: a file called `[a]*.txt` is a file, not a pattern.
            let mut command = git.command(["--literal-pathspecs", "diff"]).args(DIFF_ARGS);
            command = match source {
                DiffSource::Staged => command.args(["--cached", "-M"]),
                DiffSource::Conflict => command.arg("--ours"),
                _ => command,
            };
            command = command.arg("--");
            if let Some(orig) = &request.orig_path {
                command = command.arg(orig);
            }
            command.arg(&request.path)
        }
    };
    command = command.current_dir(root).background();
    if let Some(cancel) = cancel {
        command = command.cancel_with(cancel);
    }

    let output = command.output().await?;
    // `--no-index` reports "files differ" with exit status 1.
    let ok = output.success() || (request.source == DiffSource::Untracked && output.status == 1);
    if !ok {
        return Err(GitError::Failed {
            command: "git diff".to_owned(),
            code: output.status,
            stderr: output.stderr_text(),
        });
    }
    parse_file_diff(output.stdout)
}

/// Read a blob's raw content by object id (no smudge filters: nothing is downloaded).
pub async fn read_blob(
    git: &Git,
    root: &Path,
    oid: &str,
    cancel: Option<&CancellationToken>,
) -> Result<Vec<u8>, GitError> {
    let mut command = git
        .command(["cat-file", "blob", oid])
        .current_dir(root)
        .background();
    if let Some(cancel) = cancel {
        command = command.cancel_with(cancel);
    }
    Ok(command.run().await?.stdout)
}

/// Sizes in bytes of the given objects, in order; `None` for an object git does not have.
pub async fn object_sizes(
    git: &Git,
    root: &Path,
    oids: &[&str],
) -> Result<Vec<Option<u64>>, GitError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let mut input = String::new();
    for oid in oids {
        input.push_str(oid);
        input.push('\n');
    }
    let output = git
        .command(["cat-file", "--batch-check=%(objectname) %(objectsize)"])
        .current_dir(root)
        .background()
        .stdin(input)
        .run()
        .await?;
    // One line per request: `<oid> <size>` or `<oid> missing`.
    let text = String::from_utf8_lossy(&output.stdout);
    let mut sizes: Vec<Option<u64>> = text
        .lines()
        .map(|line| line.rsplit(' ').next().and_then(|size| size.parse().ok()))
        .collect();
    sizes.resize(oids.len(), None);
    Ok(sizes)
}

const LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs.github.com/spec/";
/// Real LFS pointers are a few hundred bytes; anything longer is ordinary content.
const LFS_POINTER_MAX: usize = 1024;

/// What a Git LFS pointer file refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsPointer {
    /// SHA-256 of the real content, lowercase hex.
    pub oid: String,
    pub size: u64,
}

impl LfsPointer {
    /// Recognize the content of an LFS pointer file (the documented `key value` line format).
    pub fn parse(content: &[u8]) -> Option<Self> {
        if content.len() > LFS_POINTER_MAX || !content.starts_with(LFS_POINTER_PREFIX) {
            return None;
        }
        let text = std::str::from_utf8(content).ok()?;
        let mut oid = None;
        let mut size = None;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("oid sha256:") {
                oid = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("size ") {
                size = value.trim().parse().ok();
            }
        }
        let oid =
            oid.filter(|oid| oid.len() == 64 && oid.bytes().all(|b| b.is_ascii_hexdigit()))?;
        Some(Self { oid, size: size? })
    }

    /// Where the real content lives if it has already been downloaded into this repository.
    pub fn local_object(&self, git_dir: &Path) -> PathBuf {
        git_dir
            .join("lfs")
            .join("objects")
            .join(&self.oid[0..2])
            .join(&self.oid[2..4])
            .join(&self.oid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfs_pointers_are_recognized() {
        let oid = "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";
        let pointer =
            format!("version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize 12345\n");
        let parsed = LfsPointer::parse(pointer.as_bytes()).unwrap();
        assert_eq!(parsed.oid, oid);
        assert_eq!(parsed.size, 12345);
        assert_eq!(
            parsed.local_object(Path::new("r/.git")),
            Path::new("r/.git/lfs/objects/4d/7a").join(oid)
        );

        assert_eq!(LfsPointer::parse(b"just some text\n"), None);
        assert_eq!(
            LfsPointer::parse(
                b"version https://git-lfs.github.com/spec/v1\noid sha256:xyz\nsize 1\n"
            ),
            None
        );
        let huge = [pointer.as_bytes(), &vec![b'x'; 2000][..]].concat();
        assert_eq!(LfsPointer::parse(&huge), None);
    }
}
