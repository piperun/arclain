//! Integration tests for the display-image surface:
//! `ArclainApp::{read_host_image, discard_host_image, fetch_host_image}`
//! and `ArclainApp::fetch_plugin_image`.
//!
//! These exist so a frontend needs neither a `ContentCache` handle nor an
//! HTTP client to render an image reference. The unit tests in
//! `crates/app/src/plugins.rs` cover the pure namespace logic and the
//! cache read/write halves in isolation against a hand-built cache; this
//! file's job is the wiring behind the public API against a real
//! bootstrap: that a fetch really does reach the network exactly once and
//! is served from the cache afterwards, that the bytes land in the
//! namespace the matching read resolves, and that a crafted key cannot
//! cross between the two namespaces in either direction.
//!
//! Every test is a plain (synchronous) `#[test]` awaiting facade futures on
//! a *foreign* runtime, following this crate's convention (see
//! `network_surface.rs`'s module doc comment): `ArclainApp` owns its own
//! Tokio runtime, dropping it must not happen from inside an async
//! context, and awaiting from a foreign executor is exactly the
//! executor-agnostic contract the facade promises.

mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::plugins::{MAX_HOST_IMAGE_BYTES, MAX_PLUGIN_IMAGE_BYTES};
use arclain_app::{ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

fn dummy_sevenzip(temp: &tempfile::TempDir) -> PathBuf {
    support::create_dummy_executable(temp.path(), sevenzip_exe_name())
}

/// Bootstraps an `ArclainApp` against an isolated temp profile.
///
/// The isolation is load-bearing beyond the usual "don't touch the
/// developer's profile": the content cache reconciles its own physical
/// blobs against its index at construction, deleting anything the index
/// does not reference. Two bootstraps sharing one cache root would
/// therefore destroy each other's blobs, so every test gets its own
/// `paths_override` root.
fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed")
}

/// A request-counting HTTP stub that answers every `GET` with the same
/// body and content type.
///
/// The counter is the whole point: "fetched once, then served from cache"
/// is only proven by a *server* that saw exactly one request. Asserting on
/// the returned `served_from_cache` flag alone would pass even if the
/// application re-fetched and then reported the wrong flag.
///
/// Hand-rolled on `std::net::TcpListener` for the same reason
/// `network_surface.rs` hand-rolls its health stub: these tests are
/// synchronous with no ambient async runtime, and the surface under test
/// needs a handful of well-formed responses, not a mock framework.
struct ImageStub {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl ImageStub {
    fn start(body: Vec<u8>, content_type: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the image stub");
        let address = listener.local_addr().expect("read the stub address");
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let served = requests.clone();
        let stopped = stop.clone();
        let body = Arc::new(body);
        std::thread::spawn(move || {
            while let Ok((mut socket, _)) = listener.accept() {
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
                let mut request = Vec::new();
                let mut chunk = [0_u8; 512];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match socket.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                // Only a complete request head counts, and the count
                // happens here rather than at accept time. Loopback
                // ephemeral ports are shared with every other test binary
                // running in parallel -- one of them reserves a port,
                // frees it, and connects to prove the connection is
                // refused. If that port has since been handed to this
                // stub, the connect lands here instead, and counting it
                // would inflate the fetch-count assertions this stub
                // exists to make. A bare connect sends nothing, so it
                // never completes a request head.
                if !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    continue;
                }
                served.fetch_add(1, Ordering::SeqCst);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len(),
                );
                let _ = socket.write_all(header.as_bytes());
                let _ = socket.write_all(&body);
                let _ = socket.flush();
            }
        });
        Self {
            address,
            requests,
            stop,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/cover.png", self.address)
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for ImageStub {
    /// Signals the accept loop and wakes it with one throwaway connection.
    ///
    /// Deliberately does not `join`: an untimed `accept()` joined during a
    /// panic's unwind turns a red test into a hung test binary. The thread
    /// observes the flag on its next wakeup and returns on its own.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
    }
}

/// A body big enough to clear the fetch path's "real images are >1KB"
/// floor, prefixed with the PNG signature so its intent is legible.
fn png_body(len: usize) -> Vec<u8> {
    let mut body = b"\x89PNG\r\n\x1a\n".to_vec();
    body.resize(len, 0x5A);
    body
}

const HOST_KEY: &str = "dlsite:image:RJ000001";

// ---------------------------------------------------------------------------
// fetch_host_image
// ---------------------------------------------------------------------------

/// The core affordance: a miss fetches and caches, and every later call for
/// the same key is served from the cache without a second request.
#[test]
fn fetch_host_image_fetches_once_and_serves_the_cache_afterwards() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let body = png_body(2048);
    let stub = ImageStub::start(body.clone(), "image/png");
    let runtime = foreign_runtime();

    let fetched = runtime
        .block_on(app.fetch_host_image(HOST_KEY.to_string(), stub.url(), None))
        .expect("a cache miss must fetch the image");
    assert_eq!(fetched.bytes, body);
    assert!(!fetched.served_from_cache);
    assert_eq!(stub.request_count(), 1);

