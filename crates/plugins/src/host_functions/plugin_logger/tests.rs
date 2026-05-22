use super::{PluginLogger, TokenBucket};
use std::path::PathBuf;
use std::time::Duration;

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

#[test]
fn token_bucket_allows_burst_up_to_capacity() {
    // Rate 0.0 = no time-based refill. The 5000-iteration burst on a
    // slow CI runner takes a few ms and would otherwise refill several
    // tokens during the loop (1000/sec × 5 ms = 5 tokens), letting
    // take #5001 sneak through and breaking the assertion. The test
    // is specifically about *burst capacity*, not refill rate — refill
    // is exercised by `token_bucket_refills_at_configured_rate` below —
    // so setting rate to 0.0 isolates the property under test from
    // wall-clock scheduling.
    let bucket = TokenBucket::new(0.0, 5000);
    for _ in 0..5000 {
        assert!(bucket.try_take(), "expected token within capacity");
    }
    assert!(!bucket.try_take(), "expected refusal beyond capacity");
}

#[test]
fn token_bucket_refills_at_configured_rate() {
    let bucket = TokenBucket::new(1000.0, 100); // 1000/sec, cap 100
    // Drain it
    for _ in 0..100 {
        bucket.try_take();
    }
    assert!(!bucket.try_take(), "drained");
    // Wait 50 ms — should refill ~50 tokens
    std::thread::sleep(Duration::from_millis(50));
    let mut taken = 0;
    while bucket.try_take() {
        taken += 1;
        if taken > 100 {
            break;
        }
    }
    // Allow some slack for scheduling jitter
    assert!(
        (40..=60).contains(&taken),
        "expected ~50 refilled tokens after 50 ms, got {}",
        taken
    );
}

#[test]
fn logger_writes_to_per_plugin_dated_file() {
    let dir = temp_log_dir();
    let logger = PluginLogger::new("dlsite-metadata", &dir);

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
    let a = PluginLogger::new("plugin-a", &dir);
    let b = PluginLogger::new("plugin-b", &dir);

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
    let logger = PluginLogger::with_byte_cap("noisy", &dir, 100);

    // Drop a bunch of lines (most fail the byte cap after the first
    // few are written).
    for _ in 0..50 {
        logger.write("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
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
    let logger = PluginLogger::with_byte_cap("capper", &dir, 1024);

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
