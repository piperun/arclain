//! Integration tests for the boundary-zero network surface:
//! `ArclainApp::plugin_domain_whitelist`, `ArclainApp::
//! test_gameta_connection`, and the free function
//! `arclain_app::analyze_url`.
//!
//! These three exist so a frontend never needs `arclain-network` as a
//! direct dependency to render a plugin's domain-access section or to
//! test a gameta server the user just typed into a settings form. The
//! unit tests in `crates/app/src/plugins.rs` cover the pure mirroring and
//! filtering in isolation; this file's job is proving the wiring behind
//! the public API against a real bootstrap -- in particular that
//! `plugin_domain_whitelist` reads the *live* whitelist this application
//! composed (not a parallel copy) and that `test_gameta_connection`
//! writes nothing anywhere.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! following this crate's established convention (see
//! `archive_sessions.rs`'s own module doc comment for why): `ArclainApp`
//! owns its own Tokio runtime, and dropping it must not happen from
//! inside an async context.

mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;

use arclain_app::error::ApplicationErrorKind;
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

/// Bootstraps an `ArclainApp` against an isolated temp profile -- see
/// `settings_facade.rs::bootstrap_app`'s identical doc comment for why
/// the dummy 7-Zip seeding is required even for tests that never touch an
/// archive backend.
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

// ---------------------------------------------------------------------------
// plugin_domain_whitelist
// ---------------------------------------------------------------------------

/// Seeds the *live* whitelist this bootstrap composed, through the same
/// `take_legacy_composition` handle the egui frontend holds. Seeding here
/// rather than through some facade-private back door is the point of the
/// test: if `plugin_domain_whitelist` ever started reading a parallel
/// copy of the whitelist instead of the composed one, every assertion
/// below would fail.
fn seed_whitelist(app: &ArclainApp, seed: impl FnOnce(&arclain_core::services::Services)) {
    let composition = app
        .take_legacy_composition()
        .expect("the legacy composition must be available");
    seed(&composition.core_services);
}

#[test]
fn plugin_domain_whitelist_reads_the_live_composed_whitelist() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    seed_whitelist(&app, |services| {
        let whitelist = services.domain_whitelist.read();
        whitelist.add_pending("reader-plugin", "pending.example");
        whitelist.approve("reader-plugin", "approved.example");
        whitelist.add_pending("other-plugin", "unrelated.example");
    });

    let entries = foreign_runtime()
        .block_on(app.plugin_domain_whitelist("reader-plugin".to_string()))
        .expect("reading a plugin's whitelist must succeed");

    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.domain.as_str(), entry.approved))
            .collect::<Vec<_>>(),
        vec![("approved.example", true), ("pending.example", false)],
    );
    assert!(entries
        .iter()
        .all(|entry| entry.plugin_id == "reader-plugin"));
}

/// An approval made *after* a first read is visible on the next one --
/// the read is live, not a snapshot taken at bootstrap.
#[test]
fn plugin_domain_whitelist_observes_an_approval_made_after_the_first_read() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    seed_whitelist(&app, |services| {
        services
            .domain_whitelist
            .read()
            .add_pending("late-plugin", "dlsite.example");
    });
    let runtime = foreign_runtime();

    let before = runtime
        .block_on(app.plugin_domain_whitelist("late-plugin".to_string()))
        .expect("first read");
    assert_eq!(before.len(), 1);
    assert!(!before[0].approved);

    seed_whitelist(&app, |services| {
        services
            .domain_whitelist
            .read()
            .approve("late-plugin", "dlsite.example");
    });

    let after = runtime
        .block_on(app.plugin_domain_whitelist("late-plugin".to_string()))
        .expect("second read");
    assert_eq!(after.len(), 1);
    assert!(
        after[0].approved,
        "the facade served a stale copy instead of the live whitelist",
    );
}

#[test]
fn plugin_domain_whitelist_reports_no_domains_for_a_plugin_that_asked_for_none() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let entries = foreign_runtime()
        .block_on(app.plugin_domain_whitelist("never-asked".to_string()))
        .expect("an unknown plugin is not an error");

    assert!(entries.is_empty());
}

#[test]
fn plugin_domain_whitelist_rejects_a_blank_plugin_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = foreign_runtime()
        .block_on(app.plugin_domain_whitelist(String::new()))
        .expect_err("a blank plugin id must not be answered with an empty list");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("plugin_id"));
}

// ---------------------------------------------------------------------------
// test_gameta_connection
// ---------------------------------------------------------------------------

