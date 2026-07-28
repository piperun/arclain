//! Per-plugin file logger with rate limiting and size cap.
//!
//! Plugins that misbehave (infinite loops, debug spam) get throttled
//! at the host so they can't fill the disk or drown out arclain's
//! own logs. Drop policy is silent + periodic summary written to
//! arclain.log every `SUMMARY_INTERVAL`.

use crate::types::PluginId;
use parking_lot::Mutex;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

mod retention;

pub(crate) use retention::PluginLogRetentionPolicy;
use retention::RetentionRoot;

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
/// Maximum guest-controlled text admitted as one plugin log entry.
pub(crate) const MAX_PLUGIN_LOG_ENTRY_BYTES: usize = 16 * 1024;
/// Minimum spacing between drop-summary lines emitted to arclain.log.
/// One line per ~10 s is enough to make a misbehaving plugin visible
/// without itself becoming the noise.
const SUMMARY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Per-plugin file logger. One instance per plugin, lives in
/// `HostFunctions`. Lazy-opens the log file on first write so plugins
/// that never log don't create empty files.
pub struct PluginLogger {
    plugin_id: PluginId,
    log_dir: Option<PathBuf>,
    retention_root: Option<RetentionRoot>,
    coordination_lock: Option<File>,
    retention_policy: PluginLogRetentionPolicy,
    byte_cap: u64,
    state: Mutex<LoggerState>,
    bucket: TokenBucket,
    #[cfg(test)]
    retention_scan_count: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    io_error_report_count: std::sync::atomic::AtomicU64,
}

struct LoggerState {
    /// Lazily-opened append handle for today's file.
    file: Option<File>,
    /// Date stamp the current `file` was opened for. When this rolls
    /// (midnight crossed), reopen with the new date.
    file_date: Option<String>,
    /// Last observed current-file length. The authoritative cap check rereads
    /// metadata while holding the cross-instance coordination lock.
    bytes_written: u64,
    /// Lines dropped since the last summary flush (Task 1.4).
    dropped_since_summary: u64,
    /// Last time we emitted a "dropped N lines" summary to arclain log.
    last_summary: std::time::Instant,
    /// Last retention attempt. Failures are throttled too, avoiding a disk
    /// scan and warning storm on every admitted line.
    last_retention_check: std::time::Instant,
    /// Last generic I/O failure surfaced to host tracing. The guest can keep
    /// retrying, but cannot turn a broken log target into a tracing flood.
    last_io_error_report: Option<std::time::Instant>,
}

impl PluginLogger {
    pub fn new(plugin_id: &PluginId, log_dir: &Path) -> Self {
        Self::with_byte_cap(plugin_id, log_dir, DEFAULT_DAILY_BYTE_CAP)
    }

    pub fn with_byte_cap(plugin_id: &PluginId, log_dir: &Path, byte_cap: u64) -> Self {
        Self::with_retention_policy(
            plugin_id,
            log_dir,
            byte_cap,
            PluginLogRetentionPolicy::default(),
        )
    }

    pub(crate) fn with_retention_policy(
        plugin_id: &PluginId,
        log_dir: &Path,
        byte_cap: u64,
        retention_policy: PluginLogRetentionPolicy,
    ) -> Self {
        let retention_root = match RetentionRoot::prepare(log_dir) {
            Ok(root) => Some(root),
            Err(error) => {
                tracing::error!(
                    log_dir = %log_dir.display(),
                    %error,
                    "[plugin-logger] refusing unsafe plugin log directory"
                );
                None
            }
        };
        let coordination_lock = retention_root.as_ref().and_then(|root| {
            match root.open_coordination_lock() {
                Ok(lock) => Some(lock),
                Err(_) => {
                    tracing::error!(
                        plugin_id = %plugin_id,
                        "[plugin-logger] failed to open the coordination lock; plugin logging is disabled"
                    );
                    None
                }
            }
        });
        let logger = Self {
            plugin_id: plugin_id.clone(),
            log_dir: Some(log_dir.to_path_buf()),
            retention_root,
            coordination_lock,
            retention_policy,
            byte_cap,
            state: Mutex::new(LoggerState {
                file: None,
                file_date: None,
                bytes_written: 0,
                dropped_since_summary: 0,
                last_summary: std::time::Instant::now(),
                last_retention_check: std::time::Instant::now(),
                last_io_error_report: None,
            }),
            bucket: TokenBucket::new(DEFAULT_RATE_PER_SEC, DEFAULT_BURST),
            #[cfg(test)]
            retention_scan_count: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            io_error_report_count: std::sync::atomic::AtomicU64::new(0),
        };
        logger.run_retention(chrono::Local::now().date_naive());
        logger
    }

