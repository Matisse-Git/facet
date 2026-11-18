use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::cmdlog::{CommandLog, CommandOutcome};
use crate::error::GitError;
use crate::job::Job;

/// Arguments placed before the subcommand on every invocation.
///
/// * `--no-pager`: never wait on a pager.
/// * `-c color.ui=never`: a user with `color.ui=always` would otherwise get ANSI escapes in
///   piped output. `-c` only affects this one process; no config file is touched.
const FIXED_ARGS: &[&str] = &["--no-pager", "-c", "color.ui=never"];

const READ_CHUNK: usize = 64 * 1024;

/// Handle to the user's git executable. Cheap to clone; all clones share one [`CommandLog`].
#[derive(Clone)]
pub struct Git {
    inner: Arc<Inner>,
}

struct Inner {
    exe: PathBuf,
    log: Arc<CommandLog>,
    env: Vec<(OsString, OsString)>,
}

impl Git {
    /// `exe` is usually just `"git"` (resolved through `PATH`), or an absolute path from settings.
    pub fn new(exe: impl Into<PathBuf>, log: Arc<CommandLog>) -> Self {
        Self {
            inner: Arc::new(Inner {
                exe: exe.into(),
                log,
                env: Vec::new(),
            }),
        }
    }

    /// Returns a handle that sets an extra environment variable on every command.
    /// Used by tests to isolate git from the machine's global/system config.
    #[must_use]
    pub fn with_env(&self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let mut env = self.inner.env.clone();
        env.push((key.into(), value.into()));
        Self {
            inner: Arc::new(Inner {
                exe: self.inner.exe.clone(),
                log: self.inner.log.clone(),
                env,
            }),
        }
    }

    /// Returns a handle that runs a different git executable with the same log and
    /// environment (the git path chosen in settings).
    #[must_use]
    pub fn with_exe(&self, exe: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                exe: exe.into(),
                log: self.inner.log.clone(),
                env: self.inner.env.clone(),
            }),
        }
    }

    /// [`Git::with_env`] for several variables at once (the askpass environment).
    #[must_use]
    pub fn with_envs(&self, vars: &[(OsString, OsString)]) -> Self {
        let mut git = self.clone();
        for (key, value) in vars {
            git = git.with_env(key.clone(), value.clone());
        }
        git
    }

    pub fn log(&self) -> &Arc<CommandLog> {
        &self.inner.log
    }

    pub fn exe(&self) -> &Path {
        &self.inner.exe
    }

    /// Start building a git invocation: `git.command(["status", "--porcelain=v2", "-z"])`.
    pub fn command<I, S>(&self, args: I) -> GitCommand
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        GitCommand {
            git: self.clone(),
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            cwd: None,
            cancel: None,
            stdin: None,
            optional_locks: true,
            log_stdout: false,
            progress: None,
        }
    }
}

/// Receives git's progress lines (`--progress` output on stderr) as they are printed.
pub type ProgressFn = Arc<dyn Fn(&str) + Send + Sync>;

/// A finished git process.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// Git's own error text, trimmed, for display.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

/// Builder for one git invocation. Nothing is spawned until one of the `async` methods is awaited.
#[must_use = "a GitCommand does nothing until awaited"]
pub struct GitCommand {
    git: Git,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    cancel: Option<CancellationToken>,
    stdin: Option<Vec<u8>>,
    optional_locks: bool,
    log_stdout: bool,
    progress: Option<ProgressFn>,
}

impl GitCommand {
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Directory to run in (normally the repository's working tree).
    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Kill the process and return [`GitError::Cancelled`] when `token` is cancelled.
    pub fn cancel_with(mut self, token: &CancellationToken) -> Self {
        self.cancel = Some(token.clone());
        self
    }

    /// Bytes to feed to the process on stdin (stdin is closed otherwise).
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }

    /// Mark this as a background read (auto-refresh). Sets `GIT_OPTIONAL_LOCKS=0` so it never
    /// takes `index.lock` and can't collide with a git command the user runs in a terminal.
    pub fn background(mut self) -> Self {
        self.optional_locks = false;
        self
    }

    /// Record stdout as well as stderr in the command log. For commands whose stdout is meant
    /// for people (hook output during `commit`), never for commands whose stdout is data.
    pub fn log_stdout(mut self) -> Self {
        self.log_stdout = true;
        self
    }

    /// Hand each progress line git prints on stderr to `on_line` as it appears (git redraws
    /// a line with `\r`; every redraw is delivered). The text is shown to people as is,
    /// never parsed. The collected stderr keeps only the final state of each line.
    pub fn progress(mut self, on_line: ProgressFn) -> Self {
        self.progress = Some(on_line);
        self
    }

    /// The exact command line, quoted for display in the command log.
    pub fn command_line(&self) -> String {
        let mut line = String::from("git");
        for arg in FIXED_ARGS {
            line.push(' ');
            line.push_str(arg);
        }
        for arg in &self.args {
            line.push(' ');
            line.push_str(&quote(&arg.to_string_lossy()));
        }
        line
    }

