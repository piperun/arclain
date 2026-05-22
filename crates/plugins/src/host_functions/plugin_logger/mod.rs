//! Per-plugin file logger with rate limiting and size cap.
//!
//! Plugins that misbehave (infinite loops, debug spam) get throttled
//! at the host so they can't fill the disk or drown out arclain's
//! own logs. Drop policy is silent + periodic summary written to
//! arclain.log every `SUMMARY_INTERVAL`.

use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(test)]
mod tests;

/// Sustained line rate per plugin. ~1k/sec is generous for legitimate
/// debug spam during fetches.
const DEFAULT_RATE_PER_SEC: f64 = 1000.0;
/// Burst capacity. ~10k tokens lets a plugin do a one-time bulk dump
/// (e.g. dumping a parsed HTML for debugging) without dropping.
const DEFAULT_BURST: u32 = 10_000;
/// Hard byte cap per plugin per day. Beyond this we drop further
/// writes for the rest of the day.
pub(crate) const DEFAULT_DAILY_BYTE_CAP: u64 = 50 * 1024 * 1024; // 50 MiB
/// Minimum spacing between drop-summary lines emitted to arclain.log.
/// One line per ~10 s is enough to make a misbehaving plugin visible
/// without itself becoming the noise.
const SUMMARY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Per-plugin file logger. One instance per plugin, lives in
/// `HostFunctions`. Lazy-opens the log file on first write so plugins
/// that never log don't create empty files.
pub struct PluginLogger {
    plugin_id: String,
    log_dir: PathBuf,
    byte_cap: u64,
    state: Mutex<LoggerState>,
    bucket: TokenBucket,
}

struct LoggerState {
    /// Lazily-opened append handle for today's file.
    file: Option<File>,
    /// Date stamp the current `file` was opened for. When this rolls
    /// (midnight crossed), reopen with the new date.
    file_date: Option<String>,
    /// Bytes written to the current file. Used for size-cap enforcement
    /// (Task 1.3).
    bytes_written: u64,
    /// Lines dropped since the last summary flush (Task 1.4).
    dropped_since_summary: u64,
    /// Last time we emitted a "dropped N lines" summary to arclain log.
    last_summary: std::time::Instant,
}

impl PluginLogger {
    pub fn new(plugin_id: &str, log_dir: &Path) -> Self {
        Self::with_byte_cap(plugin_id, log_dir, DEFAULT_DAILY_BYTE_CAP)
    }

    pub fn with_byte_cap(plugin_id: &str, log_dir: &Path, byte_cap: u64) -> Self {
        Self {
            plugin_id: plugin_id.to_string(),
            log_dir: log_dir.to_path_buf(),
            byte_cap,
            state: Mutex::new(LoggerState {
                file: None,
                file_date: None,
                bytes_written: 0,
                dropped_since_summary: 0,
                last_summary: std::time::Instant::now(),
            }),
            bucket: TokenBucket::new(DEFAULT_RATE_PER_SEC, DEFAULT_BURST),
        }
    }

    /// Write a single log line for this plugin. Returns `true` if the
    /// line was written, `false` if it was dropped (rate limit or size
    /// cap). Adds a trailing newline; callers do not need to.
    pub fn write(&self, message: &str) -> bool {
        if !self.bucket.try_take() {
            self.state.lock().dropped_since_summary += 1;
            return false;
        }

        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let timestamp = now.format("%Y-%m-%d %H:%M:%S%.3f");

        let mut s = self.state.lock();

        // Reopen file on day rollover or first write.
        if s.file_date.as_deref() != Some(&today) {
            let path = self
                .log_dir
                .join(format!("{}-{}.log", self.plugin_id, today));
            // Best-effort dir create; if it fails, write fails too.
            let _ = std::fs::create_dir_all(&self.log_dir);
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => {
                    s.file = Some(f);
                    s.file_date = Some(today);
                    s.bytes_written = std::fs::metadata(&path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                }
                Err(e) => {
                    // Fall back to dropping silently — surfaced by the
                    // summary timer (Task 1.4).
                    tracing::error!(
                        "[plugin-logger] failed to open {:?} for plugin {}: {}",
                        path,
                        self.plugin_id,
                        e
                    );
                    s.dropped_since_summary += 1;
                    return false;
                }
            }
        }

        let line = format!("{} {}\n", timestamp, message);
        if s.bytes_written + line.len() as u64 > self.byte_cap {
            s.dropped_since_summary += 1;
            drop(s);
            self.maybe_flush_summary();
            return false;
        }

        let written = if let Some(file) = s.file.as_mut() {
            match file.write_all(line.as_bytes()) {
                Ok(()) => {
                    s.bytes_written += line.len() as u64;
                    true
                }
                Err(_) => {
                    s.dropped_since_summary += 1;
                    false
                }
            }
        } else {
            s.dropped_since_summary += 1;
            false
        };
        drop(s);
        self.maybe_flush_summary();
        written
    }

