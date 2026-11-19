//! The commit history: the streamed log behind the graph, and one commit in detail.

use std::path::Path;

use crate::parse::{
    parse_changed_files, parse_commit_detail, parse_file_log, BlameCommit, BlameParser, BlameRange,
    ChangedFile, CommitDetail, FileLogEntry, LogCommit, LogParser, DETAIL_FORMAT, LOG_FORMAT,
};
use crate::{CancellationToken, Git, GitError};

/// Arguments of the history query: every branch, remote-tracking branch and tag plus `HEAD`
/// (stashes have their own list), children always before their parents.
pub const LOG_ARGS: &[&str] = &[
    "log",
    "--date-order",
    "-z",
    LOG_FORMAT,
    "--no-show-signature",
    "--exclude=refs/stash",
    "--all",
];

/// Stream the whole history in one git process (G5), handing commits over in pages: a first
/// small page as soon as possible, then larger ones. `on_page` runs on a tokio worker.
///
/// A repository without commits has an empty history, not an error (G13).
pub async fn stream_log(
    git: &Git,
    root: &Path,
    cancel: &CancellationToken,
    first_page: usize,
    page: usize,
    on_page: impl FnMut(Vec<LogCommit>) + Send,
) -> Result<usize, GitError> {
    stream_log_in(git, root, Scope::All, cancel, first_page, page, on_page).await
}

/// Which commits the history shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Every branch, remote-tracking branch and tag ([`LOG_ARGS`]).
    #[default]
    All,
    /// Only what `HEAD` can reach: the checked-out branch.
    Current,
}

/// [`LOG_ARGS`] for `scope`.
pub fn log_args(scope: Scope) -> Vec<String> {
    LOG_ARGS
        .iter()
        .map(|arg| match (*arg, scope) {
            ("--all", Scope::Current) => "HEAD".to_owned(),
            (arg, _) => arg.to_owned(),
        })
        .collect()
}

/// [`stream_log`] limited to `scope`.
pub async fn stream_log_in(
    git: &Git,
    root: &Path,
    scope: Scope,
    cancel: &CancellationToken,
    first_page: usize,
    page: usize,
    on_page: impl FnMut(Vec<LogCommit>) + Send,
) -> Result<usize, GitError> {
    let args = log_args(scope);
    stream(git, root, &args, cancel, first_page, page, on_page).await
}

/// What a history search looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchKind {
    /// The subject, the author's name or the start of the commit id (case-insensitive,
    /// plain text): the filter field above the history.
    Any,
    /// The commit message (`--grep`, case-insensitive, plain text).
    Message,
    /// The author's name or e-mail (`--author`, case-insensitive, plain text).
    Author,
    /// Commits that touched a path containing the text (case-insensitive).
    Path,
    /// Commits that change how often the text occurs (`-S`, the "pickaxe").
    Pickaxe,
    /// Commits whose diff has a line matching the regular expression (`-G`).
    DiffRegex,
}

impl SearchKind {
    pub const ALL: [Self; 6] = [
        Self::Any,
        Self::Message,
        Self::Author,
        Self::Path,
        Self::Pickaxe,
        Self::DiffRegex,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Message, author or hash",
            Self::Message => "Message",
            Self::Author => "Author",
            Self::Path => "File",
            Self::Pickaxe => "Code",
            Self::DiffRegex => "Diff Regex",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Self::Any => {
                "Commits whose subject or author contains the text, or whose id starts with it"
            }
            Self::Message => "Commits whose message contains the text",
            Self::Author => "Commits by an author whose name or e-mail contains the text",
            Self::Path => "Commits that changed a file whose path contains the text",
            Self::Pickaxe => "Commits that added or removed the text (git log -S)",
            Self::DiffRegex => {
                "Commits with a changed line matching the regular expression (git log -G)"
            }
        }
    }
}

/// Arguments of a history search over the same commits as the history list.
pub fn search_args(kind: SearchKind, text: &str) -> Vec<String> {
    search_args_in(Scope::All, kind, text)
}