    let cached = runtime
        .block_on(app.fetch_host_image(HOST_KEY.to_string(), stub.url(), None))
        .expect("a warm key must resolve");
    assert_eq!(cached.bytes, body);
    assert!(cached.served_from_cache);
    assert_eq!(
        stub.request_count(),
        1,
        "a cached key must not reach the network a second time"
    );

    // The read half finds what the fetch wrote -- the same namespace, so a
    // renderer that fetched can read the asset back on its next pass.
    assert_eq!(
        runtime
            .block_on(app.read_host_image(HOST_KEY.to_string()))
            .expect("the fetched bytes must be readable through the host read"),
        body
    );
}

/// The write half of the host cap. The ceiling is enforced while the body
/// is being read, so an oversized response is refused rather than buffered
/// whole and rejected afterwards -- and nothing is cached.
#[test]
fn fetch_host_image_refuses_a_response_over_the_size_cap() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(MAX_HOST_IMAGE_BYTES as usize + 1), "image/png");
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.fetch_host_image(HOST_KEY.to_string(), stub.url(), None))
        .expect_err("an oversized image must be refused");

    assert_eq!(error.kind, ApplicationErrorKind::Backend);
    assert_eq!(
        runtime
            .block_on(app.read_host_image(HOST_KEY.to_string()))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::NotFound,
        "a refused fetch must not leave a cache entry behind"
    );
}

/// A body whose declared type is not an image is refused, so an HTML error
/// page can never be cached and then served back as an image.
#[test]
fn fetch_host_image_refuses_a_non_image_content_type() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(2048), "text/html");
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.fetch_host_image(HOST_KEY.to_string(), stub.url(), None))
        .expect_err("a non-image response must be refused");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(
        runtime
            .block_on(app.read_host_image(HOST_KEY.to_string()))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::NotFound
    );
}

/// `on_behalf_of_plugin` spends the *plugin's* network policy, not the
/// host's direct path: an unconfigured plugin is refused before a
/// connection is ever opened, so the stub sees nothing.
#[test]
fn fetch_host_image_on_behalf_of_a_plugin_goes_through_that_plugins_network_policy() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(2048), "image/png");
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.fetch_host_image(
            HOST_KEY.to_string(),
            stub.url(),
            Some("not-installed".to_string()),
        ))
        .expect_err("a plugin with no network policy must be refused");

    assert_eq!(error.kind, ApplicationErrorKind::Backend);
    assert_eq!(
        stub.request_count(),
        0,
        "the plugin gate must refuse before any connection is opened"
    );
}

// ---------------------------------------------------------------------------
// Namespace separation: crafted keys, both directions
// ---------------------------------------------------------------------------

/// Host door, crafted key: `fetch_host_image` must not be a way to write
/// into a plugin's cache namespace. The refusal happens before any network
/// work, and the victim's own entry is untouched.
#[test]
fn fetch_host_image_refuses_a_key_naming_a_plugin_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(2048), "image/png");
    let runtime = foreign_runtime();
    let victim_key = "plugin-image:victim-plugin:secret".to_string();
    let victim_bytes = png_body(1200);
    runtime
        .block_on(app.write_plugin_image(
            "victim-plugin".to_string(),
            victim_key.clone(),
            victim_bytes.clone(),
            None,
        ))
        .expect("the owning plugin may seed its own namespace");

    let error = runtime
        .block_on(app.fetch_host_image(victim_key.clone(), stub.url(), None))
        .expect_err("a plugin-scoped key must be refused by the host surface");

    assert_eq!(error.kind, ApplicationErrorKind::PermissionDenied);
    assert_eq!(error.field.as_deref(), Some("cache_key"));
    assert_eq!(
        stub.request_count(),
        0,
        "the namespace refusal must precede any network work"
    );
    assert_eq!(
        runtime
            .block_on(app.read_plugin_image(victim_key.clone()))
            .expect("the victim's entry must still resolve"),
        victim_bytes,
        "the victim's bytes must be untouched"
    );
    assert_eq!(
        runtime
            .block_on(app.discard_host_image(victim_key))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::PermissionDenied,
        "nor may the host surface evict a plugin's entry"
    );
}

/// Plugin door, crafted key: `fetch_plugin_image` must not be a way to
/// write into the shared host namespace, nor into another plugin's.
#[test]
fn fetch_plugin_image_refuses_host_owned_and_foreign_keys() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(2048), "image/png");
    let runtime = foreign_runtime();

    let host_owned = runtime
        .block_on(app.fetch_plugin_image("attacker".to_string(), HOST_KEY.to_string(), stub.url()))
        .expect_err("a host-owned key must be refused by the plugin surface");
    assert_eq!(host_owned.kind, ApplicationErrorKind::NotFound);

    let foreign = runtime
        .block_on(app.fetch_plugin_image(
            "attacker".to_string(),
            "plugin-image:victim-plugin:secret".to_string(),
            stub.url(),
        ))
        .expect_err("a key naming another plugin must be refused");
    assert_eq!(foreign.kind, ApplicationErrorKind::PermissionDenied);
    assert_eq!(foreign.field.as_deref(), Some("cache_key"));

    let malformed = runtime
        .block_on(app.fetch_plugin_image(
            "../../escape".to_string(),
            "plugin-image:../../escape:k".to_string(),
            stub.url(),
        ))
        .expect_err("a malformed owner must not mint a namespace");
    assert_eq!(malformed.kind, ApplicationErrorKind::InvalidInput);

    assert_eq!(
        stub.request_count(),
        0,
        "every namespace refusal must precede any network work"
    );
    assert_eq!(
        runtime
            .block_on(app.read_host_image(HOST_KEY.to_string()))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::NotFound,
        "a refused plugin fetch must not have written the host namespace"
    );
}

