use super::{Clock, PluginLogger, TokenBucket};
use crate::types::PluginId;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Test-only clock with explicit `advance`. The token-bucket tests
/// pass an `Arc<MockClock>` to `TokenBucket::with_clock` and keep a
/// second `Arc` handle for advancing time mid-test, replacing the
/// wall-clock `Instant::now()` that made these tests flaky on slow
/// CI runners.
pub(crate) struct MockClock {
    elapsed: Mutex<Duration>,
}

impl MockClock {
    pub(crate) fn new() -> Self {
        Self {
            elapsed: Mutex::new(Duration::ZERO),
        }
    }

    pub(crate) fn advance(&self, by: Duration) {
        *self.elapsed.lock() += by;
    }
}

impl Clock for MockClock {
    fn now(&self) -> Duration {
        *self.elapsed.lock()
    }
}

fn temp_log_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "arclain_plugin_logger_test_{}_{}",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn plugin_id(value: &str) -> PluginId {
    PluginId::parse(value).unwrap()
}

#[test]
fn token_bucket_allows_burst_up_to_capacity() {
    let clock = Arc::new(MockClock::new());
    let bucket = TokenBucket::with_clock(1000.0, 5000, clock);
    // Bucket starts at capacity (5000). Time never advances, so no
    // refill — exactly 5000 takes succeed, the 5001st is refused.
    for _ in 0..5000 {
        assert!(bucket.try_take(), "expected token within capacity");
    }
    assert!(!bucket.try_take(), "expected refusal beyond capacity");
}

#[test]
fn token_bucket_refills_at_configured_rate() {
    let clock = Arc::new(MockClock::new());
    let bucket = TokenBucket::with_clock(1000.0, 100, clock.clone());
    // Drain.
    for _ in 0..100 {
        bucket.try_take();
    }
    assert!(!bucket.try_take(), "drained");

    // Advance 50 ms of mock time — at 1000 tokens/sec that's exactly
    // 50 tokens refilled. With a real Instant this would need a
    // `thread::sleep` plus a 40..=60 slack window; the mock clock
    // lets us assert the precise count.
    clock.advance(Duration::from_millis(50));
    let mut taken = 0;
    while bucket.try_take() {
        taken += 1;
        if taken > 100 {
            break;
        }
    }
    assert_eq!(taken, 50, "expected exactly 50 refilled tokens after 50 ms");
}

#[test]
fn logger_writes_to_per_plugin_dated_file() {
    let dir = temp_log_dir();
    let plugin_id = plugin_id("dlsite-metadata");
    let logger = PluginLogger::new(&plugin_id, &dir);

    logger.write("hello from plugin");

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = dir.join(format!("dlsite-metadata-{}.log", today));
    assert!(path.exists(), "log file at {:?} should exist", path);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("hello from plugin"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn logger_isolates_plugins_into_different_files() {
    let dir = temp_log_dir();
    let a_id = plugin_id("plugin-a");
    let b_id = plugin_id("plugin-b");
    let a = PluginLogger::new(&a_id, &dir);
    let b = PluginLogger::new(&b_id, &dir);

    a.write("from A");
    b.write("from B");

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let a_path = dir.join(format!("plugin-a-{}.log", today));
    let b_path = dir.join(format!("plugin-b-{}.log", today));

    let a_content = std::fs::read_to_string(&a_path).unwrap();
    let b_content = std::fs::read_to_string(&b_path).unwrap();
    assert!(a_content.contains("from A") && !a_content.contains("from B"));
    assert!(b_content.contains("from B") && !b_content.contains("from A"));

    let _ = std::fs::remove_dir_all(&dir);
}

use tracing_test::traced_test;

#[traced_test]
#[test]
fn logger_emits_summary_when_drops_accumulate() {
    let dir = temp_log_dir();
    let plugin_id = plugin_id("noisy");
    let logger = PluginLogger::with_byte_cap(&plugin_id, &dir, 100);

    // Drop a bunch of lines (most fail the byte cap after the first
    // few are written).
    for _ in 0..50 {
        logger
            .write("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
    }

    // Force a summary flush regardless of timer
    logger.flush_drop_summary_for_test();

    assert!(
        logs_contain("[plugin-logger] noisy dropped"),
        "expected drop summary to appear in tracing output"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn logger_drops_lines_after_byte_cap() {
    let dir = temp_log_dir();
    // 1 KiB cap so we hit it fast in test
    let plugin_id = plugin_id("capper");
    let logger = PluginLogger::with_byte_cap(&plugin_id, &dir, 1024);

    let line = "x".repeat(100); // ~120 bytes per line incl. timestamp
    let mut accepted = 0;
    let mut rejected = 0;
    for _ in 0..50 {
        if logger.write(&line) {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }

    assert!(accepted > 0, "at least some writes should succeed");
    assert!(rejected > 0, "byte cap must have triggered some rejections");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = dir.join(format!("capper-{}.log", today));
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len <= 1024 + 200, "file should be near cap, got {}", len);

    let _ = std::fs::remove_dir_all(&dir);
}