/// [`search_args`] limited to `scope`. A [`SearchKind::Any`] search is the plain log,
/// filtered as it streams.
pub fn search_args_in(scope: Scope, kind: SearchKind, text: &str) -> Vec<String> {
    let mut args = log_args(scope);
    match kind {
        SearchKind::Any => {}
        SearchKind::Message => {
            args.extend([
                "-i".into(),
                "--fixed-strings".into(),
                format!("--grep={text}"),
            ]);
        }
        SearchKind::Author => {
            args.extend([
                "-i".into(),
                "--fixed-strings".into(),
                format!("--author={text}"),
            ]);
        }
        SearchKind::Pickaxe => args.push(format!("-S{text}")),
        SearchKind::DiffRegex => args.push(format!("-G{text}")),
        SearchKind::Path => {
            // `*` in an ordinary pathspec also matches `/`, so this finds the text anywhere
            // in the path; glob characters the user typed keep their meaning.
            args.extend(["--".into(), format!(":(icase)*{text}*")]);
        }
    }
    args
}

/// Stream the commits matching a search, newest first, in pages like [`stream_log`].
#[allow(clippy::too_many_arguments)]
pub async fn stream_search(
    git: &Git,
    root: &Path,
    kind: SearchKind,
    text: &str,
    cancel: &CancellationToken,
    first_page: usize,
    page: usize,
    on_page: impl FnMut(Vec<LogCommit>) + Send,
) -> Result<usize, GitError> {
    stream_search_in(
        git,
        root,
        Scope::All,
        kind,
        text,
        cancel,
        first_page,
        page,
        on_page,
    )
    .await
}

/// [`stream_search`] limited to `scope`.
#[allow(clippy::too_many_arguments)]
pub async fn stream_search_in(
    git: &Git,
    root: &Path,
    scope: Scope,
    kind: SearchKind,
    text: &str,
    cancel: &CancellationToken,
    first_page: usize,
    page: usize,
    mut on_page: impl FnMut(Vec<LogCommit>) + Send,
) -> Result<usize, GitError> {
    if text.trim().is_empty() {
        return Err(GitError::Unsupported(
            "Type something to search for.".to_owned(),
        ));
    }
    let args = search_args_in(scope, kind, text);
    if kind != SearchKind::Any {
        return stream(git, root, &args, cancel, first_page, page, on_page).await;
    }
    let needle = text.trim().to_lowercase();
    let mut found = 0usize;
    stream(git, root, &args, cancel, first_page, page, |commits| {
        let kept: Vec<LogCommit> = commits
            .into_iter()
            .filter(|commit| matches_any(commit, &needle))
            .collect();
        found += kept.len();
        on_page(kept);
    })
    .await?;
    Ok(found)
}

/// Whether a commit's subject or author contains `needle` (lowercase), or its id starts
/// with it.
fn matches_any(commit: &LogCommit, needle: &str) -> bool {
    commit.subject.to_lowercase().contains(needle)
        || commit.author.to_lowercase().contains(needle)
        || commit.oid.to_string().starts_with(needle)
}

async fn stream<S: AsRef<std::ffi::OsStr>>(
    git: &Git,
    root: &Path,
    args: &[S],
    cancel: &CancellationToken,
    first_page: usize,
    page: usize,
    mut on_page: impl FnMut(Vec<LogCommit>) + Send,
) -> Result<usize, GitError> {
    let mut parser = LogParser::default();
    let mut pending: Vec<LogCommit> = Vec::new();
    let mut total = 0usize;
    let mut failure: Option<GitError> = None;
    let mut target = first_page.max(1);
    let result = git
        .command(args)
        .current_dir(root)
        .background()
        .cancel_with(cancel)
        .stream(|chunk| {
            if failure.is_some() {
                return;
            }
            if let Err(error) = parser.feed(chunk, &mut pending) {
                failure = Some(error);
                return;
            }
            if pending.len() >= target {
                total += pending.len();
                on_page(std::mem::take(&mut pending));
                target = page.max(1);
            }
        })
        .await;
    if let Err(error) = result {
        // `git log` fails on a repository with no commits at all (no HEAD, no refs).
        if let GitError::Failed { .. } = &error {
            if total == 0 && pending.is_empty() && !has_commits(git, root).await {
                return Ok(0);
            }
        }
        return Err(error);
    }
    if let Some(error) = failure {
        return Err(error);
    }
    parser.finish()?;
    total += pending.len();
    on_page(pending);
    Ok(total)
}