/// A single-request HTTP stub answering `GET /api/v1/health` with the
/// body `GametaClient::health` expects, then closing the connection.
///
/// Hand-rolled on `std::net::TcpListener` rather than pulled in as a mock
/// framework for the same reason `settings_facade.rs` hand-rolls its
/// SOCKS5 sentinel: this crate's tests are synchronous `#[test]`s with no
/// ambient async runtime, and the surface under test needs exactly one
/// well-formed response.
struct HealthStub {
    address: SocketAddr,
    server: Option<std::thread::JoinHandle<()>>,
}

impl HealthStub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the health stub");
        let address = listener.local_addr().expect("read the stub address");
        let server = std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            // Read just far enough to see the end of the request headers;
            // the client sends no body for a GET.
            let mut request = Vec::new();
            let mut chunk = [0_u8; 512];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match socket.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&chunk[..read]),
                }
            }
            const BODY: &str = r#"{"status":"ok","version":"9.9.9"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
                BODY.len(),
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
        });
        Self {
            address,
            server: Some(server),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for HealthStub {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

/// Reserves a port and immediately releases it, so a connection to the
/// returned address is refused rather than hanging until
/// `arclain_network::PROBE_TIMEOUT`.
fn unused_local_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a port");
    let address = listener.local_addr().expect("read the reserved address");
    drop(listener);
    format!("http://{address}")
}

#[test]
fn test_gameta_connection_succeeds_against_a_healthy_server() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = HealthStub::start();

    foreign_runtime()
        .block_on(app.test_gameta_connection(stub.url(), Some("probe-key".to_string())))
        .expect("a healthy server must report Ok");
}

/// The candidate values are a *probe*, not a save: nothing about them may
/// reach the settings snapshot or the encrypted vault.
#[test]
fn test_gameta_connection_persists_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = HealthStub::start();
    let runtime = foreign_runtime();

    let before = runtime.block_on(app.settings()).expect("read settings");
    runtime
        .block_on(app.test_gameta_connection(stub.url(), Some("probe-key".to_string())))
        .expect("a healthy server must report Ok");
    let after = runtime.block_on(app.settings()).expect("re-read settings");

    assert_eq!(before.revision, after.revision);
    assert_eq!(
        before.network.gameta_server_url,
        after.network.gameta_server_url,
    );
    assert_eq!(
        before.network.gameta_server_enabled,
        after.network.gameta_server_enabled,
    );
    assert_eq!(
        before.network.gameta_api_key_configured,
        after.network.gameta_api_key_configured,
    );
    assert!(
        !after.network.gameta_api_key_configured,
        "the probe key was stored as if it had been saved",
    );
}

#[test]
fn test_gameta_connection_reports_an_unreachable_server_without_leaking_the_api_key() {
    const API_KEY: &str = "gameta-probe-api-key-7b3e";
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let url = unused_local_url();

    let error = foreign_runtime()
        .block_on(app.test_gameta_connection(url, Some(API_KEY.to_string())))
        .expect_err("an unreachable server must not report Ok");

    let rendered = format!("{error:?}");
    assert!(
        !rendered.contains(API_KEY),
        "the API key reached the error surface: {rendered}",
    );
    assert_eq!(error.kind, ApplicationErrorKind::Backend);
    assert!(error.retryable);
    assert_eq!(error.field.as_deref(), Some("server_url"));
}

#[test]
fn test_gameta_connection_rejects_a_blank_url_before_any_request() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = foreign_runtime()
        .block_on(app.test_gameta_connection("   ".to_string(), None))
        .expect_err("a blank URL must be rejected");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("server_url"));
}

// ---------------------------------------------------------------------------
// analyze_url
// ---------------------------------------------------------------------------

/// The free function is reachable from outside the crate at both the
/// documented crate-root path and through `plugins`, and needs no
/// application instance at all.
#[test]
fn analyze_url_is_usable_without_an_application() {
    let analysis = arclain_app::analyze_url("https://secure-login.google.com.evil.tk/verify")
        .expect("analysis must succeed");

    assert_eq!(analysis.effective_domain, "evil.tk");
    assert!(!analysis.warnings.is_empty());
    assert!(analysis
        .warnings
        .iter()
        .all(|warning| !warning.description().is_empty()));

    let through_module = arclain_app::plugins::analyze_url("https://dlsite.com/product/123")
        .expect("analysis must succeed");
    assert!(through_module.warnings.is_empty());
}
