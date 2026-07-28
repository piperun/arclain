use super::{
    daily_log_name, Clock, PluginLogRetentionPolicy, PluginLogger, RetentionRoot, TokenBucket,
};
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

fn dated_log_path(dir: &std::path::Path, plugin: &str, days_ago: i64) -> PathBuf {
    let date = chrono::Local::now().date_naive() - chrono::Duration::days(days_ago);
    dir.join(format!("{plugin}-{}.log", date.format("%Y-%m-%d")))
}

fn retention_policy(
    max_bytes_per_plugin: u64,
    max_total_bytes: u64,
    max_age_days: u64,
    max_files_per_plugin: usize,
) -> PluginLogRetentionPolicy {
    PluginLogRetentionPolicy {
        max_files_per_plugin,
        max_age: Duration::from_secs(max_age_days * 24 * 60 * 60),
        max_bytes_per_plugin,
        max_total_bytes,
        cleanup_interval: Duration::from_secs(60 * 60),
        max_scan_entries: 128,
    }
}

#[cfg(unix)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) {
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("junction creation command should start");
    assert!(status.success(), "failed to create logger test junction");
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

#[test]
fn retention_removes_files_older_than_the_policy_but_not_sibling_logs() {
    let dir = temp_log_dir();
    let old = dated_log_path(&dir, "mixed-plugin", 31);
    let boundary = dated_log_path(&dir, "MIXED-PLUGIN", 30);
    let sibling = dated_log_path(&dir, "mixed-plugin-extra", 90);
    let unowned = dir.join("mixed-plugin-2000-01-01.log.backup");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&boundary, b"boundary").unwrap();
    std::fs::write(&sibling, b"sibling").unwrap();
    std::fs::write(&unowned, b"unowned").unwrap();

    let logger = PluginLogger::with_retention_policy(
        &plugin_id("MiXeD-PlUgIn"),
        &dir,
        1024,
        retention_policy(1024, 4096, 30, 30),
    );

    assert!(!old.exists(), "expired plugin log was retained");
    assert!(boundary.exists(), "age boundary should be inclusive");
    assert!(sibling.exists(), "a sibling plugin log was removed");
    assert!(unowned.exists(), "a non-log backup file was removed");
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_prunes_the_oldest_file_until_the_per_plugin_count_cap_holds() {
    let dir = temp_log_dir();
    let oldest = dated_log_path(&dir, "bounded", 4);
    let next = dated_log_path(&dir, "bounded", 3);
    let newer = dated_log_path(&dir, "bounded", 2);
    let newest = dated_log_path(&dir, "bounded", 1);
    for path in [&oldest, &next, &newer, &newest] {
        std::fs::write(path, b"1234").unwrap();
    }

    let logger = PluginLogger::with_retention_policy(
        &plugin_id("bounded"),
        &dir,
        1024,
        retention_policy(1024, 4096, 365, 3),
    );

    assert!(!oldest.exists());
    assert!(next.exists());
    assert!(newer.exists());
    assert!(newest.exists());
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_prunes_oldest_files_until_the_per_plugin_byte_cap_holds() {
    let dir = temp_log_dir();
    let oldest = dated_log_path(&dir, "byte-bounded", 4);
    let next = dated_log_path(&dir, "byte-bounded", 3);
    let newer = dated_log_path(&dir, "byte-bounded", 2);
    for path in [&oldest, &next, &newer] {
        std::fs::write(path, b"1234").unwrap();
    }

    let logger = PluginLogger::with_retention_policy(
        &plugin_id("byte-bounded"),
        &dir,
        1024,
        retention_policy(8, 4096, 365, 10),
    );

    assert!(!oldest.exists());
    assert!(next.exists());
    assert!(newer.exists());
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_is_throttled_on_admission_and_runs_on_day_rollover() {
    let dir = temp_log_dir();
    let logger = PluginLogger::with_retention_policy(
        &plugin_id("lifecycle"),
        &dir,
        4096,
        retention_policy(4096, 8192, 7, 8),
    );
    assert_eq!(logger.retention_scan_count_for_test(), 1);
    for _ in 0..20 {
        assert!(logger.write("admitted without a directory rescan"));
    }
    assert_eq!(
        logger.retention_scan_count_for_test(),
        1,
        "ordinary admitted lines must not scan the directory"
    );

    let admission_old = dated_log_path(&dir, "lifecycle", 8);
    std::fs::write(&admission_old, b"old").unwrap();
    logger.state.lock().last_retention_check = std::time::Instant::now()
        .checked_sub(Duration::from_secs(2 * 60 * 60))
        .unwrap();
    assert!(logger.write("admission after throttle interval"));
    assert!(!admission_old.exists());
    assert_eq!(logger.retention_scan_count_for_test(), 2);

    let rollover_old = dated_log_path(&dir, "lifecycle", 9);
    std::fs::write(&rollover_old, b"old").unwrap();
    logger.state.lock().file_date = Some("1900-01-01".to_string());

    assert!(logger.write("rollover"));
    assert!(
        !rollover_old.exists(),
        "day rollover did not rerun retention"
    );
    assert_eq!(logger.retention_scan_count_for_test(), 3);
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_preserves_the_active_file_when_it_alone_exceeds_limits() {
    let dir = temp_log_dir();
    let active = dated_log_path(&dir, "active", 0);
    std::fs::write(&active, b"active log must survive").unwrap();

    let logger = PluginLogger::with_retention_policy(
        &plugin_id("active"),
        &dir,
        1024,
        retention_policy(1, 1, 0, 0),
    );

    assert_eq!(std::fs::read(&active).unwrap(), b"active log must survive");
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_does_not_follow_a_matching_directory_link() {
    let dir = temp_log_dir();
    let outside = temp_log_dir();
    let sentinel = outside.join("outside-sentinel");
    std::fs::write(&sentinel, b"preserve").unwrap();
    let linked_log = dated_log_path(&dir, "linked", 60);
    create_directory_link(&outside, &linked_log);
    let expired_regular = dated_log_path(&dir, "linked", 59);
    std::fs::write(&expired_regular, b"old").unwrap();

    let logger = PluginLogger::with_retention_policy(
        &plugin_id("linked"),
        &dir,
        1024,
        retention_policy(1, 1024, 7, 1),
    );

    assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserve");
    assert!(linked_log.exists(), "cleanup removed the directory link");
    assert!(
        !expired_regular.exists(),
        "regular expired log was retained"
    );
    drop(logger);
    #[cfg(windows)]
    std::fs::remove_dir(&linked_log).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&linked_log).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn global_retention_evicts_oldest_owned_logs_across_plugins() {
    let dir = temp_log_dir();
    let oldest = dated_log_path(&dir, "other-plugin", 4);
    let next = dated_log_path(&dir, "global", 3);
    let newest = dated_log_path(&dir, "third-plugin", 2);
    for path in [&oldest, &next, &newest] {
        std::fs::write(path, b"12345678").unwrap();
    }

    let logger = PluginLogger::with_retention_policy(
        &plugin_id("global"),
        &dir,
        4096,
        retention_policy(4096, 16, 30, 30),
    );

    assert!(!oldest.exists(), "global cap must evict the oldest log");
    assert!(next.exists());
    assert!(newest.exists());
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_logger_initialization_serializes_retention_cleanup() {
    let dir = temp_log_dir();
    for (plugin, days_ago) in [("concurrent", 10), ("other", 9), ("third", 8)] {
        std::fs::write(dated_log_path(&dir, plugin, days_ago), vec![b'x'; 256]).unwrap();
    }
    let policy = retention_policy(512, 512, 30, 30);
    let threads = ["concurrent", "concurrent", "concurrent", "concurrent"].map(|id| {
        let dir = dir.clone();
        std::thread::spawn(move || {
            PluginLogger::with_retention_policy(&plugin_id(id), &dir, 4096, policy)
        })
    });
    let loggers = threads.map(|thread| thread.join().unwrap());

    let retained_bytes = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("log"))
        .try_fold(0u64, |total, entry| {
            total.checked_add(entry.metadata().ok()?.len())
        })
        .unwrap();
    assert!(retained_bytes <= 512, "global cap was not serialized");
    drop(loggers);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_logger_instances_share_the_daily_byte_cap() {
    let dir = temp_log_dir();
    let byte_cap = 2 * 1024;
    let start = Arc::new(std::sync::Barrier::new(2));
    let threads = [(), ()].map(|()| {
        let dir = dir.clone();
        let start = start.clone();
        std::thread::spawn(move || {
            let logger =
                PluginLogger::with_byte_cap(&plugin_id("concurrent-daily-cap"), &dir, byte_cap);
            start.wait();
            for _ in 0..100 {
                logger.write(&"x".repeat(100));
            }
        })
    });
    for thread in threads {
        thread.join().unwrap();
    }

    let path = dated_log_path(&dir, "concurrent-daily-cap", 0);
    let retained = std::fs::metadata(path).unwrap().len();
    assert!(
        retained <= byte_cap,
        "concurrent instances retained {retained} bytes above the {byte_cap}-byte cap"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn daily_log_names_use_the_normalized_plugin_identity() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 7, 22).unwrap();

    assert_eq!(
        daily_log_name(&plugin_id("MiXeD-Identity"), date),
        "mixed-identity-2026-07-22.log"
    );
}

#[test]
fn mixed_case_instances_share_one_daily_byte_cap() {
    let dir = temp_log_dir();
    let byte_cap = 2 * 1024;
    let start = Arc::new(std::sync::Barrier::new(2));
    let threads = ["MiXeD-Daily-Cap", "mixed-daily-cap"].map(|id| {
        let dir = dir.clone();
        let start = start.clone();
        std::thread::spawn(move || {
            let logger = PluginLogger::with_byte_cap(&plugin_id(id), &dir, byte_cap);
            start.wait();
            for _ in 0..100 {
                logger.write(&"x".repeat(100));
            }
        })
    });
    for thread in threads {
        thread.join().unwrap();
    }

    let path = dated_log_path(&dir, "mixed-daily-cap", 0);
    let retained = std::fs::metadata(path).unwrap().len();
    assert!(retained <= byte_cap);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cleanup_skips_a_busy_coordination_lock_without_blocking_construction() {
    let dir = temp_log_dir();
    let root = RetentionRoot::prepare(&dir).unwrap();
    let held_lock = root.open_coordination_lock().unwrap();
    std::fs::File::lock(&held_lock).unwrap();
    let worker_dir = dir.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let logger = PluginLogger::new(&plugin_id("busy-cleanup"), &worker_dir);
        ready_tx.send(()).unwrap();
        logger
    });

    let prompt = ready_rx.recv_timeout(Duration::from_millis(250));
    std::fs::File::unlock(&held_lock).unwrap();
    let logger = worker.join().unwrap();

    assert!(prompt.is_ok(), "logger construction blocked on cleanup");
    assert!(logger.write("cleanup can retry later"));
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validated_root_checks_the_exact_directory_handle_it_returns() {
    let dir = temp_log_dir();
    let moved = dir.with_extension("identity-before-open");
    let root = RetentionRoot::prepare(&dir).unwrap();

    let opened = root.open_validated_with_hook_for_test(|| {
        std::fs::rename(&dir, &moved).unwrap();
        std::fs::create_dir(&dir).unwrap();
    });

    assert!(opened.is_err(), "replacement root handle was accepted");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&moved);
}

#[test]
fn repeated_log_target_errors_are_observably_throttled() {
    let dir = temp_log_dir();
    let blocked_target = dated_log_path(&dir, "blocked-target", 0);
    std::fs::create_dir(&blocked_target).unwrap();
    let logger = PluginLogger::with_byte_cap(&plugin_id("blocked-target"), &dir, 4096);

    for _ in 0..20 {
        assert!(!logger.write("cannot open a directory as a log file"));
    }

    assert_eq!(
        logger.io_error_report_count_for_test(),
        1,
        "repeated I/O failures must produce one throttled observable error"
    );
    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replacing_the_configured_root_with_a_link_fails_closed() {
    let dir = temp_log_dir();
    let moved_dir = dir.with_extension("moved");
    let outside = temp_log_dir();
    let outside_log = dated_log_path(&outside, "root-swap", 60);
    std::fs::write(&outside_log, b"outside").unwrap();
    let logger = PluginLogger::with_retention_policy(
        &plugin_id("root-swap"),
        &dir,
        4096,
        retention_policy(1, 1, 7, 1),
    );
    if let Err(error) = std::fs::rename(&dir, &moved_dir) {
        #[cfg(windows)]
        {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            drop(logger);
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }
        #[cfg(not(windows))]
        panic!("failed to replace logger root for regression: {error}");
    }
    create_directory_link(&outside, &dir);
    logger.state.lock().file_date = Some("1900-01-01".to_string());

    assert!(!logger.write("must not follow replaced root"));
    for _ in 0..20 {
        assert!(!logger.write("repeated failure must not spam host tracing"));
    }
    assert_eq!(std::fs::read(&outside_log).unwrap(), b"outside");
    assert_eq!(
        logger.io_error_report_count_for_test(),
        1,
        "repeated I/O failures must produce one throttled observable error"
    );

    #[cfg(windows)]
    std::fs::remove_dir(&dir).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&dir).unwrap();
    drop(logger);
    let _ = std::fs::remove_dir_all(&moved_dir);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn replacing_the_configured_root_with_a_different_directory_fails_closed() {
    let dir = temp_log_dir();
    let moved_dir = dir.with_extension("moved-directory");
    let logger = PluginLogger::with_retention_policy(
        &plugin_id("directory-swap"),
        &dir,
        4096,
        retention_policy(1, 1, 7, 1),
    );
    if let Err(error) = std::fs::rename(&dir, &moved_dir) {
        #[cfg(windows)]
        {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            drop(logger);
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        #[cfg(not(windows))]
        panic!("failed to replace logger root for regression: {error}");
    }
    std::fs::create_dir(&dir).unwrap();
    let replacement_log = dated_log_path(&dir, "directory-swap", 60);
    std::fs::write(&replacement_log, b"replacement").unwrap();
    logger.state.lock().file_date = Some("1900-01-01".to_string());

    assert!(!logger.write("must not use a replacement directory"));
    assert_eq!(std::fs::read(&replacement_log).unwrap(), b"replacement");

    drop(logger);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&moved_dir);
}
