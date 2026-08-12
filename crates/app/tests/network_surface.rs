//! Integration tests for the boundary-zero network surface:
//! `ArclainApp::plugin_domain_whitelist`, `ArclainApp::
//! test_gameta_connection`, `ArclainApp::probe_network`, and the free
//! function `arclain_app::analyze_url`.
//!
//! These exist so a frontend never needs `arclain-network` as a direct
//! dependency to render a plugin's domain-access section, or to test a
//! gameta server or a proxy the user just typed into a settings form. The
//! unit tests in `crates/app/src/plugins.rs` and
//! `crates/app/src/runtime/settings_ops.rs` cover the pure mirroring,
//! filtering, and redaction in isolation; this file's job is proving the
//! wiring behind the public API against a real bootstrap -- in particular
//! that `plugin_domain_whitelist` reads the *live* whitelist this
//! application composed (not a parallel copy), that neither probe writes
//! anything anywhere, and that `probe_network`'s two modes really do take
//! different paths.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! following this crate's established convention (see
//! `archive_sessions.rs`'s own module doc comment for why): `ArclainApp`
//! owns its own Tokio runtime, and dropping it must not happen from
//! inside an async context.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use arclain_app::challenge::SecretInput;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::settings::{NetworkProbeReport, Socks5Candidate};
use arclain_app::{
    inspect_legacy_network_settings, ArclainApp, BootstrapConfig, PreparedPluginNetworkRouting,
};

/// Captures `tracing` output from **every** thread for the lifetime of
/// this test binary, so a redaction test can prove a secret reached
/// neither an `ApplicationError` nor a log line.
///
/// A global subscriber, not `tracing-test`'s `#[traced_test]`: that macro
/// installs a *thread-local* scoped default, and every facade method does
/// its real work on the application's own runtime worker threads. A
/// thread-local capture therefore sees nothing a facade method logs --
/// verified by injecting a `tracing::error!` leak into
/// `run_test_gameta_connection` and watching a `#[traced_test]` version of
/// the guard below stay green. A test that cannot fail is worse than no
/// test, so this captures globally instead.
mod tracing_capture {
    use std::io;
    use std::sync::{Arc, Mutex, OnceLock};

    /// The shared sink every captured event is written into. Cloneable so
    /// the subscriber and the assertions share one buffer.
    #[derive(Clone, Default)]
    pub struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        /// Whether `needle` appears anywhere in what has been logged so
        /// far. Process-wide by construction (one global subscriber, tests
        /// running in parallel), which is exactly right for a
        /// "this marker must never appear anywhere" assertion and is why
        /// every caller uses a unique marker string.
        pub fn contains(&self, needle: &str) -> bool {
            let buffer = self.0.lock().expect("captured log buffer poisoned");
            String::from_utf8_lossy(&buffer).contains(needle)
        }
    }

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut buffer = self.0.lock().expect("captured log buffer poisoned");
            buffer.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Installs the global subscriber the first time it is called and
    /// returns the shared buffer. Idempotent: a second call reuses the
    /// same buffer rather than fighting over the global default.
    pub fn install() -> Captured {
        static CAPTURED: OnceLock<Captured> = OnceLock::new();
        CAPTURED
            .get_or_init(|| {
                let captured = Captured::default();
                let subscriber = tracing_subscriber::fmt()
                    .with_writer(captured.clone())
                    .with_max_level(tracing::Level::TRACE)
                    .with_ansi(false)
                    .finish();
                // Ignored on failure: another test binary component may
                // legitimately have installed one first, and the
                // assertions below are "must not contain", which a
                // narrower capture can only make stricter.
                let _ = tracing::subscriber::set_global_default(subscriber);
                captured
            })
            .clone()
    }
}

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
        initial_plugin_network_routing: None,
    })
    .expect("bootstrap must succeed")
}