    /// Run to completion and collect output. A non-zero exit status is **not** an error here;
    /// inspect [`GitOutput::status`]. Use [`GitCommand::run`] when failure should be an error.
    pub async fn output(self) -> Result<GitOutput, GitError> {
        self.execute(None::<fn(&[u8])>).await
    }

    /// Run to completion; a non-zero exit status becomes [`GitError::Failed`].
    pub async fn run(self) -> Result<GitOutput, GitError> {
        let command = self.command_line();
        let output = self.output().await?;
        check(command, output)
    }

    /// Run to completion, handing stdout to `on_chunk` as it arrives instead of buffering it.
    /// The returned [`GitOutput::stdout`] is empty. A non-zero exit becomes [`GitError::Failed`].
    pub async fn stream<F>(self, on_chunk: F) -> Result<GitOutput, GitError>
    where
        F: FnMut(&[u8]) + Send,
    {
        let command = self.command_line();
        let output = self.execute(Some(on_chunk)).await?;
        check(command, output)
    }

    async fn execute<F>(self, on_chunk: Option<F>) -> Result<GitOutput, GitError>
    where
        F: FnMut(&[u8]) + Send,
    {
        let command_line = self.command_line();
        let inner = &self.git.inner;

        let mut cmd = Command::new(&inner.exe);
        cmd.args(FIXED_ARGS)
            .args(&self.args)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Credential prompts must go through a helper/GUI, never a hidden console.
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true);
        if !self.optional_locks {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        for (key, value) in &inner.env {
            cmd.env(key, value);
        }
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let log = &inner.log;
        let id = log.begin(command_line, self.cwd.clone());
        let started = Instant::now();

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(source) => {
                log.finish(
                    id,
                    CommandOutcome::SpawnFailed(source.to_string()),
                    started.elapsed(),
                    b"",
                );
                return Err(GitError::Spawn {
                    exe: inner.exe.display().to_string(),
                    source,
                });
            }
        };

        // G6: the job takes git's children with it on cancel; see `job.rs`. If the job can't
        // be set up (Facet itself runs in a job that forbids nesting), cancelling still
        // kills git.exe as before.
        let job = Job::new().ok().filter(|job| {
            #[cfg(windows)]
            {
                child
                    .raw_handle()
                    .is_some_and(|handle| job.assign(handle).is_ok())
            }
            #[cfg(not(windows))]
            {
                let _ = job;
                false
            }
        });
        let kill = |job: &Option<Job>| {
            if let Some(job) = job {
                job.terminate();
            }
        };

        let stdin_pipe = child.stdin.take();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let stdin_data = self.stdin;
        let progress = self.progress;

        let io = async {
            let feed = async {
                if let (Some(mut pipe), Some(data)) = (stdin_pipe, stdin_data) {
                    // Git may exit without reading all of stdin; a broken pipe is not our error.
                    let _ = pipe.write_all(&data).await;
                    let _ = pipe.shutdown().await;
                }
            };
            let ((), stdout, stderr) = tokio::join!(
                feed,
                read_stdout(stdout_pipe, on_chunk),
                read_stderr(stderr_pipe, progress)
            );
            Ok::<_, std::io::Error>((stdout?, stderr?))
        };

        // A token that is never cancelled stands in when the caller did not provide one.
        let cancel = self.cancel.unwrap_or_default();

        let io_result = tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            result = io => Some(result),
        };

        let (stdout, stderr) = match io_result {
            Some(Ok(streams)) => streams,
            Some(Err(err)) => {
                kill(&job);
                let _ = child.kill().await;
                log.finish(id, CommandOutcome::Exited(-1), started.elapsed(), b"");
                return Err(GitError::Io(err));
            }
            None => {
                kill(&job);
                let _ = child.kill().await;
                log.finish(id, CommandOutcome::Cancelled, started.elapsed(), b"");
                return Err(GitError::Cancelled);
            }
        };

        let status = tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            status = child.wait() => Some(status),
        };
        let status = match status {
            Some(Ok(status)) => status.code().unwrap_or(-1),
            Some(Err(err)) => {
                log.finish(id, CommandOutcome::Exited(-1), started.elapsed(), &stderr);
                return Err(GitError::Io(err));
            }
            None => {
                kill(&job);
                let _ = child.kill().await;
                log.finish(id, CommandOutcome::Cancelled, started.elapsed(), &stderr);
                return Err(GitError::Cancelled);
            }
        };
        if let Some(job) = job {
            job.release();
        }

        if self.log_stdout && !stdout.is_empty() {
            let mut combined = stdout.clone();
            if !combined.ends_with(b"\n") {
                combined.push(b'\n');
            }
            combined.extend_from_slice(&stderr);
            log.finish(
                id,
                CommandOutcome::Exited(status),
                started.elapsed(),
                &combined,
            );
        } else {
            log.finish(
                id,
                CommandOutcome::Exited(status),
                started.elapsed(),
                &stderr,
            );
        }
        Ok(GitOutput {
            status,
            stdout,
            stderr,
        })
    }
}