    /// Emit a "dropped N lines" summary to arclain.log if more than
    /// SUMMARY_INTERVAL has elapsed since the last summary AND there
    /// are drops to report. Called from `write` (so the cost is
    /// amortized across legitimate writes); plugins that spam without
    /// any successful writes still get a summary on their next
    /// dropped attempt.
    pub fn maybe_flush_summary(&self) {
        let mut s = self.state.lock();
        if s.dropped_since_summary == 0 {
            return;
        }
        if s.last_summary.elapsed() < SUMMARY_INTERVAL {
            return;
        }
        let dropped = std::mem::replace(&mut s.dropped_since_summary, 0);
        s.last_summary = std::time::Instant::now();
        drop(s);
        tracing::warn!(
            "[plugin-logger] {} dropped {} log lines (rate or byte-cap)",
            self.plugin_id,
            dropped
        );
    }

    #[cfg(test)]
    pub fn flush_drop_summary_for_test(&self) {
        let mut s = self.state.lock();
        let dropped = std::mem::replace(&mut s.dropped_since_summary, 0);
        drop(s);
        tracing::warn!(
            "[plugin-logger] {} dropped {} log lines (rate or byte-cap)",
            self.plugin_id,
            dropped
        );
    }
}

/// Time source for [`TokenBucket`]. Production code uses
/// [`SystemClock`] (the default); tests can substitute a mock that
/// advances explicitly so token-bucket assertions don't depend on
/// real wall-clock scheduling.
///
/// Implementors must be `Send + Sync` because the bucket lives in
/// `PluginLogger` which is shared across WASM call threads.
pub(crate) trait Clock: Send + Sync {
    /// Time elapsed since some clock-internal reference point. Only
    /// the *difference* between two `now()` calls matters; absolute
    /// values are arbitrary.
    fn now(&self) -> std::time::Duration;
}

/// Real wall-clock implementation. Records `Instant::now()` at
/// construction and returns `elapsed()` thereafter, which is the
/// monotonic OS clock — immune to wall-clock jumps from NTP / DST.
pub(crate) struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}

/// Delegating impl so a test can pass `Arc<MockClock>` while keeping
/// a handle to call `advance()` on. Same pattern works for any
/// other shared clock wrapper.
impl<C: Clock + ?Sized> Clock for std::sync::Arc<C> {
    fn now(&self) -> std::time::Duration {
        (**self).now()
    }
}

/// Simple token bucket. `rate_per_sec` tokens are added per second up
/// to `capacity`. `try_take` consumes one token if available.
///
/// Generic over [`Clock`] with [`SystemClock`] as the default so
/// production call sites (`TokenBucket::new(...)`) get monotonic
/// real time while tests can construct with `with_clock` + an
/// `Arc<MockClock>` for deterministic timing.
pub(crate) struct TokenBucket<C: Clock = SystemClock> {
    rate_per_sec: f64,
    capacity: f64,
    clock: C,
    state: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last_refill: std::time::Duration,
}

impl TokenBucket<SystemClock> {
    pub(crate) fn new(rate_per_sec: f64, capacity: u32) -> Self {
        Self::with_clock(rate_per_sec, capacity, SystemClock::new())
    }
}

impl<C: Clock> TokenBucket<C> {
    pub(crate) fn with_clock(rate_per_sec: f64, capacity: u32, clock: C) -> Self {
        let initial = clock.now();
        Self {
            rate_per_sec,
            capacity: capacity as f64,
            clock,
            state: Mutex::new(BucketState {
                tokens: capacity as f64,
                last_refill: initial,
            }),
        }
    }

    pub(crate) fn try_take(&self) -> bool {
        let mut s = self.state.lock();
        let now = self.clock.now();
        // Safe subtraction: both values come from the same monotonic
        // clock, so `now >= s.last_refill` always holds.
        let elapsed = (now - s.last_refill).as_secs_f64();
        s.tokens = (s.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        s.last_refill = now;
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
