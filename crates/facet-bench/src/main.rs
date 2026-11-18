//! Benchmark repository generator and measurements (SPEC §2.4).
//!
//! ```text
//! cargo run --release -p facet-bench -- generate [--files N] [--commits N] [--force] [DIR]
//! cargo run --release -p facet-bench -- graph [--commits N] [--branches N] [--force] [DIR]
//! cargo run --release -p facet-bench -- measure [--runs N] [--perf on|off] [DIR]
//! cargo run --release -p facet-bench -- history [--runs N] [DIR]
//! cargo run --release -p facet-bench -- package [--exe PATH] [--publisher "CN=..."] [DIR]
//! ```
//!
//! `graph` builds a lane-heavy history (topic branches, forks, merges) in `target/bench-graph`
//! for the commit graph; the main benchmark repository's history is linear.
//!
//! `generate` streams a synthetic history into `git fast-import` (100k files and 100k commits
//! by default: C++-like sources in a deep tree, Unreal-style binary assets, one 10 MB text
//! file), checks it out, and leaves the working tree dirty the way a working day does.
//! `measure` times what Facet runs, through the same `facet-git` functions the app calls;
//! `history` does the same for search, file history, the reflog and blame (M3).
//! `DIR` defaults to `target/bench-repo`. Nothing here touches any other repository or any
//! global git configuration.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use facet_git::parse::{parse_file_diff, parse_status};
use facet_git::{CommandLog, DiffRequest, DiffSource, Git};

mod package;

const DEFAULT_DIR: &str = "target/bench-repo";
const BIG_FILE: &str = "Docs/Generated/Big.txt";
/// Lines of the big text file; about 10 MB at ~72 bytes a line.
const BIG_LINES: usize = 140_000;
/// One binary asset per this many files.
const ASSET_EVERY: usize = 500;