fn check(command: String, output: GitOutput) -> Result<GitOutput, GitError> {
    if output.success() {
        Ok(output)
    } else {
        Err(GitError::Failed {
            command,
            code: output.status,
            stderr: output.stderr_text(),
        })
    }
}

async fn read_all(pipe: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut buf).await?;
    }
    Ok(buf)
}

/// Collect stderr; with a progress callback, also deliver each line (and each `\r` redraw
/// of it) as it arrives. The collected text keeps only the final state of each line.
async fn read_stderr(
    pipe: Option<impl AsyncRead + Unpin>,
    progress: Option<ProgressFn>,
) -> std::io::Result<Vec<u8>> {
    let Some(progress) = progress else {
        return read_all(pipe).await;
    };
    let Some(mut pipe) = pipe else {
        return Ok(Vec::new());
    };
    let mut lines = ProgressLines::default();
    let mut chunk = vec![0u8; 4096];
    loop {
        let n = pipe.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        lines.feed(&chunk[..n], &*progress);
    }
    Ok(lines.finish(&*progress))
}

/// Splits stderr into lines and `\r` redraws (see [`read_stderr`]).
#[derive(Default)]
struct ProgressLines {
    collected: Vec<u8>,
    line: Vec<u8>,
    /// The last `\r`-terminated redraw of the current line.
    redraw: Vec<u8>,
}

impl ProgressLines {
    fn feed(&mut self, bytes: &[u8], progress: &(dyn Fn(&str) + Send + Sync)) {
        for &byte in bytes {
            match byte {
                b'\r' => {
                    if !self.line.is_empty() {
                        progress(String::from_utf8_lossy(&self.line).trim_end());
                        self.redraw = std::mem::take(&mut self.line);
                    }
                }
                b'\n' => {
                    if !self.line.is_empty() {
                        progress(String::from_utf8_lossy(&self.line).trim_end());
                        self.collected.extend_from_slice(&self.line);
                    } else {
                        self.collected.extend_from_slice(&self.redraw);
                    }
                    self.collected.push(b'\n');
                    self.line.clear();
                    self.redraw.clear();
                }
                other => self.line.push(other),
            }
        }
    }

    fn finish(mut self, progress: &(dyn Fn(&str) + Send + Sync)) -> Vec<u8> {
        if !self.line.is_empty() {
            progress(String::from_utf8_lossy(&self.line).trim_end());
            self.collected.extend_from_slice(&self.line);
        } else {
            self.collected.extend_from_slice(&self.redraw);
        }
        self.collected
    }
}

async fn read_stdout<F>(
    pipe: Option<impl AsyncRead + Unpin>,
    on_chunk: Option<F>,
) -> std::io::Result<Vec<u8>>
where
    F: FnMut(&[u8]),
{
    let Some(mut on_chunk) = on_chunk else {
        return read_all(pipe).await;
    };
    if let Some(mut pipe) = pipe {
        let mut chunk = vec![0u8; READ_CHUNK];
        loop {
            let n = pipe.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            on_chunk(&chunk[..n]);
        }
    }
    Ok(Vec::new())
}

/// Quote one argument for display so the logged line can be pasted into a shell.
fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=+.,:/\\@%^~{}[]".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("\"{}\"", arg.replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_for_display() {
        assert_eq!(quote("status"), "status");
        assert_eq!(quote("--porcelain=v2"), "--porcelain=v2");
        assert_eq!(quote("--format=%H%x00%s"), "--format=%H%x00%s");
        assert_eq!(quote("HEAD^{commit}"), "HEAD^{commit}");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("my file.txt"), "\"my file.txt\"");
        assert_eq!(quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn progress_redraws_are_delivered_and_collapsed() {
        let seen = std::sync::Mutex::new(Vec::<String>::new());
        let record = |line: &str| seen.lock().unwrap().push(line.to_owned());
        let mut lines = ProgressLines::default();
        lines.feed(b"remote: Counting objects: 1\r", &record);
        lines.feed(b"remote: Counting objects: 2\rremote: Counting", &record);
        lines.feed(b" objects: 3, done.\nFrom x\n", &record);
        lines.feed(b"Receiving: 50%\r", &record);
        let collected = lines.finish(&record);
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "remote: Counting objects: 1",
                "remote: Counting objects: 2",
                "remote: Counting objects: 3, done.",
                "From x",
                "Receiving: 50%",
            ]
        );
        assert_eq!(
            String::from_utf8(collected).unwrap(),
            "remote: Counting objects: 3, done.\nFrom x\nReceiving: 50%"
        );
    }

    #[test]
    fn command_line_includes_fixed_args() {
        let git = Git::new("git", Arc::new(CommandLog::new(4)));
        let line = git
            .command(["status", "--porcelain=v2", "-z"])
            .arg("--")
            .arg("a b")
            .command_line();
        assert_eq!(
            line,
            "git --no-pager -c color.ui=never status --porcelain=v2 -z -- \"a b\""
        );
    }
}