fn bootstrap_app_with_host_routing(
    temp: &tempfile::TempDir,
    initial_plugin_network_routing: PreparedPluginNetworkRouting,
) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: Some(initial_plugin_network_routing),
    })
    .expect("host-owned bootstrap must succeed")
}

fn seed_persisted_proxy(temp: &tempfile::TempDir) {
    let paths = support::temp_paths(temp.path());
    let db =
        arclain_core::config::ConfigDb::open(&support::databases_dir(&paths).join("config.sqlite"))
            .expect("open isolated config database");
    db.into_sqlite_db()
        .with_connection(|conn| {
            let mut config = arclain_core::UserConfig::load(conn)?.unwrap_or_default();
            config.socks5_enabled = true;
            config.socks5_address = Some("127.0.0.1:1080".to_string());
            config.save(conn)?;
            Ok(())
        })
        .expect("seed persisted proxy settings");
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
fn set_plugin_domain_approved_updates_the_live_and_persisted_whitelists() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    seed_whitelist(&app, |services| {
        services
            .domain_whitelist
            .read()
            .add_pending("mutable-plugin", "dlsite.example");
    });
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_plugin_domain_approved(
            "mutable-plugin".to_string(),
            "dlsite.example".to_string(),
            true,
        ))
        .expect("approving a requested domain must succeed");

    let entries = runtime
        .block_on(app.plugin_domain_whitelist("mutable-plugin".to_string()))
        .expect("read the live whitelist after approval");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].approved);

    let composition = app
        .take_legacy_composition()
        .expect("the transitional composition remains available as a test probe");
    let config_pool = &composition
        .dbs
        .as_ref()
        .expect("bootstrap must compose the config databases")
        .config_pool;
    assert!(
        config_pool
            .with_conn(|connection| {
                arclain_db::is_domain_approved(connection, "mutable-plugin", "dlsite.example")
            })
            .expect("read the persisted approval"),
        "the facade updated only the live network policy, not durable storage",
    );

    runtime
        .block_on(app.set_plugin_domain_approved(
            "mutable-plugin".to_string(),
            "dlsite.example".to_string(),
            false,
        ))
        .expect("revoking an approved domain must succeed");

    let entries = runtime
        .block_on(app.plugin_domain_whitelist("mutable-plugin".to_string()))
        .expect("read the live whitelist after revocation");
    assert!(!entries[0].approved);
    assert!(
        !config_pool
            .with_conn(|connection| {
                arclain_db::is_domain_approved(connection, "mutable-plugin", "dlsite.example")
            })
            .expect("read the persisted revocation"),
        "the facade updated only the live network policy, not durable storage",
    );
}

#[test]
fn set_plugin_domain_approved_rejects_invalid_or_unrequested_domains() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let blank_plugin = runtime
        .block_on(app.set_plugin_domain_approved(" ".to_string(), "example.test".to_string(), true))
        .expect_err("a blank plugin id must be rejected");
    assert_eq!(blank_plugin.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(blank_plugin.field.as_deref(), Some("plugin_id"));

    let blank_domain = runtime
        .block_on(app.set_plugin_domain_approved("demo".to_string(), " ".to_string(), true))
        .expect_err("a blank domain must be rejected");
    assert_eq!(blank_domain.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(blank_domain.field.as_deref(), Some("domain"));

    let unrequested = runtime
        .block_on(app.set_plugin_domain_approved(
            "demo".to_string(),
            "unrequested.example".to_string(),
            true,
        ))
        .expect_err("the facade must not grant a domain the plugin never requested");
    assert_eq!(unrequested.kind, ApplicationErrorKind::NotFound);
    assert_eq!(unrequested.field.as_deref(), Some("domain"));
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

/// The version a [`HealthStub`] reports, so a test can assert the probe
/// returns the server's own words rather than something invented.
const STUB_VERSION: &str = "9.9.9";

/// A single-request HTTP stub answering `GET /api/v1/health` with the
/// body `GametaClient::health` expects, then closing the connection.
///
/// Hand-rolled on `std::net::TcpListener` rather than pulled in as a mock
/// framework for the same reason `settings_facade.rs` hand-rolls its
/// SOCKS5 sentinel: this crate's tests are synchronous `#[test]`s with no
/// ambient async runtime, and the surface under test needs exactly one
/// well-formed response.
///
/// The join handle is returned rather than joined from a `Drop` impl: an
/// untimed `accept()` joined during unwind turns any probe failure into a
/// hung test binary instead of a red test. A test that wants to confirm
/// the stub served its request joins explicitly; a panicking test just
/// detaches the thread and lets the process reap it.
struct HealthStub {
    address: SocketAddr,
    server: std::thread::JoinHandle<()>,
}

impl HealthStub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the health stub");
        let address = listener.local_addr().expect("read the stub address");
        let server = std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
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
            let body = format!(r#"{{"status":"ok","version":"{STUB_VERSION}"}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
        });
        Self { address, server }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn join(self) {
        self.server.join().expect("health stub thread panicked");
    }
}

