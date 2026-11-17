use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// How a logged command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Running,
    /// Process exited with this status code.
    Exited(i32),
    Cancelled,
    /// The process could not be started; the string is the OS error.
    SpawnFailed(String),
}

/// One executed git command, exactly as it was spawned.
#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub id: u64,
    /// The full command line, shell-quoted for display and copy/paste.
    pub command_line: String,
    pub cwd: Option<PathBuf>,
    pub started: SystemTime,
    pub duration: Option<Duration>,
    pub outcome: CommandOutcome,
    /// Trimmed stderr (capped), kept so failures can be inspected in the command log panel.
    pub stderr: String,
}

type Listener = Box<dyn Fn() + Send + Sync>;

/// Bounded, thread-safe log of every git command Facet runs.
///
/// The UI reads it with [`CommandLog::snapshot`]; [`CommandLog::revision`] changes whenever the
/// content changes, so a view can cheaply skip rebuilding.
pub struct CommandLog {
    capacity: usize,
    next_id: AtomicU64,
    revision: AtomicU64,
    records: Mutex<VecDeque<CommandRecord>>,
    listener: Mutex<Option<Listener>>,
}

const MAX_STDERR_BYTES: usize = 4096;

impl CommandLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            next_id: AtomicU64::new(1),
            revision: AtomicU64::new(0),
            records: Mutex::new(VecDeque::new()),
            listener: Mutex::new(None),
        }
    }

    /// Register a callback fired after every change (used to request a UI repaint).
    pub fn set_listener(&self, listener: impl Fn() + Send + Sync + 'static) {
        *lock(&self.listener) = Some(Box::new(listener));
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Vec<CommandRecord> {
        lock(&self.records).iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        lock(&self.records).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        lock(&self.records).clear();
        self.changed();
    }

    pub(crate) fn begin(&self, command_line: String, cwd: Option<PathBuf>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut records = lock(&self.records);
            if records.len() == self.capacity {
                records.pop_front();
            }
            records.push_back(CommandRecord {
                id,
                command_line,
                cwd,
                started: SystemTime::now(),
                duration: None,
                outcome: CommandOutcome::Running,
                stderr: String::new(),
            });
        }
        self.changed();
        id
    }

    pub(crate) fn finish(
        &self,
        id: u64,
        outcome: CommandOutcome,
        duration: Duration,
        stderr: &[u8],
    ) {
        {
            let mut records = lock(&self.records);
            // Newest records are at the back; a finishing command is almost always near it.
            if let Some(record) = records.iter_mut().rev().find(|r| r.id == id) {
                record.outcome = outcome;
                record.duration = Some(duration);
                let tail = &stderr[..stderr.len().min(MAX_STDERR_BYTES)];
                record.stderr = String::from_utf8_lossy(tail).trim().to_owned();
            }
        }
        self.changed();
    }

    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
        if let Some(listener) = lock(&self.listener).as_ref() {
            listener();
        }
    }
}

/// A poisoned log mutex only means another thread panicked mid-update; the data is still usable.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_newest_records() {
        let log = CommandLog::new(2);
        let a = log.begin("git a".into(), None);
        let _b = log.begin("git b".into(), None);
        let _c = log.begin("git c".into(), None);
        let lines: Vec<_> = log.snapshot().into_iter().map(|r| r.command_line).collect();
        assert_eq!(lines, ["git b", "git c"]);
        // Finishing an evicted record is a harmless no-op.
        log.finish(a, CommandOutcome::Exited(0), Duration::ZERO, b"");
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn finish_updates_record_and_revision() {
        let log = CommandLog::new(8);
        let id = log.begin("git status".into(), None);
        let before = log.revision();
        log.finish(
            id,
            CommandOutcome::Exited(128),
            Duration::from_millis(5),
            b"  fatal: nope\n",
        );
        assert!(log.revision() > before);
        let record = &log.snapshot()[0];
        assert_eq!(record.outcome, CommandOutcome::Exited(128));
        assert_eq!(record.stderr, "fatal: nope");
        assert_eq!(record.duration, Some(Duration::from_millis(5)));
    }

    #[test]
    fn listener_fires_on_change() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        let log = CommandLog::new(8);
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        log.set_listener(move || {
            hits2.fetch_add(1, Ordering::Relaxed);
        });
        let id = log.begin("git x".into(), None);
        log.finish(id, CommandOutcome::Cancelled, Duration::ZERO, b"");
        assert_eq!(hits.load(Ordering::Relaxed), 2);
    }
}
