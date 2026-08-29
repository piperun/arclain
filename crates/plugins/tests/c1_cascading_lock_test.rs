//! Characterization test for C1 from `docs/AUDIT_2026-05-03.md`.
//!
//! Pre-fix, `PluginManager::set_async_http_client` held
//! `self.plugins.read()` while iterating, locking per-plugin
//! `instance.lock()` AND calling `client.approve_domain(...)` which
//! takes `whitelist.write()`. That nested acquisition order
//! (plugins → instance → whitelist) creates a deadlock cycle with any
//! code path that holds `whitelist.read()` and later wants
//! `plugins.read()` (writer-preference on `parking_lot::RwLock` blocks
//! new readers behind a pending writer).
//!
//! Post-fix, `set_async_http_client` snapshots the per-plugin data it
//! needs (instance Arc + manifest domains) under a *brief*
//! `plugins.read()`, drops the guard, then runs the per-plugin work
//! (instance.lock + whitelist.write) outside the read guard. The
//! plugins lock is no longer held during whitelist mutation, so the
//! cycle can't form.
//!
//! Constructing a real `PluginManager` with loaded plugins requires a
//! WASM component for testing, so this test exercises the lock
//! acquisition pattern in isolation using bare `parking_lot::RwLock`s.
//! Both tests pass regardless of `manager/mod.rs` state — they
//! characterize Rust + parking_lot semantics, not the production code
//! path. A true production-side regression test would need WASM
//! scaffolding to load a stub plugin and is tracked separately.

use parking_lot::RwLock;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const HOLD_DURATION: Duration = Duration::from_millis(300);

/// Pre-fix shape: thread A holds `plugins.read()` and tries
/// `whitelist.write()` while still holding the read guard. Thread B
/// holds `whitelist.read()` and tries `plugins.read()`. A pending
/// writer on `plugins` (thread C) makes B's `plugins.read()` block
/// behind it (parking_lot writer-preference). A then waits forever
/// on `whitelist.write()` because B holds `whitelist.read()`. Cycle.
///
/// Bounded by a generous timeout: if everyone finishes in under
/// `HOLD_DURATION + slack`, no cycle. Currently this test detects the
/// cycle (test passes, asserting deadlock).
#[test]
// Demonstrates the hazard the fix removed; it is not a regression test
// and must not gate CI.
//
// It asserts that the deadlock *reproduces* -- so it fails on the runs
// where scheduling happens to avoid it, which is the outcome nobody
// should be paged for. Its own message says as much: "this test happened
// to schedule lucky". A shared runner reported it as a red build for a
// race that did not occur.
//
// `c1_snapshot_then_release_avoids_cycle` below is the real guard: it
// asserts the post-fix shape holds, and it is deterministic. Run this one
// deliberately with `cargo test -- --ignored` when reasoning about the
// lock order.
#[ignore = "asserts a race reproduces, so it fails whenever scheduling is kind"]
fn c1_nested_lock_acquisition_can_cycle() {
    let plugins: Arc<RwLock<()>> = Arc::new(RwLock::new(()));
    let whitelist: Arc<RwLock<()>> = Arc::new(RwLock::new(()));

    // Thread A: simulates the buggy `set_async_http_client` pattern —
    // takes plugins.read(), holds it, then later wants whitelist.write().
    let plugins_a = plugins.clone();
    let whitelist_a = whitelist.clone();
    let t_a = thread::spawn(move || {
        let _plugins_guard = plugins_a.read();
        // Give B time to acquire whitelist.read and C time to register
        // its pending plugins.write.
        thread::sleep(HOLD_DURATION);
        // Now try to take whitelist.write while still holding plugins.read.
        // With B holding whitelist.read, this blocks.
        whitelist_a
            .try_write_for(Duration::from_millis(800))
            .is_some()
    });

    // Thread B: simulates a hypothetical concurrent path that holds
    // whitelist.read and later wants plugins.read.
    let plugins_b = plugins.clone();
    let whitelist_b = whitelist.clone();
    let t_b = thread::spawn(move || {
        // Brief delay so A acquires plugins.read first.
        thread::sleep(Duration::from_millis(50));
        let _whitelist_guard = whitelist_b.read();
        thread::sleep(HOLD_DURATION);
        // Now try to take plugins.read. With C's pending writer
        // (registered after A's read), parking_lot blocks new readers.
        plugins_b.try_read_for(Duration::from_millis(800)).is_some()
    });

    // Thread C: simulates anyone calling something that needs
    // plugins.write (e.g. plugin loader). Registers the pending
    // writer that triggers writer-preference for B.
    let plugins_c = plugins.clone();
    let t_c = thread::spawn(move || {
        // Wait until A is holding plugins.read.
        thread::sleep(Duration::from_millis(100));
        // Now try plugins.write. Blocks behind A's read.
        plugins_c.try_write_for(Duration::from_secs(2)).is_some()
    });

    let a_succeeded = t_a.join().expect("thread A panicked");
    let b_succeeded = t_b.join().expect("thread B panicked");
    let c_succeeded = t_c.join().expect("thread C panicked");

    // Pre-fix: at least one of A/B times out (the cycle). If we ever
    // see all three succeed, the cycle didn't form on this run — flaky
    // scheduling, but the structural risk remains.
    assert!(
        !(a_succeeded && b_succeeded && c_succeeded),
        "C1 cycle didn't reproduce on this run: A={}, B={}, C={}. \
         The structural risk in the pre-fix `set_async_http_client` \
         pattern (plugins.read() held during whitelist.write()) still \
         exists; this test happened to schedule lucky.",
        a_succeeded,
        b_succeeded,
        c_succeeded,
    );
}

/// Post-fix shape: thread A snapshots under plugins.read(), drops the
/// guard, then takes whitelist.write(). With the read released, C's
/// pending writer can complete, B's plugins.read() proceeds, B drops
/// whitelist.read(), and A's whitelist.write() proceeds. No cycle.
#[test]
fn c1_snapshot_then_release_avoids_cycle() {
    let plugins: Arc<RwLock<()>> = Arc::new(RwLock::new(()));
    let whitelist: Arc<RwLock<()>> = Arc::new(RwLock::new(()));

    let plugins_a = plugins.clone();
    let whitelist_a = whitelist.clone();
    let t_a = thread::spawn(move || {
        // Snapshot under plugins.read, then drop before taking
        // whitelist.write — the post-fix shape.
        {
            let _plugins_guard = plugins_a.read();
            thread::sleep(HOLD_DURATION);
        }
        whitelist_a
            .try_write_for(Duration::from_millis(800))
            .is_some()
    });

    let plugins_b = plugins.clone();
    let whitelist_b = whitelist.clone();
    let t_b = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let _whitelist_guard = whitelist_b.read();
        thread::sleep(HOLD_DURATION);
        plugins_b.try_read_for(Duration::from_millis(800)).is_some()
    });

    let plugins_c = plugins.clone();
    let t_c = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        plugins_c.try_write_for(Duration::from_secs(2)).is_some()
    });

    let started = Instant::now();
    let a_succeeded = t_a.join().expect("thread A panicked");
    let b_succeeded = t_b.join().expect("thread B panicked");
    let c_succeeded = t_c.join().expect("thread C panicked");
    let elapsed = started.elapsed();

    assert!(
        a_succeeded && b_succeeded && c_succeeded,
        "C1 fix shape regressed: expected all three threads to acquire \
         their second lock without timing out; got A={}, B={}, C={}",
        a_succeeded,
        b_succeeded,
        c_succeeded,
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "C1 fix shape: total time {:?} suggests contention serialization, not cycle",
        elapsed,
    );
}