/// Reserves a port and immediately releases it, so a connection to the
/// returned address is refused rather than hanging until
/// `arclain_network::PROBE_TIMEOUT`.
fn unused_local_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a port");
    let address = listener.local_addr().expect("read the reserved address");
    drop(listener);
    address
}

fn unused_local_url() -> String {
    format!("http://{}", unused_local_address())
}

/// A throwaway candidate key for probes whose assertions are not about
/// the key itself.
fn probe_key() -> SecretInput {
    SecretInput::new("probe-key".to_string())
}

/// Success carries the server's own health-body facts back verbatim --
/// the settings page displays the version, so losing it would change what
/// the user reads.
#[test]
fn test_gameta_connection_returns_the_servers_own_health_facts() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let stub = HealthStub::start();

    let info = foreign_runtime()
        .block_on(app.test_gameta_connection(stub.url(), Some(probe_key())))
        .expect("a healthy server must report Ok");

    assert_eq!(info.version, STUB_VERSION);
    assert_eq!(info.status, "ok");
    stub.join();
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
        .block_on(app.test_gameta_connection(stub.url(), Some(probe_key())))
        .expect("a healthy server must report Ok");
    stub.join();
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

/// Covers **both** sinks a secret can escape through: the returned error
/// and anything this crate logged while producing it. `crates/ui`'s
/// traced tests cannot stand in for the second half -- they capture only
/// events their own test binary's thread emits, so an `arclain_app` leak
/// on a runtime worker thread is invisible to them.
#[test]
fn test_gameta_connection_reports_an_unreachable_server_without_leaking_the_api_key() {
    const API_KEY: &str = "gameta-probe-api-key-7b3e";
    let logs = tracing_capture::install();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let url = unused_local_url();

    let error = foreign_runtime()
        .block_on(app.test_gameta_connection(url, Some(SecretInput::new(API_KEY.to_string()))))
        .expect_err("an unreachable server must not report Ok");

    let rendered = format!("{error:?}");
    assert!(
        !rendered.contains(API_KEY),
        "the API key reached the error surface: {rendered}",
    );
    assert!(
        !logs.contains(API_KEY),
        "the API key reached this crate's own tracing output",
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
// probe_network
// ---------------------------------------------------------------------------

/// Accepts one connection and answers the SOCKS5 greeting with "no
/// acceptable methods" (`0x05 0xFF`), which is enough to prove the probe
/// reached the proxy and to make the handshake fail deterministically
/// without implementing a real SOCKS5 server. Mirrors
/// `settings_facade.rs`'s own `serve_proxy_sentinel` in spirit.
fn serve_rejecting_socks5(listener: TcpListener) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
        let mut greeting = [0_u8; 32];
        let _ = socket.read(&mut greeting);
        let _ = socket.write_all(&[0x05, 0xFF]);
        let _ = socket.flush();
    })
}

