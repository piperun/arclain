//! Characterization tests for C2 from `docs/AUDIT_2026-05-03.md`.
//!
//! Pre-fix, `crates/plugins/src/manager/dispatch.rs::event_worker` held
//! `instance.lock()` across blocking `client.get_metadata` calls, forcing
//! every other operation on the same plugin (every UI render that touched
//! `get_top_tabs`, `get_ui_layout`, `get_all_settings`, etc.) to wait for
//! the HTTP round-trip to finish.
//!
//! Post-fix, `event_worker` snapshots `gameta_client` and `metadata_signal`
//! under a brief lock, drops the lock, then runs the blocking HTTP call
//! outside any lock. The native-fetch fallback re-acquires briefly for the
//! WASM call.
//!
//! These tests document the anti-pattern (test 1) and the fixed shape
//! (test 2) using the same primitives `event_worker` uses
//! (`parking_lot::Mutex`, real `GametaClient`, real reqwest blocking HTTP,
//! wiremock). They both pass regardless of `dispatch.rs` state — they
//! characterize Rust mutex semantics, not the production path. A true
//! regression test against `event_worker` would need WASM scaffolding to
//! load a stub plugin and is tracked separately.

use arclain_network::features::gameta_client::{GametaClient, ServerConfig};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// HTTP round-trip latency the wiremock server simulates. Picked to be
/// large enough that the concurrent-acquire timing isn't dominated by
/// thread-scheduling jitter on a busy CI host.
const HTTP_LATENCY: Duration = Duration::from_millis(500);

/// How long thread B is allowed to wait for the lock before we consider
/// the wait "fast" (i.e. the lock wasn't held during HTTP). Generous
/// enough to absorb scheduling noise.
const FAST_LOCK_THRESHOLD: Duration = Duration::from_millis(150);

/// Minimum wait we require to confirm the lock was held for the full
/// HTTP duration. Below the actual HTTP_LATENCY to allow startup overhead.
const SLOW_LOCK_THRESHOLD: Duration = Duration::from_millis(400);

/// Pre-fix shape: lock held across the HTTP call. Concurrent acquirers
/// wait the full round-trip. `event_worker` no longer follows this
/// pattern; this test documents what it used to do.
#[tokio::test]
async fn c2_lock_held_during_blocking_http_blocks_other_acquirers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/RJ12345"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(
                    r#"{"id":"dlsite:RJ12345","source":"dlsite","tags":[],"extras":{}}"#,
                    "application/json",
                )
                .set_delay(HTTP_LATENCY),
        )
        .mount(&server)
        .await;

    let url = server.uri();
    let instance_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // Thread A: mirrors `event_worker` at dispatch.rs:115 (lock instance) +
    // line 147 (call get_metadata while holding the lock).
    let lock_a = instance_lock.clone();
    let url_a = url.clone();
    let t1 = tokio::task::spawn_blocking(move || {
        let _guard = lock_a.lock();
        let client = GametaClient::new(ServerConfig {
            url: url_a,
            api_key: None,
        });
        let _ = client.get_metadata("dlsite", "RJ12345");
    });

    // Give thread A a moment to acquire the lock and start the HTTP call.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Thread B: mirrors any UI render path that needs the same plugin's
    // instance lock (e.g. `get_top_tabs` per frame). Times the wait.
    let lock_b = instance_lock.clone();
    let start = Instant::now();
    let t2 = tokio::task::spawn_blocking(move || {
        let _guard = lock_b.lock();
    });
    t2.await.expect("thread B panicked");
    let elapsed = start.elapsed();

    t1.await.expect("thread A panicked");

    assert!(
        elapsed >= SLOW_LOCK_THRESHOLD,
        "C2 not reproduced: thread B acquired the lock in {:?}, expected ≥ {:?}. \
         Either wiremock didn't apply the {:?} delay, or the lock-holding \
         pattern doesn't reproduce on this platform.",
        elapsed,
        SLOW_LOCK_THRESHOLD,
        HTTP_LATENCY,
    );
    assert!(
        elapsed < HTTP_LATENCY + Duration::from_secs(2),
        "C2 acquire took absurdly long ({:?}) — likely a real deadlock, not the \
         expected lock-during-HTTP pattern.",
        elapsed,
    );
}

/// Post-fix shape: snapshot the client under the lock, drop the lock,
/// then run the blocking HTTP. Concurrent acquirers acquire fast.
/// This is the shape `event_worker` now uses (with the gameta_client
/// + metadata_signal snapshot).
#[tokio::test]
async fn c2_dropping_lock_before_http_does_not_block_others() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/RJ12345"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(
                    r#"{"id":"dlsite:RJ12345","source":"dlsite","tags":[],"extras":{}}"#,
                    "application/json",
                )
                .set_delay(HTTP_LATENCY),
        )
        .mount(&server)
        .await;

    let url = server.uri();
    let instance_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // Thread A: lock briefly to extract the gameta_client Arc, drop the
    // lock, THEN run the HTTP call. This is the post-fix shape.
    let lock_a = instance_lock.clone();
    let url_a = url.clone();
    let t1 = tokio::task::spawn_blocking(move || {
        let client = {
            let _guard = lock_a.lock();
            GametaClient::new(ServerConfig {
                url: url_a,
                api_key: None,
            })
            // _guard drops here, before the HTTP call below
        };
        let _ = client.get_metadata("dlsite", "RJ12345");
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let lock_b = instance_lock.clone();
    let start = Instant::now();
    let t2 = tokio::task::spawn_blocking(move || {
        let _guard = lock_b.lock();
    });
    t2.await.expect("thread B panicked");
    let elapsed = start.elapsed();

    t1.await.expect("thread A panicked");

    assert!(
        elapsed < FAST_LOCK_THRESHOLD,
        "Expected the post-fix shape to acquire the lock fast (< {:?}), got {:?}",
        FAST_LOCK_THRESHOLD,
        elapsed,
    );
}
