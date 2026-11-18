//! External diff and merge tools (Beyond Compare, P4Merge, UnityYAMLMerge, …), chosen per
//! file extension. A tool is a command line with placeholders — `$LOCAL`, `$REMOTE`, `$BASE`,
//! `$MERGED` — like git's own `difftool` / `mergetool` configuration. Facet writes the
//! versions to temporary files, runs the tool in a job object (G6) and waits for it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cmdlog::CommandOutcome;
use crate::conflict::ConflictVersions;
use crate::job::Job;
use crate::{CancellationToken, Git, GitError};

/// Split a command line into program and arguments: whitespace separates, double quotes
/// group (`"C:\Program Files\Beyond Compare 4\BComp.exe" $LOCAL $REMOTE`). Backslashes are
/// literal, as in Windows paths.
pub fn split_command(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// Replace `$NAME` / `${NAME}` placeholders in each word.
pub fn expand(words: &[String], vars: &[(&str, &Path)]) -> Vec<String> {
    words
        .iter()
        .map(|word| {
            let mut word = word.clone();
            for (name, value) in vars {
                let value = value.to_string_lossy();
                word = word
                    .replace(&format!("${{{name}}}"), &value)
                    .replace(&format!("${name}"), &value);
            }
            word
        })
        .collect()
}

/// Whether a tool command line names the placeholders it needs.
pub fn missing_placeholders(line: &str, needed: &[&str]) -> Vec<String> {
    needed
        .iter()
        .filter(|name| {
            !line.contains(&format!("${name}")) && !line.contains(&format!("${{{name}}}"))
        })
        .map(|name| format!("${name}"))
        .collect()
}

/// Run `line` with its placeholders filled in, in `cwd`, and wait for it to exit. Recorded
/// in the command log like a git command. Returns the exit code.
pub async fn run_tool(
    git: &Git,
    line: &str,
    vars: &[(&str, &Path)],
    cwd: &Path,
    cancel: &CancellationToken,
) -> Result<i32, GitError> {
    let words = expand(&split_command(line), vars);
    let Some((program, args)) = words.split_first() else {
        return Err(GitError::Unsupported(
            "The external tool's command is empty.".to_owned(),
        ));
    };
    let shown = words
        .iter()
        .map(|word| {
            if word.contains(' ') {
                format!("\"{word}\"")
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let log = git.log();
    let id = log.begin(shown, Some(cwd.to_path_buf()));
    let started = Instant::now();
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            log.finish(
                id,
                CommandOutcome::SpawnFailed(source.to_string()),
                started.elapsed(),
                b"",
            );
            return Err(GitError::Unsupported(format!(
                "Couldn't start the external tool ({program}): {source}"
            )));
        }
    };
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
    let status = tokio::select! {
        biased;
        () = cancel.cancelled() => None,
        status = child.wait() => Some(status),
    };
    match status {
        Some(Ok(status)) => {
            if let Some(job) = job {
                job.release();
            }
            let code = status.code().unwrap_or(-1);
            log.finish(id, CommandOutcome::Exited(code), started.elapsed(), b"");
            Ok(code)
        }
        Some(Err(error)) => {
            log.finish(id, CommandOutcome::Exited(-1), started.elapsed(), b"");
            Err(GitError::Io(error))
        }
        None => {
            if let Some(job) = &job {
                job.terminate();
            }
            let _ = child.kill().await;
            log.finish(id, CommandOutcome::Cancelled, started.elapsed(), b"");
            Err(GitError::Cancelled)
        }
    }
}

/// A scratch folder for the versions handed to a tool: `<git dir>/facet-tools/<n>/`.
pub fn scratch_dir(git_dir: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    git_dir
        .join("facet-tools")
        .join(format!("{}-{n}", std::process::id()))
}

/// `name.EXT` → `name.LABEL.EXT`, so tools that pick a syntax by extension still do.
fn labelled(path: &str, label: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}.{label}.{ext}"),
        _ => format!("{name}.{label}"),
    }
}

/// Write the three versions of a conflicted file for a merge tool and return the
/// placeholder values (`$BASE`, `$LOCAL`, `$REMOTE`, `$MERGED`). A missing side is an empty
/// file, as git's mergetool does.
pub async fn merge_files(
    dir: &Path,
    root: &Path,
    path: &str,
    versions: &ConflictVersions,
) -> Result<Vec<(&'static str, PathBuf)>, GitError> {
    tokio::fs::create_dir_all(dir).await?;
    let mut vars = Vec::new();
    for (name, label, data) in [
        ("BASE", "BASE", &versions.base),
        ("LOCAL", "LOCAL", &versions.ours),
        ("REMOTE", "REMOTE", &versions.theirs),
    ] {
        let file = dir.join(labelled(path, label));
        tokio::fs::write(&file, data.as_deref().unwrap_or_default()).await?;
        vars.push((name, file));
    }
    vars.push(("MERGED", root.join(path)));
    Ok(vars)
}

/// Write the two sides of a diff for a diff tool: `$LOCAL` (old) and `$REMOTE` (new). A side
/// given as `Err(path)` is a file on disk used as it is (the working copy).
pub async fn diff_files(
    dir: &Path,
    path: &str,
    old: Option<&[u8]>,
    new: Result<&[u8], PathBuf>,
) -> Result<Vec<(&'static str, PathBuf)>, GitError> {
    tokio::fs::create_dir_all(dir).await?;
    let local = dir.join(labelled(path, "OLD"));
    tokio::fs::write(&local, old.unwrap_or_default()).await?;
    let remote = match new {
        Ok(data) => {
            let file = dir.join(labelled(path, "NEW"));
            tokio::fs::write(&file, data).await?;
            file
        }
        Err(file) => file,
    };
    Ok(vec![("LOCAL", local), ("REMOTE", remote)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_split_with_quotes() {
        assert_eq!(
            split_command(r#""C:\Program Files\BC\BComp.exe" $LOCAL  "$REMOTE" /title="a b""#),
            vec![
                r"C:\Program Files\BC\BComp.exe",
                "$LOCAL",
                "$REMOTE",
                "/title=a b"
            ]
        );
        assert_eq!(split_command(r#"tool "" x"#), vec!["tool", "", "x"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn placeholders_expand_in_place() {
        let words = split_command("merge -o${MERGED} $BASE $LOCAL");
        let base = PathBuf::from("b.txt");
        let merged = PathBuf::from("m.txt");
        let vars = [("BASE", base.as_path()), ("MERGED", merged.as_path())];
        assert_eq!(
            expand(&words, &vars),
            vec!["merge", "-om.txt", "b.txt", "$LOCAL"]
        );
        assert_eq!(
            missing_placeholders("x $LOCAL ${REMOTE}", &["LOCAL", "REMOTE", "MERGED"]),
            vec!["$MERGED"]
        );
    }

    #[test]
    fn labelled_names_keep_the_extension() {
        assert_eq!(labelled("Assets/Scene.unity", "BASE"), "Scene.BASE.unity");
        assert_eq!(labelled("Makefile", "LOCAL"), "Makefile.LOCAL");
        assert_eq!(labelled(".gitignore", "NEW"), ".gitignore.NEW");
    }
}