/// A plugin-scoped key already in the plugin's namespace is served from
/// there with no network request -- the same "fetch once" property the host
/// path has, and proof that the fetch resolves the namespace the write and
/// the read both use.
#[test]
fn fetch_plugin_image_serves_the_plugins_own_namespace_without_refetching() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(2048), "image/png");
    let runtime = foreign_runtime();
    let key = "plugin-image:demo-plugin:cover:RJ1".to_string();
    let bytes = png_body(1500);
    runtime
        .block_on(app.write_plugin_image(
            "demo-plugin".to_string(),
            key.clone(),
            bytes.clone(),
            None,
        ))
        .expect("seeding the plugin's own namespace must succeed");

    let served = runtime
        .block_on(app.fetch_plugin_image("demo-plugin".to_string(), key.clone(), stub.url()))
        .expect("a warm plugin key must resolve");

    assert_eq!(served.bytes, bytes);
    assert!(served.served_from_cache);
    assert_eq!(stub.request_count(), 0);
    assert_eq!(
        runtime
            .block_on(app.read_plugin_image(key))
            .expect("the plugin read resolves the same namespace"),
        bytes
    );
}

/// The plugin cap is refused on write, so an oversized asset never becomes
/// a cache entry that every later read would reject anyway.
#[test]
fn write_plugin_image_and_the_host_read_disagree_about_nothing_at_the_cap() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();
    let key = "plugin-image:demo-plugin:huge".to_string();

    let error = runtime
        .block_on(app.write_plugin_image(
            "demo-plugin".to_string(),
            key.clone(),
            vec![0_u8; MAX_PLUGIN_IMAGE_BYTES as usize + 1],
            None,
        ))
        .expect_err("an oversized plugin image must be refused on write");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(
        runtime
            .block_on(app.read_plugin_image(key))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::NotFound
    );
}

// ---------------------------------------------------------------------------
// discard_host_image / materialized_resource_limit
// ---------------------------------------------------------------------------

/// The corrupt-entry escape hatch: an entry a frontend cannot decode is
/// removable, and the removal is reported so a caller can tell "evicted"
/// from "was never there".
#[test]
fn discard_host_image_removes_the_entry_once_and_reports_whether_it_did() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = ImageStub::start(png_body(2048), "image/png");
    let runtime = foreign_runtime();
    runtime
        .block_on(app.fetch_host_image(HOST_KEY.to_string(), stub.url(), None))
        .expect("seed the host namespace through a fetch");

    assert!(runtime
        .block_on(app.discard_host_image(HOST_KEY.to_string()))
        .expect("discarding a present entry must succeed"));
    assert!(!runtime
        .block_on(app.discard_host_image(HOST_KEY.to_string()))
        .expect("discarding a missing entry is not an error"));
    assert_eq!(
        runtime
            .block_on(app.read_host_image(HOST_KEY.to_string()))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::NotFound
    );
}

/// Synchronous, callable from any thread with no runtime in scope at all --
/// which is the reason it is not `async`: its callers are render-path code
/// that would otherwise have to block on the runtime to read a constant.
#[test]
fn materialized_resource_limit_is_readable_without_a_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let limit = app.materialized_resource_limit();

    assert!(limit > 0, "a zero ceiling would refuse every fetch");
    assert_eq!(
        limit,
        std::thread::spawn(move || app.materialized_resource_limit())
            .join()
            .unwrap(),
        "the ceiling is a resolved constant, not per-caller state"
    );
}

/// The stub's counter must mean "HTTP requests served", not "TCP
/// connections accepted".
///
/// Not a test of production code, but the fetch-count assertions above are
/// only as trustworthy as this: loopback ephemeral ports are shared with
/// every concurrently running test binary, and one of them deliberately
/// frees a port and connects to it to prove the connection is refused. If
/// that lands on this stub and counts, "fetched exactly once" starts
/// failing for a reason that has nothing to do with the code under test --
/// which is precisely how this file first went red under a full parallel
/// run.
#[test]
fn the_stub_counts_requests_rather_than_connections() {
    let stub = ImageStub::start(png_body(2048), "image/png");

    drop(TcpStream::connect(stub.address).expect("connect without sending a request"));
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        stub.request_count(),
        0,
        "a bare connect must not count as a request"
    );
}
