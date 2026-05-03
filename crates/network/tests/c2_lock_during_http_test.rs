//! Characterization test for C2 from `docs/AUDIT_2026-05-03.md`.
//!
//! `crates/plugins/src/manager/dispatch.rs:115-186` (`event_worker`) takes
//! `instance.lock()` on a plugin, then while holding that guard calls
//! `client.get_metadata(...)` — a synchronous `reqwest::blocking` HTTP request
//! with a 10s timeout. Any other code path that needs the same plugin's
//! instance lock (every UI render that calls `get_top_tabs`,
//! `get_ui_layout`, `get_all_settings`, etc.) is forced to wait for the
//! HTTP round-trip to finish.
//!
//! Constructing a real `PluginInstance` for testing requires a compiled WASM
//! component and a full wasmtime engine, so we can't drive `event_worker`
//! end-to-end here. Instead this test uses the same primitives the buggy
//! path uses (`parking_lot::Mutex`, real `GametaClient`, real reqwest
//! blocking HTTP, wiremock for the server) and reproduces the structure:
//! lock the mutex, call `get_metadata` while holding it, then time how long
//! a concurrent acquirer waits.
//!
//! The assertion is that the concurrent waiter blocks for the full HTTP
//! latency. This proves the C2 finding: the dispatch pattern at lines
//! 115-186 holds the instance lock across a blocking network call.
//!
//! After the C2 fix (drop the instance lock before HTTP, re-acquire to push
//! the result back into instance state), the production dispatch path
//! should be covered by a separate regression test that exercises
//! `event_worker` directly — likely with a stub plugin loaded into a real
//! `PluginManager`. This test would then be retired or repurposed.

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

#[tokio::test]
async fn c2_event_worker_pattern_holds_lock_during_get_metadata() {
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

/// Counterexample: when the lock is dropped before HTTP, a concurrent
/// acquirer should not be forced to wait for HTTP. This is the shape the
/// fix should restore in `event_worker`.
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