fn candidate_at(address: SocketAddr) -> Socks5Candidate {
    Socks5Candidate {
        host: address.ip().to_string(),
        port: address.port(),
        username: None,
        password: None,
    }
}

fn step_names(report: &NetworkProbeReport) -> Vec<&str> {
    report.steps.iter().map(|step| step.name.as_str()).collect()
}

/// The failure path, and the guard that the candidate password reaches
/// neither the report nor this crate's tracing. Global capture for the
/// same reason the gameta twin above uses it: `ProxyConfig::
/// test_connection` logs from a runtime worker thread.
#[test]
fn probe_network_reports_a_rejecting_proxy_without_leaking_the_password() {
    const PASSWORD: &str = "socks5-probe-password-5c1b";
    const USERNAME: &str = "socks5-probe-user-2e9f";
    let logs = tracing_capture::install();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the proxy sentinel");
    let address = listener.local_addr().expect("read the sentinel address");
    let sentinel = serve_rejecting_socks5(listener);

    let report = foreign_runtime()
        .block_on(app.probe_network(Some(Socks5Candidate {
            host: address.ip().to_string(),
            port: address.port(),
            username: Some(USERNAME.to_string()),
            password: Some(SecretInput::new(PASSWORD.to_string())),
        })))
        .expect("a probe that ran reports its trace, not an error");

    assert!(
        !report.succeeded(),
        "a proxy that refuses every auth method must not report success",
    );
    let rendered = format!("{report:?}");
    assert!(
        !rendered.contains(PASSWORD),
        "the proxy password reached the report: {rendered}",
    );
    assert!(
        !logs.contains(PASSWORD),
        "the proxy password reached this crate's own tracing output",
    );
    let _ = sentinel.join();
}

/// A probe that ran returns `Ok` with the trace, whatever it found. The
/// panel needs the per-step detail, which an `Err` carrying one summary
/// string cannot express.
#[test]
fn probe_network_returns_the_step_trace_for_a_failed_proxy_probe() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let address = unused_local_address();

    let report = foreign_runtime()
        .block_on(app.probe_network(Some(candidate_at(address))))
        .expect("a probe against a closed port still ran");

    assert!(!report.succeeded());
    // Proxy mode resolves and connects to the proxy first, so a closed
    // port gets as far as the TCP step and no further.
    assert_eq!(step_names(&report), vec!["DNS", "TCP"]);
    let dns = &report.steps[0];
    assert!(dns.passed);
    assert!(
        dns.message
            .as_deref()
            .unwrap_or_default()
            .contains("Resolved to"),
        "{dns:?}",
    );
    let tcp = &report.steps[1];
    assert!(!tcp.passed);
    assert!(tcp.message.is_some(), "a failed step must say why");
    assert_eq!(report.ip, None);
    assert_eq!(report.country, None);
}

/// Mode selection is the whole point of `proxy: Option<_>`: `Some` walks
/// the proxy's own DNS and TCP steps first, `None` skips straight to the
/// direct request. Asserted through the step trace rather than a
/// successful round trip, which would need a real proxy *and* the
/// third-party endpoint `ProxyConfig::test_connection` targets.
#[test]
fn probe_network_routes_direct_and_proxy_modes_down_different_paths() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();
    let address = unused_local_address();

    let through_proxy = runtime
        .block_on(app.probe_network(Some(candidate_at(address))))
        .expect("the proxy probe ran");
    assert_eq!(step_names(&through_proxy), vec!["DNS", "TCP"]);

    let direct = runtime
        .block_on(app.probe_network(None))
        .expect("the direct probe ran");
    assert!(
        !step_names(&direct).contains(&"DNS") && !step_names(&direct).contains(&"TCP"),
        "the direct path must not probe a proxy's DNS or TCP: {:?}",
        step_names(&direct),
    );
    // Whether the direct probe reaches the internet is not this test's
    // business; what it must never do is report a SOCKS5 step.
    assert!(
        !step_names(&direct).contains(&"SOCKS5"),
        "the direct path reported a SOCKS5 step: {:?}",
        step_names(&direct),
    );
}