    pub(crate) fn deferred(plugin_id: &PluginId) -> Self {
        Self {
            plugin_id: plugin_id.clone(),
            log_dir: None,
            retention_root: None,
            coordination_lock: None,
            retention_policy: PluginLogRetentionPolicy::default(),
            byte_cap: DEFAULT_DAILY_BYTE_CAP,
            state: Mutex::new(LoggerState {
                file: None,
                file_date: None,
                bytes_written: 0,
                dropped_since_summary: 0,
                last_summary: std::time::Instant::now(),
                last_retention_check: std::time::Instant::now(),
                last_io_error_report: None,
            }),
            bucket: TokenBucket::new(DEFAULT_RATE_PER_SEC, DEFAULT_BURST),
            #[cfg(test)]
            retention_scan_count: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            io_error_report_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(crate) fn is_deferred(&self) -> bool {
        self.log_dir.is_none()
    }

    /// Write a single log line for this plugin. Returns `true` if the
    /// line was written, `false` if it was dropped (rate limit or size
    /// cap). Adds a trailing newline; callers do not need to.
    pub fn write(&self, message: &str) -> bool {
        self.write_parts(&[message])
    }

    /// Admit and write one line assembled from bounded borrowed parts.
    /// Length is checked before allocating the combined line, so hostcalls
    /// cannot force an additional unbounded copy while adding prefixes.
    pub fn write_parts(&self, parts: &[&str]) -> bool {
        let Some(_) = self.log_dir.as_ref() else {
            return true;
        };
        let Some(retention_root) = self.retention_root.as_ref() else {
            self.record_drop();
            return false;
        };
        let Some(coordination_lock) = self.coordination_lock.as_ref() else {
            self.record_drop();
            return false;
        };

        let Some(message_len) = parts
            .iter()
            .try_fold(0usize, |total, part| total.checked_add(part.len()))
        else {
            self.record_drop();
            return false;
        };
        if message_len > MAX_PLUGIN_LOG_ENTRY_BYTES {
            self.record_drop();
            return false;
        }

        if !self.bucket.try_take() {
            self.record_drop();
            return false;
        }

        let mut message = String::with_capacity(message_len);
        for part in parts {
            message.push_str(part);
        }

        let now = chrono::Local::now();
        let today_date = now.date_naive();
        let today = today_date.format("%Y-%m-%d").to_string();
        let timestamp = now.format("%Y-%m-%d %H:%M:%S%.3f");

        let mut s = self.state.lock();

        let rollover = s.file_date.as_deref().is_some_and(|date| date != today);
        let cleanup_due =
            s.last_retention_check.elapsed() >= self.retention_policy.cleanup_interval;
        if rollover {
            // Close the old handle before cleanup so Windows can evict it and
            // Unix cannot keep writing into an unlinked historical file.
            s.file = None;
            s.file_date = None;
            s.bytes_written = 0;
        }
        if rollover || cleanup_due {
            self.run_retention(today_date);
            s.last_retention_check = std::time::Instant::now();
        }

        // Reopen file on day rollover or first write.
        if s.file_date.as_deref() != Some(&today) {
            let file_name = daily_log_name(&self.plugin_id, today_date);
            match retention_root.open_daily_log(&file_name) {
                Ok(f) => {
                    let existing_len = f.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                    s.file = Some(f);
                    s.file_date = Some(today);
                    s.bytes_written = existing_len;
                }
                Err(_) => {
                    // Fall back to dropping silently — surfaced by the
                    // summary timer (Task 1.4).
                    self.maybe_report_io_failure(&mut s, "open");
                    s.dropped_since_summary += 1;
                    drop(s);
                    self.maybe_flush_summary();
                    return false;
                }
            }
        }

        let line = format!("{} {}\n", timestamp, message);
        let outcome = if let Some(file) = s.file.as_mut() {
            append_with_shared_cap(coordination_lock, file, line.as_bytes(), self.byte_cap)
        } else {
            Ok(AppendOutcome::Dropped)
        };
        let written = match outcome {
            Ok(AppendOutcome::Written(bytes_written)) => {
                s.bytes_written = bytes_written;
                true
            }
            Ok(AppendOutcome::Dropped) => {
                s.dropped_since_summary += 1;
                false
            }
            Err(_) => {
                self.maybe_report_io_failure(&mut s, "append");
                s.dropped_since_summary += 1;
                false
            }
        };
        drop(s);
        self.maybe_flush_summary();
        written
    }

    fn record_drop(&self) {
        self.state.lock().dropped_since_summary += 1;
        self.maybe_flush_summary();
    }

    fn run_retention(&self, today: chrono::NaiveDate) {
        let Some(root) = self.retention_root.as_ref() else {
            return;
        };
        #[cfg(test)]
        self.retention_scan_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Err(error) = root.cleanup(&self.plugin_id, today, self.retention_policy) {
            tracing::warn!(
                plugin_id = %self.plugin_id,
                %error,
                "[plugin-logger] retention cleanup skipped"
            );
        }
    }

    #[cfg(test)]
    fn retention_scan_count_for_test(&self) -> u64 {
        self.retention_scan_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    fn io_error_report_count_for_test(&self) -> u64 {
        self.io_error_report_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn maybe_report_io_failure(&self, state: &mut LoggerState, operation: &'static str) {
        if state
            .last_io_error_report
            .is_some_and(|last| last.elapsed() < SUMMARY_INTERVAL)
        {
            return;
        }
        state.last_io_error_report = Some(std::time::Instant::now());
        #[cfg(test)]
        self.io_error_report_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::error!(
            plugin_id = %self.plugin_id,
            operation,
            "[plugin-logger] plugin log I/O failed; further errors are throttled"
        );
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

fn daily_log_name(plugin_id: &PluginId, date: chrono::NaiveDate) -> String {
    format!(
        "{}-{}.log",
        plugin_id.identity_key().as_str(),
        date.format("%Y-%m-%d")
    )
}

enum AppendOutcome {
    Written(u64),
    Dropped,
}

/// Serialize the length check and append across logger instances and app
/// processes. The lock is nonblocking: contention drops a line through the
/// existing bounded summary path instead of stalling plugin execution.
fn append_with_shared_cap(
    coordination_lock: &File,
    file: &mut File,
    line: &[u8],
    byte_cap: u64,
) -> std::io::Result<AppendOutcome> {
    match std::fs::File::try_lock(coordination_lock) {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(AppendOutcome::Dropped),
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }

    let append = (|| {
        let current_len = file.metadata()?.len();
        let Some(next_len) = current_len.checked_add(line.len() as u64) else {
            return Ok(AppendOutcome::Dropped);
        };
        if next_len > byte_cap {
            return Ok(AppendOutcome::Dropped);
        }
        file.write_all(line)?;
        Ok(AppendOutcome::Written(next_len))
    })();
    let unlock = std::fs::File::unlock(coordination_lock);
    match (append, unlock) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
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
