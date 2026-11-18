//! `FACET_BENCH_SCROLL=1`: measure the history list while it scrolls continuously.
//! `FACET_BENCH_SCROLL=diff:<path>`: the same for the working-copy diff of `<path>` (the 10 MB
//! file of the benchmark repository).
//! `FACET_BENCH_SEARCH=<kind>:<text>`: once the history has loaded, search it (timings go to
//! `FACET_PERF_LOG`; memory is read from outside). Both together scroll the results.
//!
//! Once the active repository's history has loaded, every frame moves the commit list by a
//! fixed distance and records eframe's CPU time for the previous frame (UI code,
//! tessellation and painting; not the wait for vsync). After a warm-up the summary goes to
//! `FACET_PERF_LOG`. 120 fps means every frame must fit in 8.33 ms.

use std::time::Instant;

use facet_git::SearchKind;

/// `FACET_BENCH_SEARCH`: `message:`, `author:`, `file:`, `code:` or `regex:` and the text.
pub fn search_from_env() -> Option<(SearchKind, String)> {
    parse_search(&std::env::var("FACET_BENCH_SEARCH").ok()?)
}

fn parse_search(value: &str) -> Option<(SearchKind, String)> {
    let (kind, text) = value.split_once(':')?;
    let kind = match kind {
        "message" => SearchKind::Message,
        "author" => SearchKind::Author,
        "file" => SearchKind::Path,
        "code" => SearchKind::Pickaxe,
        "regex" => SearchKind::DiffRegex,
        _ => return None,
    };
    Some((kind, text.to_owned())).filter(|(_, text)| !text.is_empty())
}

/// Frames skipped before measuring (shader compilation, glyph atlas growth, first layout).
const WARMUP: usize = 120;
/// Frames measured.
const FRAMES: usize = 1500;
/// Pixels per frame: a fast fling (≈ 4 400 px/s at 120 fps), not a multiple of the row height.
const STEP: f32 = 37.0;
pub const BUDGET_MS: f32 = 1000.0 / 120.0;

/// What the benchmark scrolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    History,
    /// The diff of this working-copy path (repository-relative, forward slashes).
    Diff(String),
}

fn parse_target(value: &str) -> Target {
    match value.strip_prefix("diff:") {
        Some(path) if !path.is_empty() => Target::Diff(path.replace('\\', "/")),
        _ => Target::History,
    }
}

pub struct ScrollBench {
    pub target: Target,
    /// The diff target's file has been selected.
    pub opened: bool,
    seen: usize,
    cpu_ms: Vec<f32>,
    interval_ms: Vec<f32>,
    last: Option<Instant>,
    offset: f32,
}

impl ScrollBench {
    pub fn from_env() -> Option<Self> {
        let value = std::env::var("FACET_BENCH_SCROLL").ok()?;
        Some(Self {
            target: parse_target(&value),
            opened: false,
            seen: 0,
            cpu_ms: Vec::with_capacity(FRAMES),
            interval_ms: Vec::with_capacity(FRAMES),
            last: None,
            offset: 0.0,
        })
    }

    /// Record the previous frame and return where the list should be scrolled to now, or
    /// `None` once the run is complete (the report is then ready).
    pub fn step(&mut self, previous_cpu_secs: Option<f32>, content_height: f32) -> Option<f32> {
        let now = Instant::now();
        if self.seen > WARMUP {
            if let Some(cpu) = previous_cpu_secs {
                self.cpu_ms.push(cpu * 1000.0);
            }
            if let Some(last) = self.last {
                self.interval_ms
                    .push(now.duration_since(last).as_secs_f32() * 1000.0);
            }
        }
        self.last = Some(now);
        self.seen += 1;
        if self.cpu_ms.len() >= FRAMES {
            return None;
        }
        self.offset += STEP;
        let range = (content_height * 0.95).max(STEP);
        if self.offset > range {
            self.offset -= range;
        }
        Some(self.offset)
    }

    /// The summary; `subject` says what scrolled ("100000 commits, 1 lanes").
    pub fn report(&self, subject: &str) -> String {
        let over = self.cpu_ms.iter().filter(|ms| **ms > BUDGET_MS).count();
        format!(
            "scroll bench: {subject}, {} frames\n  \
             frame CPU ms  p50 {:.2}  p90 {:.2}  p99 {:.2}  max {:.2}  \
             over {BUDGET_MS:.2} ms: {over}\n  \
             frame interval ms (vsync-bound)  p50 {:.2}  p99 {:.2}\n",
            self.cpu_ms.len(),
            percentile(&self.cpu_ms, 50.0),
            percentile(&self.cpu_ms, 90.0),
            percentile(&self.cpu_ms, 99.0),
            percentile(&self.cpu_ms, 100.0),
            percentile(&self.interval_ms, 50.0),
            percentile(&self.interval_ms, 99.0),
        )
    }
}

/// Nearest-rank percentile (`p` in 0..=100); 0 for no samples.
pub fn percentile(samples: &[f32], p: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f32::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f32).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_searches_name_their_kind() {
        assert_eq!(
            parse_search("author:bench"),
            Some((SearchKind::Author, "bench".to_owned()))
        );
        assert_eq!(
            parse_search("code:a:b"),
            Some((SearchKind::Pickaxe, "a:b".to_owned()))
        );
        assert_eq!(parse_search("author:"), None);
        assert_eq!(parse_search("nope:x"), None);
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let samples: Vec<f32> = (1..=100).map(|n| n as f32).collect();
        assert_eq!(percentile(&samples, 50.0), 50.0);
        assert_eq!(percentile(&samples, 99.0), 99.0);
        assert_eq!(percentile(&samples, 100.0), 100.0);
        assert_eq!(percentile(&samples, 0.0), 1.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
        assert_eq!(percentile(&[3.0, 1.0, 2.0], 50.0), 2.0);
    }

    #[test]
    fn the_target_is_history_unless_a_diff_path_is_named() {
        assert_eq!(parse_target("1"), Target::History);
        assert_eq!(parse_target("diff:"), Target::History);
        assert_eq!(
            parse_target(r"diff:Docs\Generated/Big.txt"),
            Target::Diff("Docs/Generated/Big.txt".into())
        );
    }

    #[test]
    fn the_offset_advances_and_wraps() {
        let mut bench = ScrollBench {
            target: Target::History,
            opened: false,
            seen: 0,
            cpu_ms: Vec::new(),
            interval_ms: Vec::new(),
            last: None,
            offset: 0.0,
        };
        assert_eq!(bench.step(Some(0.001), 1000.0), Some(STEP));
        let mut offset = 0.0;
        for _ in 0..100 {
            offset = bench.step(Some(0.001), 1000.0).unwrap();
            assert!(offset <= 950.0);
        }
        assert!(offset > 0.0);
    }
}