#[test]
fn probe_network_persists_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let address = unused_local_address();
    let runtime = foreign_runtime();

    let before = runtime.block_on(app.settings()).expect("read settings");
    let _ = runtime.block_on(app.probe_network(Some(Socks5Candidate {
        host: address.ip().to_string(),
        port: address.port(),
        username: Some("candidate-user".to_string()),
        password: Some(SecretInput::new("candidate-password".to_string())),
    })));
    let after = runtime.block_on(app.settings()).expect("re-read settings");

    assert_eq!(before.revision, after.revision);
    assert_eq!(before.network.socks5_enabled, after.network.socks5_enabled);
    assert_eq!(before.network.socks5_address, after.network.socks5_address);
    assert_eq!(
        before.network.socks5_username,
        after.network.socks5_username
    );
    assert!(
        !after.network.socks5_password_configured,
        "the candidate password was stored as if it had been saved",
    );
}

/// The one shape that is an `Err`: a candidate that could never have been
/// probed at all, rejected before any packet leaves.
#[test]
fn probe_network_rejects_an_unusable_host_and_port_before_any_packet() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let blank = runtime
        .block_on(app.probe_network(Some(Socks5Candidate {
            host: String::new(),
            port: 1080,
            username: None,
            password: None,
        })))
        .expect_err("a blank host must be rejected");
    assert_eq!(blank.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(blank.field.as_deref(), Some("host"));

    let zero_port = runtime
        .block_on(app.probe_network(Some(Socks5Candidate {
            host: "127.0.0.1".to_string(),
            port: 0,
            username: None,
            password: None,
        })))
        .expect_err("port 0 must be rejected");
    assert_eq!(zero_port.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(zero_port.field.as_deref(), Some("port"));

    // An authority the network crate itself refuses (embedded userinfo)
    // must not reach the wire either.
    let malformed = runtime
        .block_on(app.probe_network(Some(Socks5Candidate {
            host: "user@proxy.invalid".to_string(),
            port: 1080,
            username: None,
            password: None,
        })))
        .expect_err("a host carrying userinfo must be rejected");
    assert_eq!(malformed.kind, ApplicationErrorKind::InvalidInput);
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

#[test]
fn host_owned_none_preserves_standalone_persisted_plugin_routing() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    seed_persisted_proxy(&temp);

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: None,
    })
    .expect("standalone bootstrap must succeed");
    let legacy = app.take_legacy_composition().expect("legacy composition");

    assert!(
        legacy
            .core_services
            .async_http_client
            .should_use_proxy_for_plugin("dlsite"),
        "standalone bootstrap must keep applying the persisted proxy"
    );
}

#[test]
fn host_owned_direct_overrides_a_legacy_persisted_proxy() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    seed_persisted_proxy(&temp);
    let prepared = ArclainApp::prepare_plugin_network_routing(
        None,
        BTreeMap::from([("dlsite".to_string(), false)]),
    )
    .expect("host direct routing must prepare");

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: Some(prepared),
    })
    .expect("host-owned bootstrap must succeed");
    let legacy = app.take_legacy_composition().expect("legacy composition");

    assert!(
        !legacy
            .core_services
            .async_http_client
            .should_use_proxy_for_plugin("dlsite"),
        "persisted standalone proxy settings must not replace host-owned direct routing"
    );
}

#[test]
fn host_owned_proxy_routes_only_the_selected_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").expect("bind proxy candidate");
    let prepared = ArclainApp::prepare_plugin_network_routing(
        Some(candidate_at(proxy.local_addr().expect("proxy address"))),
        BTreeMap::from([("host-proxied".to_string(), true)]),
    )
    .expect("host proxy routing must prepare");
    let app = bootstrap_app_with_host_routing(&temp, prepared);
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let client = &legacy.core_services.async_http_client;

    assert!(client.should_use_proxy_for_plugin("host-proxied"));
    assert!(!client.should_use_proxy_for_plugin("host-direct"));
}