type Result<T> = std::result::Result<T, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("generate") => generate(&args[1..]),
        Some("measure") => measure(&args[1..]),
        Some("graph") => generate_graph(&args[1..]),
        Some("history") => measure_history(&args[1..]),
        Some("package") => package::run(&args[1..]),
        _ => Err(
            "usage: facet-bench generate [--files N] [--commits N] [--force] [DIR]\n       \
             facet-bench graph [--commits N] [--branches N] [--force] [DIR]\n       \
             facet-bench measure [--runs N] [--perf on|off] [DIR]
                    facet-bench history [--runs N] [DIR]
                    facet-bench package [--exe PATH] [--publisher \"CN=...\"] [DIR]"
                .to_owned(),
        ),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

// ----- argument parsing ----------------------------------------------------------------------

struct Options {
    dir: PathBuf,
    numbers: Vec<(&'static str, usize)>,
    flags: Vec<&'static str>,
    texts: Vec<(&'static str, String)>,
}

impl Options {
    fn number(&self, name: &str) -> usize {
        self.numbers
            .iter()
            .find(|(key, _)| *key == name)
            .map_or(0, |(_, value)| *value)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.texts
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn parse_options(
    args: &[String],
    numbers: &[(&'static str, usize)],
    flags: &[&'static str],
    texts: &[&'static str],
) -> Result<Options> {
    let mut options = Options {
        dir: PathBuf::from(DEFAULT_DIR),
        numbers: numbers.to_vec(),
        flags: Vec::new(),
        texts: Vec::new(),
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some(flag) = flags.iter().find(|flag| arg == *flag) {
            options.flags.push(flag);
        } else if let Some((name, _)) = numbers.iter().find(|(name, _)| arg == name) {
            let value = args
                .next()
                .and_then(|value| value.replace('_', "").parse().ok())
                .ok_or_else(|| format!("{name} needs a number"))?;
            for entry in &mut options.numbers {
                if entry.0 == *name {
                    entry.1 = value;
                }
            }
        } else if let Some(name) = texts.iter().find(|name| arg == *name) {
            let value = args.next().ok_or_else(|| format!("{name} needs a value"))?;
            options.texts.push((name, value.clone()));
        } else if arg.starts_with("--") {
            return Err(format!("unknown option {arg}"));
        } else {
            options.dir = PathBuf::from(arg);
        }
    }
    Ok(options)
}

// ----- generate ------------------------------------------------------------------------------

/// xorshift64*: deterministic, so every generated repository is the same.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn source_path(index: usize) -> String {
    format!(
        "Source/Module{:02}/Private/Group{:02}/File{index:06}.cpp",
        index / 1000 % 100,
        index / 100 % 10
    )
}

fn asset_path(index: usize) -> String {
    format!(
        "Content/Assets/Set{:02}/Asset{index:06}.uasset",
        index / 5000
    )
}

/// Text of source file `index` at revision `revision`.
fn source_text(index: usize, revision: usize) -> Vec<u8> {
    let mut text = format!(
        "// File {index}, revision {revision}\n#include \"Module{:02}.h\"\n\n",
        index / 1000 % 100
    );
    for function in 0..6 {
        text.push_str(&format!(
            "int32 Function{function}_{index}(int32 Value)\n{{\n    \
             const int32 Scale = {};\n    return Value * Scale + {function};\n}}\n\n",
            (index + function * 7 + revision) % 97
        ));
    }
    text.into_bytes()
}

fn asset_bytes(index: usize, revision: usize) -> Vec<u8> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (index as u64) << 16 ^ revision as u64);
    let len = 16 * 1024 + rng.below(48 * 1024);
    let mut bytes = Vec::with_capacity(len + 8);
    bytes.extend_from_slice(b"\xC1\x83\x2A\x9E\0\0\0\0");
    while bytes.len() < len {
        bytes.extend_from_slice(&rng.next().to_le_bytes());
    }
    bytes
}

fn big_text(edit_every: Option<usize>) -> Vec<u8> {
    let mut text = String::with_capacity(BIG_LINES * 104);
    for line in 0..BIG_LINES {
        let edited = edit_every.is_some_and(|every| line % every == 0);
        text.push_str(&format!(
            "{line:06}: {} the quick brown fox jumps over the lazy dog, entry {}\n",
            if edited { "CHANGED" } else { "original" },
            line * 31 % 1000
        ));
    }
    text.into_bytes()
}

fn blob(out: &mut impl Write, path: &str, data: &[u8]) -> std::io::Result<()> {
    writeln!(out, "M 100644 inline {path}\ndata {}", data.len())?;
    out.write_all(data)?;
    out.write_all(b"\n")
}

fn commit_header(out: &mut impl Write, number: usize, message: &str) -> std::io::Result<()> {
    let time = 1_600_000_000 + number as u64 * 600;
    writeln!(
        out,
        "commit refs/heads/main\nmark :{number}\n\
         author Facet Bench <bench@example.invalid> {time} +0000\n\
         committer Facet Bench <bench@example.invalid> {time} +0000\ndata {}\n{message}",
        message.len()
    )
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {} failed: {status}", args.join(" ")))
    }
}

fn generate(args: &[String]) -> Result<()> {
    let options = parse_options(
        args,
        &[("--files", 100_000), ("--commits", 100_000)],
        &["--force"],
        &[],
    )?;
    let (files, commits, dir) = (
        options.number("--files").max(1),
        options.number("--commits").max(1),
        options.dir.clone(),
    );
    if dir.exists() {
        if !options.flag("--force") {
            return Err(format!(
                "{} already exists; pass --force to replace it",
                dir.display()
            ));
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|error| format!("removing {}: {error}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let started = Instant::now();

    git(&dir, &["init", "-q", "-b", "main"])?;
    for (key, value) in [
        ("core.autocrlf", "false"),
        ("gc.auto", "0"),
        ("user.name", "Facet Bench"),
        ("user.email", "bench@example.invalid"),
    ] {
        git(&dir, &["config", "--local", key, value])?;
    }

    // History.
    let mut import = Command::new("git")
        .args(["fast-import", "--quiet", "--done"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git fast-import: {error}"))?;
    {
        let stdin = import.stdin.take().ok_or("no stdin for fast-import")?;
        let mut out = BufWriter::with_capacity(1 << 20, stdin);
        let io = |error: std::io::Error| format!("writing to fast-import: {error}");
        commit_header(&mut out, 1, "Import the project").map_err(io)?;
        for index in 0..files {
            if index % ASSET_EVERY == 0 {
                blob(&mut out, &asset_path(index), &asset_bytes(index, 0)).map_err(io)?;
            } else {
                blob(&mut out, &source_path(index), &source_text(index, 0)).map_err(io)?;
            }
        }
        blob(&mut out, BIG_FILE, &big_text(None)).map_err(io)?;

        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        for number in 2..=commits {
            commit_header(&mut out, number, &format!("Change {number}")).map_err(io)?;
            for _ in 0..1 + rng.below(3) {
                let index = rng.below(files);
                if index % ASSET_EVERY == 0 {
                    blob(&mut out, &asset_path(index), &asset_bytes(index, number)).map_err(io)?;
                } else {
                    blob(&mut out, &source_path(index), &source_text(index, number)).map_err(io)?;
                }
            }
            if number % 10_000 == 0 {
                eprintln!("  {number} commits streamed ({:.0?})", started.elapsed());
            }
        }
        out.write_all(b"done\n").map_err(io)?;
        out.flush().map_err(io)?;
    }
    let status = import.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("git fast-import failed: {status}"));
    }
    let imported = started.elapsed();
    eprintln!("history imported in {imported:.1?}; checking out…");

    git(&dir, &["reset", "-q", "--hard", "main"])?;
    let checked_out = started.elapsed();

    // A working day: edits, deletions, new files, and one large rewrite.
    let mut rng = Rng(0xFEED_FACE_CAFE_BEEF);
    let write = |path: &str, data: &[u8]| -> Result<()> {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&full, data).map_err(|error| format!("{}: {error}", full.display()))
    };
    for _ in 0..100 {
        let index = rng.below(files);
        if index % ASSET_EVERY != 0 {
            let mut text = std::fs::read(dir.join(source_path(index))).unwrap_or_default();
            text.extend_from_slice(b"// edited in the working tree\n");
            write(&source_path(index), &text)?;
        }
    }
    for _ in 0..5 {
        let index = rng.below(files);
        let _ = std::fs::remove_file(dir.join(source_path(index)));
    }
    for index in 0..50 {
        write(
            &format!("Source/NewFeature/Private/New{index:03}.cpp"),
            &source_text(1_000_000 + index, 0),
        )?;
    }
    write(BIG_FILE, &big_text(Some(7)))?;

    println!(
        "generated {} in {:.1?}: {files} files, {commits} commits \
         (fast-import {imported:.1?}, checkout {:.1?})",
        dir.display(),
        started.elapsed(),
        checked_out - imported
    );
    Ok(())
}

// ----- graph ---------------------------------------------------------------------------------

const GRAPH_DIR: &str = "target/bench-graph";

/// One commit of the lane-heavy history: on `branch`, after `from`, optionally merging `merge`.
fn graph_commit(
    out: &mut impl Write,
    number: usize,
    branch: &str,
    from: Option<usize>,
    merge: Option<usize>,
    message: &str,
) -> std::io::Result<()> {
    let time = 1_600_000_000 + number as u64 * 600;
    writeln!(
        out,
        "commit refs/heads/{branch}\nmark :{number}\n\
         author Facet Bench <bench@example.invalid> {time} +0000\n\
         committer Facet Bench <bench@example.invalid> {time} +0000\ndata {}\n{message}",
        message.len()
    )?;
    if let Some(from) = from {
        writeln!(out, "from :{from}")?;
    }
    if let Some(merge) = merge {
        writeln!(out, "merge :{merge}")?;
    }
    Ok(())
}

/// A history for the commit graph rather than for status: `--branches` topic branches
/// committed to in interleaved order, regularly merged into `main` and restarted from it, so
/// the graph has many concurrent lanes, forks and merges. Every commit touches one tiny file.
fn generate_graph(args: &[String]) -> Result<()> {
    let mut options = parse_options(
        args,
        &[("--commits", 100_000), ("--branches", 24)],
        &["--force"],
        &[],
    )?;
    if options.dir == Path::new(DEFAULT_DIR) {
        options.dir = PathBuf::from(GRAPH_DIR);
    }
    let (commits, branches, dir) = (
        options.number("--commits").max(2),
        options.number("--branches").max(1),
        options.dir.clone(),
    );
    if dir.exists() {
        if !options.flag("--force") {
            return Err(format!(
                "{} already exists; pass --force to replace it",
                dir.display()
            ));
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|error| format!("removing {}: {error}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let started = Instant::now();
    git(&dir, &["init", "-q", "-b", "main"])?;
    for (key, value) in [("core.autocrlf", "false"), ("gc.auto", "0")] {
        git(&dir, &["config", "--local", key, value])?;
    }

    let mut import = Command::new("git")
        .args(["fast-import", "--quiet", "--done"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git fast-import: {error}"))?;
    let mut merges = 0;
    {
        let stdin = import.stdin.take().ok_or("no stdin for fast-import")?;
        let mut out = BufWriter::with_capacity(1 << 20, stdin);
        let io = |error: std::io::Error| format!("writing to fast-import: {error}");
        graph_commit(&mut out, 1, "main", None, None, "Start").map_err(io)?;
        blob(&mut out, "README.md", b"graph benchmark\n").map_err(io)?;

        let mut rng = Rng(0x0BAD_C0FF_EE15_600D);
        let mut main = 1;
        let mut tips: Vec<Option<usize>> = vec![None; branches];
        for number in 2..=commits {
            let roll = rng.below(100);
            let open: Vec<usize> = (0..branches).filter(|b| tips[*b].is_some()).collect();
            if roll < 6 && !open.is_empty() {
                // Merge a topic into main; it starts over from main later.
                let topic = open[rng.below(open.len())];
                let tip = tips[topic].take();
                graph_commit(
                    &mut out,
                    number,
                    "main",
                    Some(main),
                    tip,
                    &format!("Merge topic-{topic:02}"),
                )
                .map_err(io)?;
                main = number;
                merges += 1;
            } else if roll < 14 {
                graph_commit(
                    &mut out,
                    number,
                    "main",
                    Some(main),
                    None,
                    &format!("Fix {number}"),
                )
                .map_err(io)?;
                blob(&mut out, "main.txt", format!("{number}\n").as_bytes()).map_err(io)?;
                main = number;
            } else {
                let topic = rng.below(branches);
                let from = tips[topic].unwrap_or(main);
                graph_commit(
                    &mut out,
                    number,
                    &format!("topic-{topic:02}"),
                    Some(from),
                    None,
                    &format!("Work {number} on topic {topic}"),
                )
                .map_err(io)?;
                blob(
                    &mut out,
                    &format!("topics/{topic:02}.txt"),
                    format!("{number}\n").as_bytes(),
                )
                .map_err(io)?;
                tips[topic] = Some(number);
            }
        }
        out.write_all(b"done\n").map_err(io)?;
        out.flush().map_err(io)?;
    }
    let status = import.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("git fast-import failed: {status}"));
    }
    git(&dir, &["reset", "-q", "--hard", "main"])?;
    println!(
        "generated {} in {:.1?}: {commits} commits, {branches} topic branches, {merges} merges",
        dir.display(),
        started.elapsed()
    );
    Ok(())
}

// ----- measure -------------------------------------------------------------------------------

struct Timings(Vec<Duration>);

impl Timings {
    fn summary(&mut self) -> String {
        self.0.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let n = self.0.len();
        if n == 0 {
            return "no runs".to_owned();
        }
        format!(
            "median {:.0} ms, min {:.0} ms, max {:.0} ms ({n} runs)",
            ms(self.0[n / 2]),
            ms(self.0[0]),
            ms(self.0[n - 1])
        )
    }
}

fn measure(args: &[String]) -> Result<()> {
    let options = parse_options(args, &[("--runs", 10)], &[], &["--perf"])?;
    let runs = options.number("--runs").max(1);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(measure_async(&options.dir, runs, options.text("--perf")))
}

async fn measure_async(dir: &Path, runs: usize, perf: Option<&str>) -> Result<()> {
    let git = Git::new("git", Arc::new(CommandLog::new(64)));
    let text = |error: facet_git::GitError| error.to_string();
    let info = facet_git::discover(&git, dir).await.map_err(text)?;
    let root = info.root().to_owned();
    match perf {
        Some("on") => facet_git::set_perf_config(&git, &root, true)
            .await
            .map_err(text)?,
        Some("off") => facet_git::set_perf_config(&git, &root, false)
            .await
            .map_err(text)?,
        Some(other) => return Err(format!("--perf takes on or off, not {other}")),
        None => {}
    }
    let config = facet_git::perf_config(&git, &root).await.map_err(text)?;
    println!(
        "repository {} (fsmonitor {}, untrackedCache {})",
        root.display(),
        config.fsmonitor,
        config.untracked_cache
    );

    // First status after opening: refreshes the index's stat data (and, with fsmonitor,
    // starts the daemon), so it is reported on its own.
    let started = Instant::now();
    let status = facet_git::status(&git, &root, false, None)
        .await
        .map_err(text)?;
    println!(
        "status, first run:      {:.0} ms, {} entries",
        started.elapsed().as_secs_f64() * 1000.0,
        status.entries.len()
    );

    for (label, background) in [("foreground", false), ("background", true)] {
        let mut timings = Timings(Vec::new());
        for _ in 0..runs {
            let started = Instant::now();
            facet_git::status(&git, &root, background, None)
                .await
                .map_err(text)?;
            timings.0.push(started.elapsed());
        }
        println!("status, warm {label}: {}", timings.summary());
    }

    // Parsing alone, on the same output.
    let output = git
        .command([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ])
        .current_dir(&root)
        .background()
        .run()
        .await
        .map_err(text)?;
    let mut timings = Timings(Vec::new());
    for _ in 0..runs {
        let started = Instant::now();
        parse_status(&output.stdout).map_err(text)?;
        timings.0.push(started.elapsed());
    }
    println!(
        "status parse only:      {} for {} bytes",
        timings.summary(),
        output.stdout.len()
    );

    // The large diff, as the diff pane loads it.
    let request = DiffRequest {
        path: BIG_FILE.to_owned(),
        orig_path: None,
        source: DiffSource::Unstaged,
    };
    let mut timings = Timings(Vec::new());
    let mut size = (0, 0);
    for _ in 0..runs.min(5) {
        let started = Instant::now();
        let diff = facet_git::diff_file(&git, &root, &request, None)
            .await
            .map_err(text)?;
        timings.0.push(started.elapsed());
        size = (diff.data.len(), diff.lines.len());
    }
    println!(
        "10 MB diff load:        {} ({} bytes, {} lines)",
        timings.summary(),
        size.0,
        size.1
    );
    let output = git
        // The arguments `facet_git::diff_file` uses.
        .command([
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--numstat",
            "-z",
            "-p",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--",
            BIG_FILE,
        ])
        .current_dir(&root)
        .background()
        .run()
        .await
        .map_err(text)?;
    let mut timings = Timings(Vec::new());
    for _ in 0..runs.min(5) {
        let data = output.stdout.clone();
        let started = Instant::now();
        parse_file_diff(data).map_err(text)?;
        timings.0.push(started.elapsed());
    }
    println!("10 MB diff parse only:  {}", timings.summary());
    Ok(())
}

/// A file the benchmark history changes now and then, and the latest commit's file.
const HISTORY_FILE: &str = "Source/Module05/Private/Group05/File005567.cpp";

fn measure_history(args: &[String]) -> Result<()> {
    let options = parse_options(args, &[("--runs", 3)], &[], &[])?;
    let runs = options.number("--runs").max(1);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(measure_history_async(&options.dir, runs))
}

async fn measure_history_async(dir: &Path, runs: usize) -> Result<()> {
    use facet_git::{CancellationToken, SearchKind};

    let git = Git::new("git", Arc::new(CommandLog::new(64)));
    let text = |error: facet_git::GitError| error.to_string();
    let info = facet_git::discover(&git, dir).await.map_err(text)?;
    let root = info.root().to_owned();
    let cancel = CancellationToken::new();
    println!("repository {}", root.display());

    let mut timings = Timings(Vec::new());
    let mut count = 0;
    for _ in 0..runs {
        let started = Instant::now();
        count = facet_git::reflog(&git, &root, "HEAD", 5000)
            .await
            .map_err(text)?
            .len();
        timings.0.push(started.elapsed());
    }
    println!(
        "reflog (HEAD, 5000):     {} -> {count} entries",
        timings.summary()
    );

    // The pages the app's search streams: 200 first, then 5000 at a time.
    for (kind, query) in [
        (SearchKind::Message, "Change 9999"),
        (SearchKind::Author, "bench"),
        (SearchKind::Path, "File005567"),
        (SearchKind::Pickaxe, "CHANGED"),
    ] {
        let mut first = Timings(Vec::new());
        let mut all = Timings(Vec::new());
        let mut count = 0;
        for _ in 0..runs {
            let started = Instant::now();
            let mut first_page = None;
            count = facet_git::stream_search(&git, &root, kind, query, &cancel, 200, 5000, |_| {
                first_page.get_or_insert_with(|| started.elapsed());
            })
            .await
            .map_err(text)?;
            all.0.push(started.elapsed());
            first
                .0
                .push(first_page.unwrap_or_else(|| started.elapsed()));
        }
        println!(
            "search {:<8} {:<12} first page {}
                          all {} -> {count} commits",
            kind.label(),
            format!("\"{query}\""),
            first.summary(),
            all.summary()
        );
    }

    let mut timings = Timings(Vec::new());
    let mut count = 0;
    for _ in 0..runs {
        let started = Instant::now();
        count = facet_git::file_history(&git, &root, HISTORY_FILE, &cancel)
            .await
            .map_err(text)?
            .len();
        timings.0.push(started.elapsed());
    }
    println!(
        "file history (--follow): {} -> {count} commits",
        timings.summary()
    );

    for path in [HISTORY_FILE, BIG_FILE] {
        let mut first = Timings(Vec::new());
        let mut all = Timings(Vec::new());
        let mut lines = 0;
        for _ in 0..runs {
            let started = Instant::now();
            let mut first_range = None;
            let mut covered = 0u64;
            facet_git::blame(&git, &root, path, None, &cancel, |_, ranges| {
                first_range.get_or_insert_with(|| started.elapsed());
                covered += ranges
                    .iter()
                    .map(|range| u64::from(range.count))
                    .sum::<u64>();
            })
            .await
            .map_err(text)?;
            all.0.push(started.elapsed());
            first
                .0
                .push(first_range.unwrap_or_else(|| started.elapsed()));
            lines = covered;
        }
        println!(
            "blame {path}:
                          first lines {}
                          all {} -> {lines} lines",
            first.summary(),
            all.summary()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_and_paths_are_spread_out() {
        assert_eq!(source_text(5, 1), source_text(5, 1));
        assert_ne!(source_text(5, 1), source_text(5, 2));
        assert_eq!(asset_bytes(500, 3), asset_bytes(500, 3));
        assert!(asset_bytes(500, 3).len() >= 16 * 1024);
        assert_eq!(
            source_path(12_345),
            "Source/Module12/Private/Group03/File012345.cpp"
        );
        let big = big_text(Some(7));
        assert!(big.len() > 9_000_000 && big.len() < 12_000_000);
    }

    #[test]
    fn options_parse_numbers_flags_and_the_directory() {
        let args: Vec<String> = ["--files", "1_000", "--force", "out"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        let options = parse_options(
            &args,
            &[("--files", 5), ("--commits", 7)],
            &["--force"],
            &[],
        )
        .unwrap();
        assert_eq!(options.number("--files"), 1000);
        assert_eq!(options.number("--commits"), 7);
        assert!(options.flag("--force"));
        assert_eq!(options.dir, PathBuf::from("out"));
        assert!(parse_options(&args[..1], &[("--files", 5)], &[], &[]).is_err());
    }
}
