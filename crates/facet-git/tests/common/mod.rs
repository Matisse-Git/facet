//! Shared helpers for integration tests: throwaway repositories driven by the real git CLI,
//! isolated from the machine's global and system git config.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use facet_git::{CommandLog, Git};
use tempfile::TempDir;

/// Fixed identity and clock so commits made by tests are byte-for-byte reproducible.
pub const AUTHOR: &str = "Facet Test";
pub const EMAIL: &str = "test@facet.invalid";
pub const DATE: &str = "1700000000 +0000";

pub struct TestRepo {
    /// Keeps the temp directory alive for the duration of the test.
    _temp: TempDir,
    pub path: PathBuf,
    pub git: Git,
    pub log: Arc<CommandLog>,
}

/// A `Git` that ignores the user's global/system config and uses a fixed identity.
pub fn isolated_git(scratch: &Path) -> (Git, Arc<CommandLog>) {
    let global = scratch.join("global.gitconfig");
    std::fs::write(&global, "").expect("write empty global config");
    let log = Arc::new(CommandLog::new(256));
    let git = Git::new("git", log.clone())
        .with_env("GIT_CONFIG_GLOBAL", global.as_os_str())
        .with_env("GIT_CONFIG_NOSYSTEM", "1")
        .with_env("GIT_AUTHOR_NAME", AUTHOR)
        .with_env("GIT_AUTHOR_EMAIL", EMAIL)
        .with_env("GIT_AUTHOR_DATE", DATE)
        .with_env("GIT_COMMITTER_NAME", AUTHOR)
        .with_env("GIT_COMMITTER_EMAIL", EMAIL)
        .with_env("GIT_COMMITTER_DATE", DATE);
    (git, log)
}

impl TestRepo {
    /// An empty temp directory that is *not* a repository.
    pub fn empty_dir() -> Self {
        let temp = tempfile::tempdir().expect("create temp dir");
        let scratch = temp.path().join("scratch");
        let path = temp.path().join("work");
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        std::fs::create_dir_all(&path).expect("create work dir");
        let (git, log) = isolated_git(&scratch);
        Self {
            _temp: temp,
            path,
            git,
            log,
        }
    }

    /// A fresh repository on branch `main` with no commits.
    pub async fn init() -> Self {
        let repo = Self::empty_dir();
        repo.git_ok(&["init", "-b", "main"]).await;
        repo
    }

    /// A fresh bare repository.
    pub async fn init_bare() -> Self {
        let repo = Self::empty_dir();
        repo.git_ok(&["init", "--bare", "-b", "main"]).await;
        repo
    }

    /// Run git in the repository and panic with its stderr if it fails.
    pub async fn git_ok(&self, args: &[&str]) -> Vec<u8> {
        let output = self
            .git
            .command(args)
            .current_dir(&self.path)
            .output()
            .await
            .expect("spawn git");
        assert!(
            output.success(),
            "git {args:?} failed: {}",
            output.stderr_text()
        );
        output.stdout
    }

    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, contents).expect("write file");
    }

    pub fn write_bytes(&self, relative: &str, contents: &[u8]) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, contents).expect("write file");
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path.join(relative)).expect("read file")
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.path.join(relative).exists()
    }

    pub fn remove(&self, relative: &str) {
        std::fs::remove_file(self.path.join(relative)).expect("remove file");
    }

    /// The content of `path` as it is staged in the index.
    pub async fn staged(&self, path: &str) -> String {
        String::from_utf8(self.git_ok(&["show", &format!(":{path}")]).await).expect("utf-8")
    }

    pub async fn status(&self) -> facet_git::parse::Status {
        facet_git::status(&self.git, &self.path, false, None)
            .await
            .expect("status")
    }

    pub async fn diff(
        &self,
        path: &str,
        source: facet_git::DiffSource,
    ) -> facet_git::parse::FileDiff {
        let request = facet_git::DiffRequest {
            path: path.to_owned(),
            orig_path: None,
            source,
        };
        facet_git::diff_file(&self.git, &self.path, &request, None)
            .await
            .expect("diff")
    }

    pub fn info(&self) -> facet_git::RepoInfo {
        facet_git::RepoInfo {
            git_dir: self.path.join(".git"),
            workdir: Some(self.path.clone()),
        }
    }

    /// Install a hook script (run by Git for Windows' bundled `sh`).
    pub fn hook(&self, name: &str, script: &str) {
        let path = self.path.join(".git").join("hooks").join(name);
        std::fs::create_dir_all(path.parent().expect("hooks dir")).expect("create hooks dir");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod hook");
        }
    }

    /// Write a file, stage everything and commit with `message`.
    pub async fn commit_file(&self, relative: &str, contents: &str, message: &str) {
        self.write(relative, contents);
        self.git_ok(&["add", "-A"]).await;
        self.git_ok(&["commit", "-m", message]).await;
    }
}

/// Compare two paths after resolving symlinks, 8.3 short names and separator differences.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    canon(a) == canon(b)
}