#[test]
fn host_owned_proxy_required_without_a_proxy_fails_closed() {
    const PLUGIN_ID: &str = "host-proxy-required";
    const PUBLIC_IP: &str = "93.184.216.34";
    let temp = tempfile::tempdir().unwrap();
    let prepared = ArclainApp::prepare_plugin_network_routing(
        None,
        BTreeMap::from([(PLUGIN_ID.to_string(), true)]),
    )
    .expect("a proxy-required policy without a proxy must prepare as fail-closed");
    let app = bootstrap_app_with_host_routing(&temp, prepared);
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let client = legacy.core_services.async_http_client.clone();
    client.configure_plugin(
        PLUGIN_ID,
        arclain_network::PluginNetworkPolicy {
            network_enabled: true,
            requests_per_minute: 60,
        },
    );
    legacy
        .core_services
        .domain_whitelist
        .write()
        .approve(PLUGIN_ID, PUBLIC_IP);

    let request = client
        .request_for_plugin(
            PLUGIN_ID,
            arclain_network::HttpRequest::get(format!("http://{PUBLIC_IP}:9/unreachable")),
        )
        .expect("the configured checked request must start");
    let status = foreign_runtime().block_on(client.await_complete(&request));

    assert!(
        matches!(
            status,
            Some(arclain_network::RequestStatus::Failed(ref message))
                if message.contains("configured to use a proxy")
        ),
        "proxy-required routing did not fail closed: {status:?}"
    );
}

// ---------------------------------------------------------------------------
// Static legacy-network inspection
// ---------------------------------------------------------------------------

fn legacy_config_path(profile: &Path) -> PathBuf {
    profile.join("databases").join("config.sqlite")
}

fn legacy_secrets_path(profile: &Path) -> PathBuf {
    profile.join("secrets").join("pass.redb")
}

fn seed_legacy_network_config(profile: &Path, plugin_proxy_settings: &str) {
    let path = legacy_config_path(profile);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = arclain_core::config::ConfigDb::open(&path).expect("create config database");
    db.into_sqlite_db()
        .with_connection(|conn| {
            arclain_core::UserConfig::ensure_table(conn)?;
            let mut config = arclain_core::UserConfig::new();
            config.socks5_enabled = true;
            config.socks5_address = Some("127.0.0.1:1080".to_string());
            config.socks5_username = Some("legacy-user".to_string());
            config.plugin_proxy_settings = Some(plugin_proxy_settings.to_string());
            config.save(conn)?;
            Ok(())
        })
        .expect("seed legacy network settings");
}

fn seed_legacy_socks5_password(profile: &Path) {
    let path = legacy_secrets_path(profile);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = arclain_core::SecretsDb::open(&path, &[0x55; 32]).expect("create secrets database");
    db.set_secret("proxy:socks5", "never expose this password")
        .expect("seed fixed SOCKS5 secret");
    db.close();
}

fn profile_hashes(root: &Path) -> BTreeMap<PathBuf, u32> {
    fn visit(root: &Path, dir: &Path, hashes: &mut BTreeMap<PathBuf, u32>) {
        let mut entries = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, hashes);
            } else {
                hashes.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    crc32fast::hash(&fs::read(&path).unwrap()),
                );
            }
        }
    }

    let mut hashes = BTreeMap::new();
    if root.is_dir() {
        visit(root, root, &mut hashes);
    }
    hashes
}