async fn has_commits(git: &Git, root: &Path) -> bool {
    git.command(["rev-parse", "-q", "--verify", "HEAD^{commit}"])
        .current_dir(root)
        .background()
        .output()
        .await
        .is_ok_and(|output| output.success())
}

/// A commit's message and metadata plus the files it changed relative to `parent` (its
/// first parent, or `None` for a root commit). Both queries run at once (G5).
pub async fn commit_detail(
    git: &Git,
    root: &Path,
    oid: &str,
    parent: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> Result<(CommitDetail, Vec<ChangedFile>), GitError> {
    let mut log = git
        .command([
            "log",
            "-1",
            "-z",
            "--no-show-signature",
            DETAIL_FORMAT,
            oid,
            "--",
        ])
        .current_dir(root)
        .background();
    let mut tree = git
        .command([
            "diff-tree",
            "-r",
            "-z",
            "-M",
            "--raw",
            "--numstat",
            "--no-commit-id",
        ])
        .current_dir(root)
        .background();
    tree = match parent {
        Some(parent) => tree.args([parent, oid]),
        None => tree.args(["--root", oid]),
    };
    if let Some(cancel) = cancel {
        log = log.cancel_with(cancel);
        tree = tree.cancel_with(cancel);
    }
    let (log, tree) = tokio::join!(log.run(), tree.run());
    let detail = parse_commit_detail(&log?.stdout)?;
    let files = parse_changed_files(&tree?.stdout)?;
    Ok((detail, files))
}

/// One file's history, following renames: every commit that changed it, newest first, with
/// the file's path in that commit.
pub async fn file_history(
    git: &Git,
    root: &Path,
    path: &str,
    cancel: &CancellationToken,
) -> Result<Vec<FileLogEntry>, GitError> {
    let output = git
        .command([
            "--literal-pathspecs",
            "log",
            "--follow",
            "-z",
            "-M",
            "--name-status",
            "--no-show-signature",
            LOG_FORMAT,
            "HEAD",
            "--",
            path,
        ])
        .current_dir(root)
        .background()
        .cancel_with(cancel)
        .output()
        .await?;
    if !output.success() {
        if !has_commits(git, root).await {
            return Ok(Vec::new());
        }
        return Err(GitError::Failed {
            command: "git log --follow".to_owned(),
            code: output.status,
            stderr: output.stderr_text(),
        });
    }
    parse_file_log(&output.stdout, path)
}

/// Who last changed each line of `path` — in the working copy (`rev == None`; uncommitted
/// lines belong to the all-zero commit) or as of commit `rev`. Ranges stream to `on_ranges`
/// with the commits first seen since the previous call (commit indices are stable).
pub async fn blame(
    git: &Git,
    root: &Path,
    path: &str,
    rev: Option<&str>,
    cancel: &CancellationToken,
    mut on_ranges: impl FnMut(Vec<BlameCommit>, Vec<BlameRange>) + Send,
) -> Result<(), GitError> {
    // No `-M`: detecting lines moved within the file is pathological on large files (a
    // 10 MB file with 100k commits of history: over 7 minutes, against 4.7 s without; G23).
    // Whole-file renames are still followed.
    let mut command = git.command(["blame", "--incremental"]);
    if let Some(rev) = rev {
        command = command.arg(rev);
    }
    let mut parser = BlameParser::default();
    let mut sent = 0usize;
    let mut failure: Option<GitError> = None;
    let mut flush = |parser: &BlameParser, ranges: Vec<BlameRange>, sent: &mut usize| {
        let complete = parser.complete_commits();
        let commits = complete[*sent..].to_vec();
        *sent = complete.len();
        if !ranges.is_empty() || !commits.is_empty() {
            on_ranges(commits, ranges);
        }
    };
    command
        .args(["--", path])
        .current_dir(root)
        .background()
        .cancel_with(cancel)
        .stream(|chunk| {
            if failure.is_some() {
                return;
            }
            let mut ranges = Vec::new();
            if let Err(error) = parser.feed(chunk, &mut ranges) {
                failure = Some(error);
                return;
            }
            flush(&parser, ranges, &mut sent);
        })
        .await?;
    if let Some(error) = failure {
        return Err(error);
    }
    let mut ranges = Vec::new();
    parser.finish(&mut ranges)?;
    flush(&parser, ranges, &mut sent);
    Ok(())
}