fn sqlite_source_hashes(profile: &Path) -> BTreeMap<&'static str, Option<(u64, u32)>> {
    let config = legacy_config_path(profile);
    [
        ("config.sqlite", config.clone()),
        (
            "config.sqlite-wal",
            config.with_file_name("config.sqlite-wal"),
        ),
        (
            "config.sqlite-shm",
            config.with_file_name("config.sqlite-shm"),
        ),
    ]
    .into_iter()
    .map(|(name, path)| {
        let fingerprint = fs::read(path)
            .ok()
            .map(|bytes| (bytes.len() as u64, crc32fast::hash(&bytes)));
        (name, fingerprint)
    })
    .collect()
}

#[test]
fn static_legacy_inspection_is_pure_for_missing_and_valid_profiles() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-profile");
    assert!(inspect_legacy_network_settings(&missing)
        .expect("missing profile is absence")
        .is_none());
    assert!(!missing.exists());

    let profile = temp.path().join("valid-profile");
    seed_legacy_network_config(&profile, r#"{"dlsite":false,"custom":true}"#);
    seed_legacy_socks5_password(&profile);
    let before = profile_hashes(&profile);
    let sqlite_before = sqlite_source_hashes(&profile);
    assert!(sqlite_before["config.sqlite"].is_some());
    assert_eq!(sqlite_before["config.sqlite-wal"], None);
    assert_eq!(sqlite_before["config.sqlite-shm"], None);

    let inspected = inspect_legacy_network_settings(&profile)
        .expect("inspect valid legacy profile")
        .expect("valid user_config row exists");

    assert!(inspected.socks5_enabled);
    assert_eq!(inspected.socks5_address.as_deref(), Some("127.0.0.1:1080"));
    assert_eq!(inspected.socks5_username.as_deref(), Some("legacy-user"));
    assert!(inspected.socks5_password_configured);
    assert!(!format!("{inspected:?}").contains("never expose this password"));
    assert_eq!(
        inspected.plugin_proxy_enabled,
        BTreeMap::from([("custom".to_string(), true), ("dlsite".to_string(), false)])
    );
    assert_eq!(profile_hashes(&profile), before);
    assert_eq!(sqlite_source_hashes(&profile), sqlite_before);
}

#[test]
fn malformed_plugin_proxy_and_corrupt_config_are_bounded_backend_errors() {
    let temp = tempfile::tempdir().unwrap();
    let malformed = temp.path().join("malformed-profile");
    seed_legacy_network_config(&malformed, "not-json");
    let malformed_error = inspect_legacy_network_settings(&malformed)
        .expect_err("malformed plugin proxy JSON must not be defaulted away");
    assert_eq!(malformed_error.kind, ApplicationErrorKind::Backend);
    assert!(malformed_error.diagnostic.unwrap().len() <= 4096);

    let corrupt = temp.path().join("corrupt-profile");
    let corrupt_path = legacy_config_path(&corrupt);
    fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
    fs::write(&corrupt_path, b"not sqlite").unwrap();
    let before = profile_hashes(&corrupt);
    let corrupt_error =
        inspect_legacy_network_settings(&corrupt).expect_err("corrupt config database must fail");
    assert_eq!(corrupt_error.kind, ApplicationErrorKind::Backend);
    assert_eq!(profile_hashes(&corrupt), before);
}

#[test]
fn absent_secret_storage_keeps_otherwise_valid_legacy_settings() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    seed_legacy_network_config(&profile, "{}");

    let inspected = inspect_legacy_network_settings(&profile)
        .expect("inspect without secrets database")
        .expect("valid config row exists");

    assert!(inspected.socks5_enabled);
    assert!(!inspected.socks5_password_configured);
    assert!(!profile.join("secrets").exists());
}

#[test]
fn missing_user_config_table_or_row_is_absence_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let no_table = temp.path().join("no-table-profile");
    let no_table_path = legacy_config_path(&no_table);
    fs::create_dir_all(no_table_path.parent().unwrap()).unwrap();
    let database = arclain_db::DbConnection::open(&no_table_path).expect("create SQLite database");
    drop(database);
    let before = sqlite_source_hashes(&no_table);
    assert!(inspect_legacy_network_settings(&no_table)
        .expect("missing user_config table is absence")
        .is_none());
    assert_eq!(sqlite_source_hashes(&no_table), before);

    let no_row = temp.path().join("no-row-profile");
    seed_legacy_network_config(&no_row, "{}");
    let no_row_path = legacy_config_path(&no_row);
    let database = arclain_db::DbConnection::open(&no_row_path).expect("open SQLite database");
    database
        .execute("DELETE FROM user_config WHERE id=1", [])
        .expect("remove singleton row");
    drop(database);
    let before = sqlite_source_hashes(&no_row);
    assert!(inspect_legacy_network_settings(&no_row)
        .expect("missing user_config row is absence")
        .is_none());
    assert_eq!(sqlite_source_hashes(&no_row), before);
}

#[test]
fn committed_wal_and_sidecars_are_read_without_creation_or_mutation() {
    const CRASH_FIXTURE_PROFILE: &str = "ARCLAIN_CRASH_WAL_FIXTURE_PROFILE";
    if let Some(profile) = std::env::var_os(CRASH_FIXTURE_PROFILE) {
        let profile = PathBuf::from(profile);
        seed_legacy_network_config(&profile, "{}");
        let config_path = legacy_config_path(&profile);
        let writer = arclain_db::DbConnection::open(&config_path).expect("open WAL writer");
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;\
                 PRAGMA wal_autocheckpoint=0;\
                 UPDATE user_config SET socks5_address='wal-proxy:1080' WHERE id=1;",
            )
            .expect("commit network setting into WAL");
        std::process::exit(0);
    }

    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("wal-profile");
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "committed_wal_and_sidecars_are_read_without_creation_or_mutation",
            "--nocapture",
        ])
        .env(CRASH_FIXTURE_PROFILE, &profile)
        .status()
        .expect("launch crash-style committed-WAL fixture writer");
    assert!(status.success(), "fixture writer failed: {status}");
    let config_path = legacy_config_path(&profile);
    let wal_path = config_path.with_file_name("config.sqlite-wal");
    let shm_path = config_path.with_file_name("config.sqlite-shm");
    assert!(wal_path.is_file() && fs::metadata(&wal_path).unwrap().len() > 0);
    assert!(shm_path.is_file());
    let before = profile_hashes(&profile);
    let sqlite_before = sqlite_source_hashes(&profile);
    assert!(sqlite_before.values().all(Option::is_some));

    let inspected = inspect_legacy_network_settings(&profile)
        .expect("read committed WAL state")
        .expect("valid user_config row exists");

    assert_eq!(inspected.socks5_address.as_deref(), Some("wal-proxy:1080"));
    assert_eq!(profile_hashes(&profile), before);
    assert_eq!(sqlite_source_hashes(&profile), sqlite_before);

    let missing_shm_profile = temp.path().join("wal-without-shm-profile");
    let missing_shm_config = legacy_config_path(&missing_shm_profile);
    fs::create_dir_all(missing_shm_config.parent().unwrap()).unwrap();
    fs::copy(&config_path, &missing_shm_config).expect("copy SQLite source");
    fs::copy(
        &wal_path,
        missing_shm_config.with_file_name("config.sqlite-wal"),
    )
    .expect("copy committed WAL without SHM");
    let missing_shm_before = sqlite_source_hashes(&missing_shm_profile);
    assert!(missing_shm_before["config.sqlite"].is_some());
    assert!(missing_shm_before["config.sqlite-wal"].is_some());
    assert_eq!(missing_shm_before["config.sqlite-shm"], None);

    let error = inspect_legacy_network_settings(&missing_shm_profile)
        .expect_err("WAL without an existing SHM must fail closed");
    assert_eq!(error.kind, ApplicationErrorKind::Backend);
    assert_eq!(
        sqlite_source_hashes(&missing_shm_profile),
        missing_shm_before
    );
}
