//! Integration tests for the settings/secrets/vault facade surface
//! (`ArclainApp::settings`/`gameta_connection_status`/`update_settings`/
//! `set_gameta_api_key`/`set_socks5_password`/`move_vault`/`rekey_vault`/
//! `password_rules`/`upsert_password_rule`/`delete_password_rule`).
//!
//! The organization surface (`organization_profiles`/`organization_rules`
//! and their CRUD, plus `preview_organize_plan`) has its own file,
//! `organization_facade.rs` -- archive profiles are reachable through
//! `arclain_app::settings` only as a re-export.
//!
//! `crates/app/src/settings.rs`'s own unit tests cover `PatchValue`
//! application and DTO conversion in isolation (pure functions, no I/O);
//! this file's job is proving those pieces are wired together correctly
//! behind the public API against a real bootstrap -- real SQLite/redb
//! files in a temp profile, the same way `archive_sessions.rs`/
//! `processing_operations.rs` already do for their own surfaces.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! following this crate's established convention (see
//! `archive_sessions.rs`'s own module doc comment for why): `ArclainApp`
//! owns its own Tokio runtime, and dropping it must not happen from
//! inside an async context.

mod support;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arclain_app::challenge::SecretInput;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::settings::{
    ArchiveSettingsPatch, BackendModeDto, GametaConnectionStatusDto, NetworkSettingsPatch,
    PasswordRuleEditInput, PasswordRuleInput, PatchValue, SecuritySettingsPatch, SettingsPatch,
};
use arclain_app::{ArclainApp, BootstrapConfig};

/// A minimal SOCKS5 server that only implements enough of RFC 1928/1929
/// to observe which username/password a client authenticates with, then
/// stops -- the CONNECT step that would normally follow the auth
/// handshake is deliberately never answered; capturing the credentials
/// is this sentinel's whole job. Mirrors `settings_controller.rs`'s own
/// `serve_proxy_sentinel` in spirit (accept, observe, respond minimally)
/// but implements the actual auth subnegotiation instead of an
/// immediate rejection, since this needs to see the credential bytes.
fn capture_socks5_credentials(
    proxy: TcpListener,
    request_finished: Arc<AtomicBool>,
    captured: Arc<Mutex<Option<(String, String)>>>,
) {
    while !request_finished.load(Ordering::SeqCst) {
        match proxy.accept() {
            Ok((mut socket, _)) => {
                // The accepted socket inherits non-blocking mode from
                // the listener on Windows (unlike POSIX, where it
                // defaults to blocking) -- switch it back so the
                // `read_exact` calls below wait for bytes instead of
                // returning `WouldBlock` immediately.
                let _ = socket.set_nonblocking(false);
                let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
                if try_capture_one_handshake(&mut socket, &captured).is_some() {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return,
        }
    }
}

fn try_capture_one_handshake(
    socket: &mut std::net::TcpStream,
    captured: &Arc<Mutex<Option<(String, String)>>>,
) -> Option<()> {
    // Method negotiation request: VER(1) NMETHODS(1) METHODS(NMETHODS).
    let mut header = [0u8; 2];
    socket.read_exact(&mut header).ok()?;
    let n_methods = header[1] as usize;
    let mut methods = vec![0u8; n_methods];
    socket.read_exact(&mut methods).ok()?;
    // Select username/password auth (0x02) unconditionally.
    socket.write_all(&[0x05, 0x02]).ok()?;

    // Username/password subnegotiation (RFC 1929):
    // VER(1) ULEN(1) UNAME(ULEN) PLEN(1) PASSWD(PLEN).
    let mut sub_header = [0u8; 2];
    socket.read_exact(&mut sub_header).ok()?;
    let ulen = sub_header[1] as usize;
    let mut uname = vec![0u8; ulen];
    socket.read_exact(&mut uname).ok()?;
    let mut plen_buf = [0u8; 1];
    socket.read_exact(&mut plen_buf).ok()?;
    let plen = plen_buf[0] as usize;
    let mut passwd = vec![0u8; plen];
    socket.read_exact(&mut passwd).ok()?;

    let username = String::from_utf8_lossy(&uname).into_owned();
    let password = String::from_utf8_lossy(&passwd).into_owned();
    *captured.lock().unwrap() = Some((username, password));
    // Report auth success so the client doesn't abort immediately --
    // the CONNECT request that follows is never answered; this sentinel
    // only needs to observe the auth handshake.
    let _ = socket.write_all(&[0x01, 0x00]);
    Some(())
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
/// `archive_sessions.rs::bootstrap_app`'s doc comment for why the dummy
/// 7-Zip is seeded even though no test here touches an archive backend
/// at all: it makes the resolved-tool half of `capabilities()`/
/// `health()` deterministic regardless of what the machine running the
/// test has installed. Bootstrap itself no longer depends on it.
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

/// Copies the maintained UI-demo component into the folder layout the real
/// plugin loader discovers. The settings tests deliberately use this real
/// instance so a stale compare-and-set must leave both its live host settings
/// and the persisted row unchanged.
fn install_ui_demo_fixture(plugins_dir: &Path) {
    let plugin_dir = plugins_dir.join("ui-demo");
    std::fs::create_dir_all(&plugin_dir).expect("create UI-demo fixture directory");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../wirt/tests/fixtures/bundled/ui-demo.wasm"),
        plugin_dir.join("ui-demo.wasm"),
    )
    .expect("copy UI-demo component");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/ui-demo/plugin.toml"),
        plugin_dir.join("ui-demo.toml"),
    )
    .expect("copy UI-demo manifest");
}

fn bootstrap_app_with_ui_demo(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    install_ui_demo_fixture(&paths.plugins_dir);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with UI-demo fixture must succeed")
}

fn rebootstrap_app_with_ui_demo(temp: &tempfile::TempDir) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(support::temp_paths(temp.path())),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("rebootstrap with UI-demo fixture must succeed")
}

fn keep_archive_patch() -> ArchiveSettingsPatch {
    ArchiveSettingsPatch {
        backend_mode: PatchValue::Keep,
        cache_directory: PatchValue::Keep,
        temp_directory: PatchValue::Keep,
        transfer_directory: PatchValue::Keep,
        sevenzip_path: PatchValue::Keep,
        default_collision_policy: PatchValue::Keep,
    }
}

fn keep_network_patch() -> NetworkSettingsPatch {
    NetworkSettingsPatch {
        socks5_enabled: PatchValue::Keep,
        socks5_address: PatchValue::Keep,
        socks5_username: PatchValue::Keep,
        plugin_proxy_enabled: PatchValue::Keep,
        gameta_server_enabled: PatchValue::Keep,
        gameta_server_url: PatchValue::Keep,
    }
}

fn keep_security_patch() -> SecuritySettingsPatch {
    SecuritySettingsPatch {
        secrets_database_path: PatchValue::Keep,
        key_file_path: PatchValue::Keep,
        encrypted_crc_policy: PatchValue::Keep,
    }
}

// ============================================================================
// Revisioned plugin settings snapshots.
// ============================================================================

/// Catches a facade that persists a plugin-settings form but leaves the
/// running guest on its bootstrap-time values, or returns a private revision
/// unrelated to the application-wide settings revision.
#[test]
fn plugin_settings_reads_the_live_bounded_map_and_a_successful_cas_returns_a_fresh_snapshot() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);

    let initial = runtime
        .block_on(app.plugin_settings("ui-demo".to_string()))
        .expect("known plugin settings must be readable");
    assert_eq!(initial.plugin_id, "ui-demo");
    assert_eq!(initial.revision, 0);
    assert!(initial.values.is_empty());

    let values = BTreeMap::from([
        (
            "endpoint".to_string(),
            "https://example.test/api".to_string(),
        ),
        ("theme".to_string(), "dark".to_string()),
    ]);
    let updated = runtime
        .block_on(app.set_plugin_settings("ui-demo".to_string(), initial.revision, values.clone()))
        .expect("matching revision must persist plugin settings");

    assert_eq!(updated.plugin_id, "ui-demo");
    assert_eq!(updated.revision, 1);
    assert_eq!(updated.values, values);
    assert_eq!(
        runtime.block_on(app.settings()).unwrap().revision,
        updated.revision,
        "plugin settings must advance the shared application revision"
    );
    assert_eq!(
        runtime
            .block_on(app.plugin_settings("ui-demo".to_string()))
            .unwrap()
            .values,
        values,
        "the facade read must return the exact stored bounded map"
    );

    let legacy = app.take_legacy_composition().expect("legacy composition");
    let manager = legacy.plugin_manager.expect("UI-demo must be live").clone();
    let instance = manager
        .lock()
        .get_plugin_instance("ui-demo")
        .expect("live UI-demo instance");
    let live = instance
        .lock()
        .get_settings()
        .expect("live UI-demo settings");
    assert_eq!(
        BTreeMap::from_iter(live),
        BTreeMap::from([
            (
                "endpoint".to_string(),
                "https://example.test/api".to_string()
            ),
            ("theme".to_string(), "dark".to_string()),
        ]),
        "successful persistence must activate the same map in the live plugin"
    );
}

/// Catches a stale writer that overwrites the accepted settings either on disk
/// or in the running guest instance.
#[test]
fn stale_plugin_settings_revision_changes_neither_disk_nor_live_instance() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);

    let initial = runtime
        .block_on(app.plugin_settings("ui-demo".to_string()))
        .unwrap();
    let accepted = BTreeMap::from([("mode".to_string(), "accepted".to_string())]);
    let current = runtime
        .block_on(app.set_plugin_settings(
            "ui-demo".to_string(),
            initial.revision,
            accepted.clone(),
        ))
        .unwrap();

    let error = runtime
        .block_on(app.set_plugin_settings(
            "ui-demo".to_string(),
            initial.revision,
            BTreeMap::from([("mode".to_string(), "stale".to_string())]),
        ))
        .expect_err("a stale revision must be rejected");
    assert_eq!(error.kind, ApplicationErrorKind::Conflict);
    assert_eq!(error.field.as_deref(), Some("expected_revision"));

    assert_eq!(
        runtime
            .block_on(app.plugin_settings("ui-demo".to_string()))
            .unwrap(),
        current,
        "a stale write must not replace the facade snapshot"
    );
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let manager = legacy
        .plugin_manager
        .as_ref()
        .expect("UI-demo must be live")
        .clone();
    let instance = manager
        .lock()
        .get_plugin_instance("ui-demo")
        .expect("live UI-demo instance");
    let live = instance
        .lock()
        .get_settings()
        .expect("live UI-demo settings");
    assert_eq!(BTreeMap::from_iter(live), accepted);

    drop(legacy);
    runtime
        .block_on(app.shutdown())
        .expect("shutdown must succeed");
    drop(app);
    let restarted = rebootstrap_app_with_ui_demo(&temp);
    let after_restart = runtime
        .block_on(restarted.plugin_settings("ui-demo".to_string()))
        .unwrap();
    assert_eq!(
        after_restart.values, current.values,
        "a restart must observe the accepted settings, not the stale write"
    );
}

/// Catches validation that happens after persistence, or that silently lets
/// the plugin runtime truncate an over-bound map while reporting success.
#[test]
fn unknown_or_over_bound_plugin_settings_fail_before_persistence() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let initial = runtime
        .block_on(app.plugin_settings("ui-demo".to_string()))
        .unwrap();

    let unknown = runtime
        .block_on(app.set_plugin_settings(
            "not-installed".to_string(),
            initial.revision,
            BTreeMap::new(),
        ))
        .expect_err("unknown plugin ids must be rejected before persistence");
    assert_eq!(unknown.kind, ApplicationErrorKind::NotFound);
    assert_eq!(unknown.field.as_deref(), Some("plugin_id"));

    let over_bound = (0..=128)
        .map(|index| (format!("key-{index:03}"), "value".to_string()))
        .collect();
    let error = runtime
        .block_on(app.set_plugin_settings("ui-demo".to_string(), initial.revision, over_bound))
        .expect_err("the host setting bounds must reject an over-bound map");
    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("values"));

    assert_eq!(
        runtime
            .block_on(app.plugin_settings("ui-demo".to_string()))
            .unwrap(),
        initial,
        "validation failures must leave the saved snapshot untouched"
    );

    runtime
        .block_on(app.shutdown())
        .expect("shutdown must succeed");
    drop(app);
    let restarted = rebootstrap_app_with_ui_demo(&temp);
    assert_eq!(
        runtime
            .block_on(restarted.plugin_settings("ui-demo".to_string()))
            .unwrap()
            .values,
        initial.values,
        "unknown or over-bound requests must not persist a settings row"
    );
}

fn no_op_patch(expected_revision: u64) -> SettingsPatch {
    SettingsPatch {
        general: None,
        expected_revision,
        archive: None,
        network: None,
        security: None,
    }
}

/// Peeks at the raw stored password for a named rule directly through
/// `take_legacy_composition`'s encrypted secrets handle -- bypassing
/// `PasswordRuleSummary`'s deliberate redaction the same way
/// `support::seed_pass_rule` writes directly to `dbs.secrets`. Only ever
/// used from test code to prove *what value* survived a mutation; the
/// facade's own public surface never exposes this.
fn raw_pass_rule_password(app: &ArclainApp, name: &str) -> Option<String> {
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let dbs = legacy.dbs.expect("vault must be available");
    dbs.secrets
        .list_pass_rules()
        .expect("list raw pass rules")
        .into_iter()
        .find(|rule| rule.name == name)
        .map(|rule| rule.password)
}

/// Installs a `BEFORE INSERT ON user_config` trigger that aborts every
/// write, the same technique `crates/ui`'s pre-facade
/// `settings_controller.rs` test suite already uses (H4 regression /
/// `config_persistence_failure_rolls_back_secret_and_does_not_apply`) to
/// force a deterministic persistence failure without depending on OS-
/// level permission tricks.
fn install_failing_user_config_trigger(app: &ArclainApp) {
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let dbs = legacy.dbs.expect("vault must be available");
    dbs.config
        .with_connection(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER reject_user_config_save
                 BEFORE INSERT ON user_config
                 BEGIN
                     SELECT RAISE(ABORT, 'injected settings save failure');
                 END;",
            )?;
            Ok(())
        })
        .expect("install failing user_config trigger");
}

/// Sibling of [`install_failing_user_config_trigger`] targeting
/// `app_config` instead -- the plain key/value table `set_config`/
/// `get_config` use, which is what `persist_encrypted_crc_policy` and
/// `repoint_vault_paths`'s override-persisting step write through. A
/// trigger here fails only those two write steps, leaving the
/// `user_config` row write (a different table) unaffected -- needed to
/// isolate each of `update_settings`'s independent write steps for its
/// own forced-failure test (see the "I5" doc note in `settings_ops.rs`).
fn install_failing_app_config_trigger(app: &ArclainApp) {
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let dbs = legacy.dbs.expect("vault must be available");
    dbs.config
        .with_connection(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER reject_app_config_save
                 BEFORE INSERT ON app_config
                 BEGIN
                     SELECT RAISE(ABORT, 'injected app_config save failure');
                 END;",
            )?;
            Ok(())
        })
        .expect("install failing app_config trigger");
}

// ============================================================================
// First-run defaults.
// ============================================================================

#[test]
fn first_run_defaults_reflect_a_fresh_bootstrap() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    // `bootstrap_app` seeds `sevenzip_path` itself (see its own doc
    // comment) so 7-Zip detection succeeds deterministically -- that is
    // test-harness plumbing, not a "first run" default, so this reads
    // the same dummy path back to assert the DTO reflects whatever
    // `UserConfig` actually holds rather than asserting it away.
    let expected_sevenzip_path = dummy_sevenzip(&temp);
    let app = bootstrap_app(&temp);

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");

    assert_eq!(snapshot.revision, 0);
    assert_eq!(snapshot.archive.backend_mode, BackendModeDto::Native);
    assert!(snapshot.archive.cache_directory.is_none());
    assert!(snapshot.archive.temp_directory.is_none());
    assert!(snapshot.archive.transfer_directory.is_none());
    assert_eq!(
        snapshot.archive.sevenzip_path.as_deref(),
        Some(expected_sevenzip_path.as_path())
    );

    assert!(!snapshot.network.socks5_enabled);
    assert!(snapshot.network.socks5_address.is_none());
    assert!(snapshot.network.socks5_username.is_none());
    assert!(!snapshot.network.socks5_password_configured);
    assert!(snapshot.network.plugin_proxy_enabled.is_empty());
    assert!(!snapshot.network.gameta_server_enabled);
    assert!(snapshot.network.gameta_server_url.is_none());
    assert!(!snapshot.network.gameta_api_key_configured);

    // Nothing has ever written the `app_config` key, so the snapshot
    // reports `OutputCollisionPolicy`'s own default rather than an empty
    // string a frontend would have to interpret.
    assert_eq!(snapshot.archive.default_collision_policy, "smart");

    assert_eq!(snapshot.security.encrypted_crc_policy, "on_access");
    assert!(snapshot.security.vault_available);

    // Defaults belong to this bootstrapped profile too: a `Clear` must
    // never escape a caller-supplied `paths_override` and repoint the
    // instance at the machine-wide Arclain profile.
    let expected_secrets_dir = temp.path().join("data").join("secrets");
    let expected_secrets_db = expected_secrets_dir.join("pass.redb");
    let expected_key_file = expected_secrets_dir.join("master.key");
    assert_eq!(
        snapshot.security.default_secrets_database_path.as_deref(),
        Some(expected_secrets_db.as_path())
    );
    assert_eq!(
        snapshot.security.default_key_file_path.as_deref(),
        Some(expected_key_file.as_path())
    );
    assert_eq!(
        snapshot.security.secrets_database_path, snapshot.security.default_secrets_database_path,
        "a first-run profile must report its live vault as the reset target"
    );
    assert_eq!(
        snapshot.security.key_file_path, snapshot.security.default_key_file_path,
        "a first-run profile must report its live key as the reset target"
    );
    assert_eq!(
        snapshot
            .security
            .secrets_database_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str()),
        Some("pass.redb")
    );
    assert_eq!(
        snapshot
            .security
            .key_file_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str()),
        Some("master.key")
    );

    let rules = runtime
        .block_on(app.password_rules())
        .expect("password_rules must succeed");
    assert!(rules.is_empty());
}

// ============================================================================
// update_settings must never clobber columns written outside the facade.
// ============================================================================

/// Five still-unmigrated UI call sites (hotkeys, toolbar order, general
/// prefs, plugin settings) write `user_config` columns directly through
/// `ConfigService`, bypassing the facade entirely -- that migration is
/// tracked separately. `update_settings` must not silently revert those
/// columns back to whatever this instance last saw just because it only
/// knows how to patch archive/network/security fields: `UserConfig` is
/// one row, and a naive "patch my cached copy, write the whole row" a
/// pproach clobbers every column the cache doesn't know changed.
#[test]
fn update_settings_does_not_revert_columns_written_directly_via_config_service() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    // Simulate a still-unmigrated direct write (e.g. SaveKeyboardMouse)
    // that never goes through the facade at all.
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let config_service = legacy
        .core_services
        .config_service
        .clone()
        .expect("config service must be available");
    let mut direct = config_service
        .get_user_config()
        .expect("read user config directly");
    direct.hotkey_bindings = Some(r#"{"foo":"Ctrl+F"}"#.to_string());
    config_service
        .save_user_config(&direct)
        .expect("direct write must succeed");

    // An unrelated, facade-driven archive-only save.
    let current = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    let patch = SettingsPatch {
        general: None,
        expected_revision: current.revision,
        archive: Some(ArchiveSettingsPatch {
            cache_directory: PatchValue::Set(PathBuf::from("/cache/whatever")),
            ..keep_archive_patch()
        }),
        network: None,
        security: None,
    };
    runtime
        .block_on(app.update_settings(patch))
        .expect("update must succeed");

    // The directly-written column must survive on disk...
    let after_on_disk = config_service
        .get_user_config()
        .expect("read user config directly");
    assert_eq!(
        after_on_disk.hotkey_bindings.as_deref(),
        Some(r#"{"foo":"Ctrl+F"}"#),
        "update_settings reverted a column it never patched"
    );

    // ...and the facade's own cached view must have picked up the fresh
    // read too (not just left it correct on disk by accident while the
    // in-memory cache stays stale).
    let legacy_after = app.take_legacy_composition().expect("legacy composition");
    assert_eq!(
        legacy_after.user_config.hotkey_bindings.as_deref(),
        Some(r#"{"foo":"Ctrl+F"}"#),
        "the facade's cached user_config was not refreshed from the direct write"
    );
}

/// Sibling of the test above for `set_socks5_password` instead of
/// `update_settings`: a standalone secret-only call, with no
/// intervening `update_settings` call to refresh the facade's cache
/// first, must not silently revert a column it never touches either.
/// Unreachable through today's egui `SaveNetwork` handler (which always
/// calls `update_settings` first, refreshing the cache before this
/// method ever runs) but fully reachable through the facade API on its
/// own -- any caller that invokes `set_socks5_password` by itself, such
/// as a non-egui frontend.
#[test]
fn set_socks5_password_does_not_revert_columns_written_directly_via_config_service() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    // Simulate a still-unmigrated direct write (e.g. SaveKeyboardMouse)
    // that never goes through the facade at all -- and crucially, no
    // `update_settings` call happens in between to refresh the cache.
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let config_service = legacy
        .core_services
        .config_service
        .clone()
        .expect("config service must be available");
    let mut direct = config_service
        .get_user_config()
        .expect("read user config directly");
    direct.hotkey_bindings = Some(r#"{"foo":"Ctrl+F"}"#.to_string());
    config_service
        .save_user_config(&direct)
        .expect("direct write must succeed");

    runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(
            "does-not-clobber-hotkeys".to_string(),
        ))))
        .expect("setting the password must succeed");

    // The directly-written column must survive on disk...
    let after_on_disk = config_service
        .get_user_config()
        .expect("read user config directly");
    assert_eq!(
        after_on_disk.hotkey_bindings.as_deref(),
        Some(r#"{"foo":"Ctrl+F"}"#),
        "set_socks5_password reverted a column it never touched"
    );

    // ...and the facade's own cached view must have picked up the fresh
    // read too.
    let legacy_after = app.take_legacy_composition().expect("legacy composition");
    assert_eq!(
        legacy_after.user_config.hotkey_bindings.as_deref(),
        Some(r#"{"foo":"Ctrl+F"}"#),
        "the facade's cached user_config was not refreshed from the direct write"
    );
}

// ============================================================================
// update_settings: revision, validation, atomicity.
// ============================================================================

#[test]
fn revision_increments_on_success_and_a_stale_revision_is_a_conflict() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut patch = no_op_patch(0);
    patch.archive = Some(ArchiveSettingsPatch {
        cache_directory: PatchValue::Set(PathBuf::from("/cache/one")),
        ..keep_archive_patch()
    });
    let after_first = runtime
        .block_on(app.update_settings(patch))
        .expect("first update must succeed");
    assert_eq!(after_first.revision, 1);
    assert_eq!(
        after_first.archive.cache_directory.as_deref(),
        Some(Path::new("/cache/one"))
    );

    // Re-using the now-stale `expected_revision: 0` must be rejected...
    let stale_patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: Some(ArchiveSettingsPatch {
            cache_directory: PatchValue::Set(PathBuf::from("/cache/two")),
            ..keep_archive_patch()
        }),
        network: None,
        security: None,
    };
    let error = runtime
        .block_on(app.update_settings(stale_patch))
        .expect_err("stale expected_revision must be rejected");
    assert_eq!(error.kind, ApplicationErrorKind::Conflict);

    // ...and must not have changed anything.
    let after_conflict = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(after_conflict.revision, 1);
    assert_eq!(
        after_conflict.archive.cache_directory.as_deref(),
        Some(Path::new("/cache/one"))
    );

    // The correct (current) revision succeeds.
    let retry_patch = SettingsPatch {
        general: None,
        expected_revision: 1,
        archive: Some(ArchiveSettingsPatch {
            cache_directory: PatchValue::Set(PathBuf::from("/cache/two")),
            ..keep_archive_patch()
        }),
        network: None,
        security: None,
    };
    let after_retry = runtime
        .block_on(app.update_settings(retry_patch))
        .expect("retry must succeed");
    assert_eq!(after_retry.revision, 2);
    assert_eq!(
        after_retry.archive.cache_directory.as_deref(),
        Some(Path::new("/cache/two"))
    );
}

#[test]
fn invalid_clear_on_a_scalar_field_rejects_the_whole_patch_before_any_write() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: Some(ArchiveSettingsPatch {
            // Invalid: `backend_mode` has no empty state.
            backend_mode: PatchValue::Clear,
            // Would otherwise apply if the patch were evaluated field by
            // field instead of validated before any write.
            cache_directory: PatchValue::Set(PathBuf::from("/should/not/apply")),
            ..keep_archive_patch()
        }),
        network: None,
        security: None,
    };

    let error = runtime
        .block_on(app.update_settings(patch))
        .expect_err("Clear on backend_mode must be rejected");
    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("archive.backend_mode"));

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(
        snapshot.revision, 0,
        "a rejected patch must not bump the revision"
    );
    assert!(
        snapshot.archive.cache_directory.is_none(),
        "a rejected patch must not apply any of its other fields either"
    );
}

#[test]
fn a_forced_write_failure_leaves_settings_completely_unchanged() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    install_failing_user_config_trigger(&app);

    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: Some(ArchiveSettingsPatch {
            cache_directory: PatchValue::Set(PathBuf::from("/never/persisted")),
            ..keep_archive_patch()
        }),
        network: None,
        security: None,
    };

    let error = runtime
        .block_on(app.update_settings(patch))
        .expect_err("the injected trigger must fail the save");
    assert_eq!(error.kind, ApplicationErrorKind::Persistence);

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(
        snapshot.revision, 0,
        "a failed write must not bump the revision"
    );
    assert!(snapshot.archive.cache_directory.is_none());
}

/// Sibling of `a_forced_write_failure_leaves_settings_completely_
/// unchanged` isolating `update_settings`'s *second* write step
/// (`repoint_vault_paths`, which only runs when the security patch
/// touches `secrets_database_path`/`key_file_path`) instead of the
/// first. Pins down the documented divergence `settings_ops.rs`'s own
/// module doc comment describes for this step: `repoint_vault_paths`
/// persists the path-override row to `app_config` *before* attempting
/// to load the key file and re-open the vault at the new location, so a
/// load failure here leaves disk and memory in genuinely different
/// states -- the override is on disk, but `update_settings` returns
/// `Err` before phase 3's commit ever runs, so this instance's
/// in-memory `mutable` (and therefore every `settings()` call against
/// it) still reports the OLD key file path and the OLD, still-open
/// vault.
#[test]
fn repoint_vault_paths_failure_leaves_settings_unchanged_in_memory() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let before = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    let original_key_file_path = before
        .security
        .key_file_path
        .clone()
        .expect("a fresh bootstrap has a key file path");

    let missing_key_file = temp.path().join("does-not-exist.key");
    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: None,
        security: Some(SecuritySettingsPatch {
            key_file_path: PatchValue::Set(missing_key_file.clone()),
            ..keep_security_patch()
        }),
    };

    let error = runtime
        .block_on(app.update_settings(patch))
        .expect_err("a key file that does not exist must fail the vault repoint");
    assert_eq!(error.kind, ApplicationErrorKind::Persistence);

    // In memory: unchanged, exactly as documented.
    let after = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(
        after.revision, 0,
        "a failed vault repoint must not bump the revision"
    );
    assert_eq!(
        after.security.key_file_path.as_deref(),
        Some(original_key_file_path.as_path()),
        "the in-memory key_file_path must still show the OLD value"
    );
    assert!(
        after.security.vault_available,
        "the old vault must still be reported available after a failed repoint"
    );

    // On disk: the config-row override for the new (nonexistent) path
    // landed anyway -- this is the documented "not guaranteed" half of
    // the atomicity contract, not an oversight in this test.
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let dbs = legacy.dbs.expect("vault must still be available");
    let stored_override = dbs
        .config
        .with_connection(|conn| arclain_core::get_config(conn, "key_file_path"))
        .expect("read the persisted override directly");
    assert_eq!(
        stored_override.as_deref(),
        Some(missing_key_file.to_string_lossy().as_ref()),
        "the config-row override must have persisted even though the repoint itself failed"
    );

    // And the OLD vault handle itself must still genuinely work -- not
    // just that `vault_available` happens to read `true`.
    dbs.secrets
        .list_pass_rules()
        .expect("the old vault handle must still be open and usable after a failed repoint");
}

/// Sibling of the previous test isolating `update_settings`'s *third*
/// write step (`persist_encrypted_crc_policy`) instead of the second:
/// `secrets_database_path`/`key_file_path` both stay `Keep` so
/// `touches_vault_paths` is false and `repoint_vault_paths` never runs
/// at all, and only the `app_config` trigger (not the `user_config`
/// one `a_forced_write_failure_leaves_settings_completely_unchanged`
/// installs) is active, so the `user_config` row write (step 1) still
/// succeeds normally. This isolates the failure to the CRC-policy write
/// alone and proves the same "phase 3 never commits" guarantee holds
/// for it too -- the regression coverage the deleted H4-era test class
/// had for this exact persistence path.
#[test]
fn persist_encrypted_crc_policy_failure_leaves_settings_unchanged_in_memory() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    install_failing_app_config_trigger(&app);

    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: None,
        security: Some(SecuritySettingsPatch {
            encrypted_crc_policy: PatchValue::Set("always".to_string()),
            ..keep_security_patch()
        }),
    };

    let error = runtime
        .block_on(app.update_settings(patch))
        .expect_err("the injected app_config trigger must fail the CRC policy save");
    assert_eq!(error.kind, ApplicationErrorKind::Persistence);

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(
        snapshot.revision, 0,
        "a failed CRC policy write must not bump the revision"
    );
    assert_eq!(
        snapshot.security.encrypted_crc_policy, "on_access",
        "the in-memory CRC policy must still show the OLD value"
    );
}

/// `set_socks5_password` now routes through the *same* journaled
/// `NetworkProxyPersistenceService::save` path `run_update_settings`'s
/// identity-touching branch uses (see `run_set_socks5_password`'s own
/// "I4" doc note), instead of a bare `set_secret` call that never
/// consulted any pending recovery marker at all. This proves the
/// difference that routing makes: a stale/corrupt marker left behind by
/// an interrupted *earlier, unrelated* identity-changing save (a
/// scenario this test simulates directly, since actually crashing a
/// real save mid-flight to produce one is `arclain_core`'s own
/// `network_proxy_persistence_service` test suite's job, not
/// reproducible from here without reaching into that module's private
/// marker type) must fail this call cleanly rather than being silently
/// ignored while the new password gets applied on top of unresolved,
/// corrupt recovery state.
///
/// Before this fix, a bare `set_secret`/`remove_secret` call didn't
/// consult `journal:proxy-settings` at all, so this exact scenario would
/// have silently *succeeded*, leaving the corrupt marker in place to
/// confuse the next recovery pass at the next bootstrap.
#[test]
fn set_socks5_password_fails_cleanly_instead_of_ignoring_a_corrupt_pending_marker() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    const OLD_PASSWORD: &str = "old-password-preserved-2f6a";
    const NEW_PASSWORD: &str = "new-password-must-not-silently-apply-91be";

    runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(OLD_PASSWORD.to_string()))))
        .expect("staging the old password must succeed");

    let legacy = app.take_legacy_composition().expect("legacy composition");
    let dbs = legacy.dbs.expect("vault must be available");
    dbs.secrets
        .set_secret("journal:proxy-settings", "not-valid-json")
        .expect("stage a corrupt pending-update marker");

    let error = runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(NEW_PASSWORD.to_string()))))
        .expect_err("a corrupt pending marker must fail this call, not be silently ignored");
    assert_eq!(error.kind, ApplicationErrorKind::Persistence);

    // The OLD password must still be there -- the new one was never
    // applied on top of unresolved recovery state.
    let restored = dbs
        .secrets
        .get_secret("proxy:socks5")
        .expect("read the secret after the failed call")
        .expect("a password must still be present");
    assert_eq!(
        restored.as_str(),
        OLD_PASSWORD,
        "the new password must not have been applied while a corrupt recovery marker was pending"
    );
}

#[test]
fn update_settings_applies_live_proxy_routing_to_the_shared_http_client() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let http_client = legacy.core_services.async_http_client.clone();
    assert!(
        !http_client.should_use_proxy_for_plugin("dlsite"),
        "a fresh bootstrap must not already be routing through a proxy"
    );

    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set("127.0.0.1:1080".to_string()),
            ..keep_network_patch()
        }),
        security: None,
    };
    runtime
        .block_on(app.update_settings(patch))
        .expect("update must succeed");

    assert!(
        http_client.should_use_proxy_for_plugin("dlsite"),
        "enabling SOCKS5 through update_settings must apply live routing to the shared \
         AsyncHttpClient, the same way the pre-facade SaveNetwork handler did"
    );

    // Disabling again must clear it, mirroring the pre-facade handler's
    // own `save_network_disable_clears_runtime_plugin_proxy_map` guarantee.
    let disable_patch = SettingsPatch {
        general: None,
        expected_revision: 1,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(false),
            ..keep_network_patch()
        }),
        security: None,
    };
    runtime
        .block_on(app.update_settings(disable_patch))
        .expect("disable must succeed");
    assert!(!http_client.should_use_proxy_for_plugin("dlsite"));
}

#[test]
fn invalid_enabled_proxy_address_is_rejected_before_any_write() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set(
                "userinfo-user:userinfo-password@proxy.example:1080".to_string(),
            ),
            ..keep_network_patch()
        }),
        security: None,
    };

    let error = runtime
        .block_on(app.update_settings(patch))
        .expect_err("userinfo in the proxy address must be rejected");
    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(snapshot.revision, 0);
    assert!(!snapshot.network.socks5_enabled);
}

/// `validate_proxy_for_storage` now validates the SOCKS5 identity
/// fields against the *real* currently-stored password instead of
/// always passing `password: None` (the "fold 1" fix -- see
/// `settings_ops.rs`'s own doc comment on `validate_proxy_for_storage`).
/// This cannot be pinned as a "used to wrongly accept X, now rejects
/// X" regression test the way
/// `invalid_enabled_proxy_address_is_rejected_before_any_write` is:
/// `url::Url::set_username`/`set_password` (what
/// `ProxyConfig::validate_for_storage` calls into) only ever fail for a
/// `cannot-be-a-base` URL, which a `socks5h://host:port` authority
/// never is, so no input exists today where validation's outcome
/// differs based on which password value it receives. What this test
/// pins instead is the actual, previously-uncovered gap: an
/// identity-only `update_settings` call (its patch carries no password
/// field at all) still succeeds when a real username+password pair is
/// already configured, and the password itself survives that call
/// completely unchanged -- proving the `existing_password` this fix
/// now also feeds to validation still correctly reaches
/// `NetworkProxyPersistenceService::save` afterward, exactly as it did
/// before this fix.
#[test]
fn update_settings_preserves_an_existing_password_across_an_identity_only_change() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    const PASSWORD: &str = "preserved-across-identity-change-7d2f";

    let enable_patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set("127.0.0.1:1080".to_string()),
            socks5_username: PatchValue::Set("proxyuser".to_string()),
            ..keep_network_patch()
        }),
        security: None,
    };
    runtime
        .block_on(app.update_settings(enable_patch))
        .expect("enabling socks5 with a username must succeed");
    runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(PASSWORD.to_string()))))
        .expect("setting the password must succeed");

    // An identity-only change (a new address, same username) -- no
    // password field anywhere in this patch.
    let current = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    let readdress_patch = SettingsPatch {
        general: None,
        expected_revision: current.revision,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set("127.0.0.1:1081".to_string()),
            socks5_username: PatchValue::Set("proxyuser".to_string()),
            ..keep_network_patch()
        }),
        security: None,
    };
    let after_readdress = runtime
        .block_on(app.update_settings(readdress_patch))
        .expect(
            "an identity-only change must succeed when a real username+password pair is \
             already configured",
        );
    assert_eq!(
        after_readdress.network.socks5_address.as_deref(),
        Some("127.0.0.1:1081")
    );
    assert!(
        after_readdress.network.socks5_password_configured,
        "the identity-only change must not have cleared the password"
    );

    let legacy = app.take_legacy_composition().expect("legacy composition");
    let dbs = legacy.dbs.expect("vault must be available");
    let stored = dbs
        .secrets
        .get_secret("proxy:socks5")
        .expect("read the password after the identity-only change")
        .expect("a password must still be present");
    assert_eq!(
        stored.as_str(),
        PASSWORD,
        "the password must survive an identity-only update_settings call unchanged"
    );
}

#[test]
fn update_settings_changes_directories_and_network_fields_together() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: Some(ArchiveSettingsPatch {
            temp_directory: PatchValue::Set(PathBuf::from("/tmp/arclain-work")),
            backend_mode: PatchValue::Set(BackendModeDto::Cli),
            ..keep_archive_patch()
        }),
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set("127.0.0.1:1080".to_string()),
            gameta_server_enabled: PatchValue::Set(true),
            gameta_server_url: PatchValue::Set("https://gameta.example".to_string()),
            ..keep_network_patch()
        }),
        security: Some(SecuritySettingsPatch {
            encrypted_crc_policy: PatchValue::Set("always".to_string()),
            ..keep_security_patch()
        }),
    };

    let snapshot = runtime
        .block_on(app.update_settings(patch))
        .expect("update must succeed");

    assert_eq!(snapshot.archive.backend_mode, BackendModeDto::Cli);
    assert_eq!(
        snapshot.archive.temp_directory.as_deref(),
        Some(Path::new("/tmp/arclain-work"))
    );
    assert!(snapshot.network.socks5_enabled);
    assert_eq!(
        snapshot.network.socks5_address.as_deref(),
        Some("127.0.0.1:1080")
    );
    assert!(snapshot.network.gameta_server_enabled);
    assert_eq!(
        snapshot.network.gameta_server_url.as_deref(),
        Some("https://gameta.example")
    );
    assert_eq!(snapshot.security.encrypted_crc_policy, "always");

    // Persisted, not just in-memory: a fresh read agrees.
    let reread = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(reread.archive.backend_mode, BackendModeDto::Cli);
    assert!(reread.network.socks5_enabled);
}

/// The pipeline collision default is an archive setting stored outside
/// the `user_config` row (in `app_config`), so it needs its own
/// round-trip proof: patch it, read it back through `settings()`, and
/// confirm it survived a fresh bootstrap against the same profile --
/// i.e. that it actually reached disk rather than only the in-memory
/// mirror.
#[test]
fn the_pipeline_collision_default_round_trips_and_persists() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let snapshot = runtime
        .block_on(app.update_settings(SettingsPatch {
            expected_revision: 0,
            archive: Some(ArchiveSettingsPatch {
                default_collision_policy: PatchValue::Set("overwrite".to_string()),
                ..keep_archive_patch()
            }),
            network: None,
            security: None,
            general: None,
        }))
        .expect("update must succeed");
    assert_eq!(snapshot.archive.default_collision_policy, "overwrite");

    let reread = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(reread.archive.default_collision_policy, "overwrite");

    runtime.block_on(app.shutdown()).expect("shutdown");
    drop(app);

    let reopened = bootstrap_app(&temp);
    let after_restart = runtime
        .block_on(reopened.settings())
        .expect("settings must succeed");
    assert_eq!(
        after_restart.archive.default_collision_policy, "overwrite",
        "the collision default must be read back from app_config at bootstrap"
    );
}

/// The patch surface refuses a token `OutputCollisionPolicy` cannot
/// parse, and nothing is written -- otherwise the stored typo would read
/// back as "no app default", quietly changing which policy pipelines run
/// under.
#[test]
fn an_unrecognized_collision_policy_is_rejected_and_nothing_is_stored() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.update_settings(SettingsPatch {
            expected_revision: 0,
            archive: Some(ArchiveSettingsPatch {
                default_collision_policy: PatchValue::Set("overwrit".to_string()),
                ..keep_archive_patch()
            }),
            network: None,
            security: None,
            general: None,
        }))
        .expect_err("an unrecognized collision policy must be rejected");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(
        error.field.as_deref(),
        Some("archive.default_collision_policy")
    );

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(snapshot.revision, 0, "a rejected patch must not commit");
    assert_eq!(snapshot.archive.default_collision_policy, "smart");
}

/// A patch that leaves the collision default alone must not disturb it,
/// even though the `app_config` write step runs on every call -- the
/// value it writes is the one already in effect.
#[test]
fn an_unrelated_patch_leaves_the_collision_default_alone() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.update_settings(SettingsPatch {
            expected_revision: 0,
            archive: Some(ArchiveSettingsPatch {
                default_collision_policy: PatchValue::Set("skip".to_string()),
                ..keep_archive_patch()
            }),
            network: None,
            security: None,
            general: None,
        }))
        .expect("update must succeed");

    let snapshot = runtime
        .block_on(app.update_settings(SettingsPatch {
            expected_revision: 1,
            archive: None,
            network: None,
            security: Some(SecuritySettingsPatch {
                encrypted_crc_policy: PatchValue::Set("always".to_string()),
                ..keep_security_patch()
            }),
            general: None,
        }))
        .expect("update must succeed");

    assert_eq!(snapshot.security.encrypted_crc_policy, "always");
    assert_eq!(snapshot.archive.default_collision_policy, "skip");
}

#[test]
fn clear_on_the_plugin_proxy_map_resets_it_to_empty() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut map = std::collections::BTreeMap::new();
    map.insert("dlsite".to_string(), false);
    let set_patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: Some(NetworkSettingsPatch {
            plugin_proxy_enabled: PatchValue::Set(map),
            ..keep_network_patch()
        }),
        security: None,
    };
    let snapshot = runtime
        .block_on(app.update_settings(set_patch))
        .expect("set must succeed");
    assert_eq!(
        snapshot.network.plugin_proxy_enabled.get("dlsite"),
        Some(&false)
    );

    let clear_patch = SettingsPatch {
        general: None,
        expected_revision: snapshot.revision,
        archive: None,
        network: Some(NetworkSettingsPatch {
            plugin_proxy_enabled: PatchValue::Clear,
            ..keep_network_patch()
        }),
        security: None,
    };
    let cleared = runtime
        .block_on(app.update_settings(clear_patch))
        .expect("clear must succeed");
    assert!(cleared.network.plugin_proxy_enabled.is_empty());
}

#[test]
fn gameta_connection_status_uses_configuration_and_composed_client_state() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    assert_eq!(
        runtime
            .block_on(app.gameta_connection_status())
            .expect("read disabled gameta status"),
        GametaConnectionStatusDto::Disabled,
    );

    let current = runtime.block_on(app.settings()).expect("read settings");
    runtime
        .block_on(app.update_settings(SettingsPatch {
            expected_revision: current.revision,
            archive: None,
            network: Some(NetworkSettingsPatch {
                gameta_server_enabled: PatchValue::Set(true),
                ..keep_network_patch()
            }),
            security: None,
            general: None,
        }))
        .expect("enable gameta integration");

    assert_eq!(
        runtime
            .block_on(app.gameta_connection_status())
            .expect("read unavailable gameta status"),
        GametaConnectionStatusDto::Unavailable,
        "enabling after startup does not invent a client that was never composed",
    );
}

/// The "fold 2" fix: a patch that changes *only* the per-plugin proxy
/// map (no SOCKS5 identity field) must still reach the live
/// `AsyncHttpClient` immediately, the same way
/// `update_settings_applies_live_proxy_routing_to_the_shared_http_client`
/// already proves for an identity-touching change. Before this fix,
/// `apply_live_proxy_routing` only ran inside the identity-touching
/// branch, so a plugin-proxy-map-only patch persisted correctly (see
/// `clear_on_the_plugin_proxy_map_resets_it_to_empty`) but left the live
/// client routing stale until the next identity-touching save or a
/// restart.
#[test]
fn update_settings_applies_a_plugin_proxy_map_only_change_to_the_shared_http_client() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let http_client = legacy.core_services.async_http_client.clone();

    // Enable SOCKS5 first (an identity-touching call) so the default
    // per-plugin routing map is non-empty.
    let enable_patch = SettingsPatch {
        general: None,
        expected_revision: 0,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set("127.0.0.1:1080".to_string()),
            ..keep_network_patch()
        }),
        security: None,
    };
    let after_enable = runtime
        .block_on(app.update_settings(enable_patch))
        .expect("enabling socks5 must succeed");
    assert!(
        http_client.should_use_proxy_for_plugin("dlsite"),
        "dlsite is proxied by default once socks5 is enabled"
    );

    // A plugin-proxy-map-ONLY patch -- no SOCKS5 identity field
    // anywhere in this patch.
    let mut map = std::collections::BTreeMap::new();
    map.insert("dlsite".to_string(), false);
    let map_only_patch = SettingsPatch {
        general: None,
        expected_revision: after_enable.revision,
        archive: None,
        network: Some(NetworkSettingsPatch {
            plugin_proxy_enabled: PatchValue::Set(map),
            ..keep_network_patch()
        }),
        security: None,
    };
    runtime
        .block_on(app.update_settings(map_only_patch))
        .expect("a plugin-proxy-map-only update must succeed");

    assert!(
        !http_client.should_use_proxy_for_plugin("dlsite"),
        "a plugin-proxy-map-only update_settings call must reach the live AsyncHttpClient \
         immediately, the same way an identity-touching call already did"
    );
}

// ============================================================================
// Secrets: SOCKS5 password, gameta API key -- set/clear and redaction.
// ============================================================================

#[test]
fn socks5_password_set_and_clear_are_reflected_and_never_leak() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    const SECRET: &str = "socks5-password-fbc19a";

    runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(SECRET.to_string()))))
        .expect("setting the password must succeed");
    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert!(snapshot.network.socks5_password_configured);
    let serialized = serde_json::to_string(&snapshot).expect("snapshot must serialize");
    assert!(
        !serialized.contains(SECRET),
        "raw password leaked into serialized snapshot"
    );
    assert!(
        !format!("{snapshot:?}").contains(SECRET),
        "raw password leaked into Debug output"
    );

    runtime
        .block_on(app.set_socks5_password(None))
        .expect("clearing the password must succeed");
    let cleared = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert!(!cleared.network.socks5_password_configured);
}

/// The password is never observable from the client's own API, so this
/// proves the fix through the closest real observable: a fake SOCKS5
/// proxy that captures the actual credential bytes a live request
/// authenticates with. Before the I3 fix, `set_socks5_password` wrote
/// the new secret to storage but never re-applied live routing, so the
/// shared `AsyncHttpClient` kept using whatever password was live at the
/// last `update_settings` call until the next identity-touching save or
/// a restart -- this drives an actual proxied request immediately after
/// `set_socks5_password` alone (no intervening `update_settings` call)
/// and asserts the sentinel saw the *new* password, not the old one.
#[test]
fn set_socks5_password_re_applies_live_routing_with_the_new_credential_immediately() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let proxy = TcpListener::bind("127.0.0.1:0").expect("bind sentinel proxy");
    let proxy_address = proxy.local_addr().expect("read sentinel address");
    proxy
        .set_nonblocking(true)
        .expect("make sentinel nonblocking");

    let captured: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    let captured_on_thread = captured.clone();
    let request_finished = Arc::new(AtomicBool::new(false));
    let finished_on_thread = request_finished.clone();
    let sentinel = std::thread::spawn(move || {
        capture_socks5_credentials(proxy, finished_on_thread, captured_on_thread);
    });

    // Enable SOCKS5 (identity fields only) with an initial password.
    let current = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    runtime
        .block_on(app.update_settings(SettingsPatch {
            general: None,
            expected_revision: current.revision,
            archive: None,
            network: Some(NetworkSettingsPatch {
                socks5_enabled: PatchValue::Set(true),
                socks5_address: PatchValue::Set(proxy_address.to_string()),
                socks5_username: PatchValue::Set("sentinel-user".to_string()),
                ..keep_network_patch()
            }),
            security: None,
        }))
        .expect("enabling socks5 must succeed");
    runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new("old-password".to_string()))))
        .expect("setting the initial password must succeed");

    // The call under test: change *only* the password, with no
    // intervening `update_settings` call.
    const NEW_PASSWORD: &str = "sentinel-new-password-6d21";
    runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(NEW_PASSWORD.to_string()))))
        .expect("setting the new password must succeed");

    // Drive an actual request through the shared client's *current*
    // live routing. This errors out (the sentinel never answers the
    // CONNECT step that follows the auth handshake it captures) --
    // that's expected and irrelevant here; only the captured credential
    // matters.
    let legacy = app.take_legacy_composition().expect("legacy composition");
    let _ = legacy
        .core_services
        .async_http_client
        .blocking_get(&format!("http://{proxy_address}/whatever"), true);

    request_finished.store(true, Ordering::SeqCst);
    sentinel.join().expect("sentinel thread panicked");

    let (username, password) = captured
        .lock()
        .unwrap()
        .clone()
        .expect("the sentinel must have observed a SOCKS5 auth handshake");
    assert_eq!(username, "sentinel-user");
    assert_eq!(
        password, NEW_PASSWORD,
        "set_socks5_password must re-apply live routing with the NEW password immediately, \
         not just persist it for the next identity-touching update_settings call or a restart"
    );
}

#[test]
fn gameta_api_key_set_is_reflected_and_never_leaks() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    const SECRET: &str = "gameta-api-key-9d24ab";

    runtime
        .block_on(app.set_gameta_api_key(SecretInput::new(SECRET.to_string())))
        .expect("setting the API key must succeed");
    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert!(snapshot.network.gameta_api_key_configured);
    let serialized = serde_json::to_string(&snapshot).expect("snapshot must serialize");
    assert!(!serialized.contains(SECRET));
    assert!(!format!("{snapshot:?}").contains(SECRET));
}

// ============================================================================
// Password rules.
// ============================================================================

/// A rule auto-saved before the pattern heuristic existed matches
/// exactly one archive, so a sibling sharing its product code re-prompts
/// for a password the vault already holds. Bootstrap broadens it, and
/// the broadened pattern is what the very first `password_rules()` read
/// reports -- proving the rewrite lands before anything can read a rule,
/// which is the whole reason it belongs here rather than in a frontend.
///
/// Broadens *to the product code*, which is the tier the `gameta`
/// feature provides; the twin below covers the same startup rewrite in a
/// lean build, where derivation starts at the maker bracket instead.
#[cfg(feature = "gameta")]
#[test]
fn bootstrap_broadens_a_narrow_auto_saved_password_rule() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));

    const ARCHIVE: &str = "Some Title [RJ100001] v2.zip";
    support::seed_named_pass_rule(
        &paths,
        &format!("Auto-saved: {ARCHIVE}"),
        &regex::escape(ARCHIVE),
        "stored-password",
    );

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");

    assert_eq!(
        app.startup_password_rule_upgrades(),
        1,
        "the one narrow auto-saved rule must be reported as upgraded"
    );

    let rules = runtime
        .block_on(app.password_rules())
        .expect("password rules must be readable");
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].pattern, "(?i)RJ100001",
        "the rule must already be broadened by the first read"
    );
    assert!(
        rules[0].password_configured,
        "broadening a pattern must not disturb the stored password"
    );

    // Idempotent: a second launch against the same profile finds
    // nothing left to broaden.
    runtime.block_on(app.shutdown()).expect("shutdown");
    drop(app);

    let reopened = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");
    assert_eq!(reopened.startup_password_rule_upgrades(), 0);
    let rules = runtime
        .block_on(reopened.password_rules())
        .expect("password rules must be readable");
    assert_eq!(rules[0].pattern, "(?i)RJ100001");
}

/// The same startup rewrite in a lean build. The product-code tier is
/// not compiled there, so a narrow auto-saved rule broadens to the
/// maker bracket instead -- still before the first `password_rules()`
/// read, which is the part this test exists to pin. Without it the
/// broadening pass would be exercised only by builds that happen to
/// carry the metadata stack.
#[cfg(not(feature = "gameta"))]
#[test]
fn bootstrap_broadens_a_narrow_auto_saved_password_rule() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));

    const ARCHIVE: &str = "[Crew Name] Some Title v2.zip";
    support::seed_named_pass_rule(
        &paths,
        &format!("Auto-saved: {ARCHIVE}"),
        &regex::escape(ARCHIVE),
        "stored-password",
    );

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");

    assert_eq!(
        app.startup_password_rule_upgrades(),
        1,
        "the one narrow auto-saved rule must be reported as upgraded"
    );

    let rules = runtime
        .block_on(app.password_rules())
        .expect("password rules must be readable");
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].pattern, r"^\[Crew Name\]",
        "the rule must already be broadened by the first read"
    );
    assert!(
        rules[0].password_configured,
        "broadening a pattern must not disturb the stored password"
    );
}

/// A rule the user wrote or renamed carries neither half of the
/// auto-saved fingerprint, so bootstrap must leave it exactly as it is.
#[test]
fn bootstrap_leaves_a_hand_written_password_rule_alone() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    support::seed_named_pass_rule(
        &paths,
        "My own rule",
        &regex::escape("Some Title [RJ100001] v2.zip"),
        "stored-password",
    );

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");

    assert_eq!(app.startup_password_rule_upgrades(), 0);
    let rules = runtime
        .block_on(app.password_rules())
        .expect("password rules must be readable");
    assert_eq!(
        rules[0].pattern,
        regex::escape("Some Title [RJ100001] v2.zip")
    );
}

#[test]
fn upsert_and_delete_password_rule_round_trip() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let rules = runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "DLsite Standard".to_string(),
            pattern: r"^RJ\d+".to_string(),
            priority: 10,
            enabled: true,
            password: Some(SecretInput::new("dlsite-password-2b6e".to_string())),
        }))
        .expect("upsert must succeed");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "DLsite Standard");
    assert!(rules[0].password_configured);

    let listed = runtime
        .block_on(app.password_rules())
        .expect("password_rules must succeed");
    assert_eq!(listed.len(), 1);

    let after_delete = runtime
        .block_on(app.delete_password_rule("DLsite Standard".to_string()))
        .expect("delete must succeed");
    assert!(after_delete.is_empty());
    assert!(runtime.block_on(app.password_rules()).unwrap().is_empty());
}

#[test]
fn upsert_password_rule_without_a_password_requires_an_existing_rule() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "brand new".to_string(),
            pattern: "pattern".to_string(),
            priority: 10,
            enabled: true,
            password: None,
        }))
        .expect_err("a new rule with no password must be rejected");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("password"));
}

#[test]
fn upsert_password_rule_without_a_password_keeps_the_existing_one() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "Maker Bracket".to_string(),
            pattern: r"^\[Maker\]".to_string(),
            priority: 5,
            enabled: true,
            password: Some(SecretInput::new("original-password-71ac".to_string())),
        }))
        .expect("initial create must succeed");
    assert_eq!(
        raw_pass_rule_password(&app, "Maker Bracket").as_deref(),
        Some("original-password-71ac")
    );

    // Same name, no password: pattern/priority/enabled change, password
    // must survive untouched.
    let updated = runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "Maker Bracket".to_string(),
            pattern: r"^\[Other Maker\]".to_string(),
            priority: 20,
            enabled: false,
            password: None,
        }))
        .expect("update without a password must succeed and keep the old one");

    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].pattern, r"^\[Other Maker\]");
    assert_eq!(updated[0].priority, 20);
    assert!(!updated[0].enabled);
    assert!(updated[0].password_configured);
    assert_eq!(
        raw_pass_rule_password(&app, "Maker Bracket").as_deref(),
        Some("original-password-71ac"),
        "the stored password must not change when password: None updates an existing rule"
    );
}

#[test]
fn replace_password_rules_renames_without_exposing_or_changing_password() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "before rename".to_string(),
            pattern: r"^before".to_string(),
            priority: 5,
            enabled: true,
            password: Some(SecretInput::new("rename-secret-5c31".to_string())),
        }))
        .expect("seed rule");

    let summaries = runtime
        .block_on(app.replace_password_rules(vec![PasswordRuleEditInput {
            original_name: Some("before rename".to_string()),
            name: "after rename".to_string(),
            pattern: r"^after".to_string(),
            priority: 9,
            enabled: false,
            password: None,
        }]))
        .expect("rename without replacement password");

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "after rename");
    assert_eq!(summaries[0].pattern, r"^after");
    assert_eq!(summaries[0].priority, 9);
    assert!(!summaries[0].enabled);
    assert!(summaries[0].password_configured);
    assert_eq!(
        raw_pass_rule_password(&app, "after rename").as_deref(),
        Some("rename-secret-5c31")
    );
    assert!(raw_pass_rule_password(&app, "before rename").is_none());
}

#[test]
fn replace_password_rules_replaces_password_when_supplied() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "replace me".to_string(),
            pattern: "old-pattern".to_string(),
            priority: 1,
            enabled: true,
            password: Some(SecretInput::new("old-secret-95ec".to_string())),
        }))
        .expect("seed rule");

    runtime
        .block_on(app.replace_password_rules(vec![PasswordRuleEditInput {
            original_name: Some("replace me".to_string()),
            name: "replace me".to_string(),
            pattern: "new-pattern".to_string(),
            priority: 2,
            enabled: true,
            password: Some(SecretInput::new("new-secret-f200".to_string())),
        }]))
        .expect("replace password");

    assert_eq!(
        raw_pass_rule_password(&app, "replace me").as_deref(),
        Some("new-secret-f200")
    );
}

#[test]
fn replace_password_rules_rejects_new_rule_without_password_atomically() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "existing".to_string(),
            pattern: "existing-pattern".to_string(),
            priority: 1,
            enabled: true,
            password: Some(SecretInput::new("existing-secret-b70a".to_string())),
        }))
        .expect("seed rule");

    let error = runtime
        .block_on(app.replace_password_rules(vec![
            PasswordRuleEditInput {
                original_name: Some("existing".to_string()),
                name: "existing changed".to_string(),
                pattern: "changed-pattern".to_string(),
                priority: 2,
                enabled: false,
                password: None,
            },
            PasswordRuleEditInput {
                original_name: None,
                name: "new without password".to_string(),
                pattern: "new-pattern".to_string(),
                priority: 3,
                enabled: true,
                password: None,
            },
        ]))
        .expect_err("new rule without a password must reject the whole replacement");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("password"));
    let summaries = runtime.block_on(app.password_rules()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "existing");
    assert_eq!(summaries[0].pattern, "existing-pattern");
    assert!(summaries[0].enabled);
    assert_eq!(
        raw_pass_rule_password(&app, "existing").as_deref(),
        Some("existing-secret-b70a")
    );
}

#[test]
fn replace_password_rules_rejects_duplicate_original_names() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "one source".to_string(),
            pattern: "source-pattern".to_string(),
            priority: 1,
            enabled: true,
            password: Some(SecretInput::new("source-secret-f931".to_string())),
        }))
        .expect("seed rule");

    let error = runtime
        .block_on(app.replace_password_rules(vec![
            PasswordRuleEditInput {
                original_name: Some("one source".to_string()),
                name: "first result".to_string(),
                pattern: "first-pattern".to_string(),
                priority: 1,
                enabled: true,
                password: None,
            },
            PasswordRuleEditInput {
                original_name: Some("one source".to_string()),
                name: "second result".to_string(),
                pattern: "second-pattern".to_string(),
                priority: 2,
                enabled: true,
                password: None,
            },
        ]))
        .expect_err("one stored rule cannot back two edited rows");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("original_name"));
    let summaries = runtime.block_on(app.password_rules()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "one source");
}

#[test]
fn replace_password_rules_rejects_duplicate_result_names() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    for (name, password) in [
        ("first", "first-secret-65ea"),
        ("second", "second-secret-c8ed"),
    ] {
        runtime
            .block_on(app.upsert_password_rule(PasswordRuleInput {
                name: name.to_string(),
                pattern: format!("{name}-pattern"),
                priority: 1,
                enabled: true,
                password: Some(SecretInput::new(password.to_string())),
            }))
            .expect("seed rule");
    }

    let error = runtime
        .block_on(app.replace_password_rules(vec![
            PasswordRuleEditInput {
                original_name: Some("first".to_string()),
                name: "same result".to_string(),
                pattern: "first-pattern".to_string(),
                priority: 1,
                enabled: true,
                password: None,
            },
            PasswordRuleEditInput {
                original_name: Some("second".to_string()),
                name: "same result".to_string(),
                pattern: "second-pattern".to_string(),
                priority: 1,
                enabled: true,
                password: None,
            },
        ]))
        .expect_err("two edited rows cannot have the same resulting name");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("name"));
    let summaries = runtime.block_on(app.password_rules()).unwrap();
    assert_eq!(summaries.len(), 2);
    assert!(summaries.iter().any(|rule| rule.name == "first"));
    assert!(summaries.iter().any(|rule| rule.name == "second"));
}

#[test]
fn replace_password_rules_rejects_unknown_original_name() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.replace_password_rules(vec![PasswordRuleEditInput {
            original_name: Some("not stored".to_string()),
            name: "renamed".to_string(),
            pattern: "pattern".to_string(),
            priority: 1,
            enabled: true,
            password: None,
        }]))
        .expect_err("an unknown original identity must be rejected");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("original_name"));
    assert!(runtime.block_on(app.password_rules()).unwrap().is_empty());
}

#[test]
fn replace_password_rules_persistence_failure_keeps_memory_unchanged() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "memory sentinel".to_string(),
            pattern: "before-persistence-failure".to_string(),
            priority: 1,
            enabled: true,
            password: Some(SecretInput::new("memory-secret-929a".to_string())),
        }))
        .expect("seed rule");
    let legacy = app.take_legacy_composition().expect("legacy composition");
    legacy.dbs.expect("vault must be available").secrets.close();

    let error = runtime
        .block_on(app.replace_password_rules(vec![PasswordRuleEditInput {
            original_name: Some("memory sentinel".to_string()),
            name: "memory sentinel".to_string(),
            pattern: "must-not-land".to_string(),
            priority: 99,
            enabled: false,
            password: None,
        }]))
        .expect_err("closed vault must reject persistence");

    assert_eq!(error.kind, ApplicationErrorKind::Persistence);
    let summaries = runtime.block_on(app.password_rules()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "memory sentinel");
    assert_eq!(summaries[0].pattern, "before-persistence-failure");
    assert_eq!(summaries[0].priority, 1);
    assert!(summaries[0].enabled);
}

#[test]
fn upsert_password_rule_rejects_an_invalid_pattern() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "broken".to_string(),
            pattern: "(".to_string(),
            priority: 10,
            enabled: true,
            password: Some(SecretInput::new("password".to_string())),
        }))
        .expect_err("an invalid regex pattern must be rejected");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("pattern"));
    assert!(runtime.block_on(app.password_rules()).unwrap().is_empty());
}

#[test]
fn delete_password_rule_with_an_unknown_name_is_not_found() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.delete_password_rule("does not exist".to_string()))
        .expect_err("deleting an unknown rule must fail");

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

#[test]
fn password_rule_summaries_never_carry_the_raw_password() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    const SECRET: &str = "very-secret-archive-password-c40f";

    let rules = runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "n".to_string(),
            pattern: "p".to_string(),
            priority: 1,
            enabled: true,
            password: Some(SecretInput::new(SECRET.to_string())),
        }))
        .expect("upsert must succeed");

    let serialized = serde_json::to_string(&rules).expect("rules must serialize");
    assert!(!serialized.contains(SECRET));
    assert!(!format!("{rules:?}").contains(SECRET));
}

// ============================================================================
// Vault genuinely unavailable (composite `open_databases` never
// succeeded -- see `bootstrap.rs::corrupt_configuration_database_is_
// tolerated`'s identical setup for why corrupting `config.sqlite` is the
// established way to force this, not just a `pass.redb`-specific
// failure).
// ============================================================================

#[test]
fn secret_writing_methods_fail_cleanly_and_leak_nothing_when_the_vault_never_opened() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_corrupt_config(&paths);
    // No dummy 7-Zip seeded: the corrupt file wipes any seeded
    // `sevenzip_path` override too, so this relies on the same real-PATH
    // 7-Zip assumption `bootstrap.rs`'s own module doc comment documents
    // for this exact scenario.
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("corrupt config.sqlite must not fail bootstrap");

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must still be readable");
    assert!(!snapshot.security.vault_available);

    const SECRET: &str = "never-persisted-password-6a3e";
    let error = runtime
        .block_on(app.set_socks5_password(Some(SecretInput::new(SECRET.to_string()))))
        .expect_err("setting a secret with no open vault must fail");
    assert_eq!(error.kind, ApplicationErrorKind::Unsupported);
    assert!(!format!("{error:?}").contains(SECRET));

    let network_patch = SettingsPatch {
        general: None,
        expected_revision: snapshot.revision,
        archive: None,
        network: Some(NetworkSettingsPatch {
            socks5_enabled: PatchValue::Set(true),
            socks5_address: PatchValue::Set("127.0.0.1:1080".to_string()),
            ..keep_network_patch()
        }),
        security: None,
    };
    // A network-identity patch also needs the vault (to preserve the
    // existing SOCKS5 password across the address/username change), so
    // it must fail the same way rather than silently persisting only
    // the config-side fields.
    let error = runtime
        .block_on(app.update_settings(network_patch))
        .expect_err("a socks5-identity patch with no open vault must fail");
    assert_eq!(error.kind, ApplicationErrorKind::Unsupported);

    let after = runtime
        .block_on(app.settings())
        .expect("settings must still be readable");
    assert_eq!(
        after.revision, snapshot.revision,
        "a failed patch must not bump the revision"
    );
    assert!(!after.network.socks5_enabled);
}

// ============================================================================
// Vault move/rekey: single-authority mirror + pass-rule survival.
// ============================================================================

#[test]
fn move_vault_relocates_the_secrets_file_and_the_legacy_mirror_observes_it() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "before move".to_string(),
            pattern: "pattern".to_string(),
            priority: 10,
            enabled: true,
            password: Some(SecretInput::new("moved-vault-password-83ce".to_string())),
        }))
        .expect("seed a rule before moving the vault");

    let destination = temp.path().join("relocated").join("pass.redb");
    runtime
        .block_on(app.move_vault(destination.clone()))
        .expect("move_vault must succeed");

    assert!(
        destination.exists(),
        "the vault file must exist at the new location"
    );

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(
        snapshot.security.secrets_database_path.as_deref(),
        Some(destination.as_path())
    );
    assert!(snapshot.security.vault_available);

    // Pass rules survive the move...
    let rules = runtime
        .block_on(app.password_rules())
        .expect("password_rules must succeed");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "before move");

    // ...and this task's single-authority fix means the legacy mirror
    // `crates/ui` reads from is the SAME live state, not a stale
    // bootstrap-time snapshot: a fresh `take_legacy_composition` call
    // must see the moved vault's contents too.
    let legacy = app.take_legacy_composition().expect("legacy composition");
    assert_eq!(legacy.pass_rules.len(), 1);
    assert_eq!(legacy.pass_rules[0].name, "before move");
    let legacy_dbs = legacy
        .dbs
        .expect("legacy composition must still have a vault");
    let legacy_rules = legacy_dbs
        .secrets
        .list_pass_rules()
        .expect("list via legacy mirror");
    assert_eq!(legacy_rules.len(), 1);
}

#[test]
fn rekey_vault_re_encrypts_and_pass_rules_survive() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_password_rule(PasswordRuleInput {
            name: "before rekey".to_string(),
            pattern: "pattern".to_string(),
            priority: 10,
            enabled: true,
            password: Some(SecretInput::new("rekeyed-vault-password-1a9f".to_string())),
        }))
        .expect("seed a rule before rekeying");

    let new_key_path = temp.path().join("new_master.key");
    arclain_core::SecretsKey::generate()
        .save_to_file(&new_key_path)
        .expect("write the new key file");

    runtime
        .block_on(app.rekey_vault(new_key_path.clone()))
        .expect("rekey_vault must succeed");

    let snapshot = runtime
        .block_on(app.settings())
        .expect("settings must succeed");
    assert_eq!(
        snapshot.security.key_file_path.as_deref(),
        Some(new_key_path.as_path())
    );
    assert!(snapshot.security.vault_available);

    let rules = runtime
        .block_on(app.password_rules())
        .expect("password_rules must succeed");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "before rekey");
}

/// Reproduces production's actual shape, not an artificial stress test:
/// `crates/ui`'s `AppState.dbs` is a long-lived clone of the same
/// `Arc<Mutex<redb::Database>>` the facade's own live vault state wraps
/// (obtained once via `take_legacy_composition` at startup, and again
/// after every facade-driven settings mutation via `refresh_settings_
/// from_facade`) -- it is never dropped just because the facade is about
/// to move or rekey the vault. Holding `_legacy` here across the call is
/// exactly that shape. `move_vault`/`rekey_vault` must succeed regardless
/// of what any earlier `take_legacy_composition` caller is still holding
/// -- the facade, not its callers, is responsible for coordinating
/// around its own vault handle's lifecycle.
#[test]
fn move_vault_succeeds_even_with_an_outstanding_legacy_composition_held() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let _legacy = app.take_legacy_composition().expect("legacy composition");

    let destination = temp.path().join("relocated").join("pass.redb");
    runtime
        .block_on(app.move_vault(destination.clone()))
        .expect(
            "move_vault must succeed even with an outstanding legacy composition held -- this is \
         production's actual shape (crates/ui's AppState.dbs), not a contrived edge case",
        );

    assert!(
        destination.exists(),
        "the vault file must exist at the new location"
    );
}

#[test]
fn rekey_vault_succeeds_even_with_an_outstanding_legacy_composition_held() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let _legacy = app.take_legacy_composition().expect("legacy composition");

    let new_key_path = temp.path().join("new_master.key");
    arclain_core::SecretsKey::generate()
        .save_to_file(&new_key_path)
        .expect("write the new key file");

    runtime.block_on(app.rekey_vault(new_key_path)).expect(
        "rekey_vault must succeed even with an outstanding legacy composition held -- this is \
         production's actual shape (crates/ui's AppState.dbs), not a contrived edge case",
    );
}

/// The pre-facade single-owner `AppState::move_vault` made its one live
/// copy go dark the instant a move started (`self.dbs.take()`); with
/// multiple clones now possible, both the facade's own state *and* any
/// externally-held clone taken *before* the move must go dark together,
/// not just the facade's own. A stale external clone silently succeeding
/// with pre-move data (or hanging) would be a correctness regression
/// this test locks in against.
#[test]
fn move_vault_closes_every_outstanding_clone_of_the_old_secrets_handle() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let legacy_before = app.take_legacy_composition().expect("legacy composition");
    let old_secrets = legacy_before
        .dbs
        .expect("legacy composition must have a vault before the move")
        .secrets;
    // Sanity: the old handle is usable before the move.
    old_secrets
        .get_secret("proxy:socks5")
        .expect("old handle usable before the move");

    let destination = temp.path().join("relocated").join("pass.redb");
    runtime
        .block_on(app.move_vault(destination))
        .expect("move_vault must succeed");

    // The clone taken *before* the move must now fail cleanly -- not
    // silently return stale data, and not hang or corrupt anything.
    let result = old_secrets.get_secret("proxy:socks5");
    assert!(
        result.is_err(),
        "a SecretsDb clone taken before the move must go dark once the vault has moved, \
         matching the pre-facade single-owner behavior"
    );

    // A *fresh* legacy composition (taken after the move) must be fully
    // usable against the new vault.
    let legacy_after = app.take_legacy_composition().expect("legacy composition");
    legacy_after
        .dbs
        .expect("legacy composition must have a vault after the move")
        .secrets
        .get_secret("proxy:socks5")
        .expect("a fresh clone taken after the move must be usable");
}

// ============================================================================
// Shutdown / restart persistence ("shutdown flush").
// ============================================================================

#[test]
fn settings_and_password_rules_survive_shutdown_and_a_fresh_bootstrap() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));

    {
        let runtime = foreign_runtime();
        let app = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(paths.clone()),
            worker_threads: None,
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
        })
        .expect("first bootstrap must succeed");

        runtime.block_on(async {
            app.update_settings(SettingsPatch {
                general: None,
                expected_revision: 0,
                archive: Some(ArchiveSettingsPatch {
                    temp_directory: PatchValue::Set(PathBuf::from("/persisted/temp")),
                    ..keep_archive_patch()
                }),
                network: None,
                security: None,
            })
            .await
            .expect("update_settings must succeed");

            app.set_gameta_api_key(SecretInput::new("persisted-gameta-key-6f2d".to_string()))
                .await
                .expect("set_gameta_api_key must succeed");

            app.upsert_password_rule(PasswordRuleInput {
                name: "persisted rule".to_string(),
                pattern: "pattern".to_string(),
                priority: 10,
                enabled: true,
                password: Some(SecretInput::new("persisted-rule-password-5c31".to_string())),
            })
            .await
            .expect("upsert_password_rule must succeed");

            // Explicit, idempotent shutdown -- nothing here is buffered
            // behind it (every mutation above already persisted
            // synchronously), but a real frontend calls this on exit and
            // this test exercises that it does not itself corrupt or
            // lose anything already on disk.
            app.shutdown().await.expect("shutdown must succeed");
        });
        // `app`/`runtime` drop here, outside any async context.
    }

    let runtime = foreign_runtime();
    let restarted = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("second bootstrap against the same profile must succeed");

    let snapshot = runtime
        .block_on(restarted.settings())
        .expect("settings must succeed");
    assert_eq!(
        snapshot.archive.temp_directory.as_deref(),
        Some(Path::new("/persisted/temp"))
    );
    assert!(snapshot.network.gameta_api_key_configured);

    let rules = runtime
        .block_on(restarted.password_rules())
        .expect("password_rules must succeed");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "persisted rule");
}

// ============================================================================
// Public-DTO constructibility, mirroring `public_contract.rs`'s pattern
// for earlier tasks but scoped to this task's own additions.
// ============================================================================

#[test]
fn constructs_every_public_settings_dto() {
    use arclain_app::settings::{
        ArchiveSettingsDto, GeneralSettingsDto, NetworkSettingsDto, PasswordRuleSummary,
        SecuritySettingsDto, SessionArchiveEntry, SettingsSnapshot,
    };

    let archive = ArchiveSettingsDto {
        backend_mode: BackendModeDto::Native,
        cache_directory: None,
        temp_directory: None,
        transfer_directory: None,
        sevenzip_path: None,
        default_collision_policy: "smart".to_string(),
    };
    let mut plugin_proxy_enabled = std::collections::BTreeMap::new();
    plugin_proxy_enabled.insert("dlsite".to_string(), true);
    let network = NetworkSettingsDto {
        socks5_enabled: false,
        socks5_address: None,
        socks5_username: None,
        socks5_password_configured: false,
        plugin_proxy_enabled,
        gameta_server_enabled: false,
        gameta_server_url: None,
        gameta_api_key_configured: false,
    };
    let security = SecuritySettingsDto {
        secrets_database_path: None,
        key_file_path: None,
        default_secrets_database_path: None,
        default_key_file_path: None,
        encrypted_crc_policy: "on_access".to_string(),
        vault_available: false,
    };
    let general = GeneralSettingsDto {
        hotkey_bindings: None,
        open_nested_in_new_tab: false,
        drop_behavior: "new_tab".to_string(),
        restore_tabs_on_launch: true,
    };
    let snapshot = SettingsSnapshot {
        revision: 0,
        archive,
        network,
        security,
        general,
    };
    assert_eq!(snapshot.revision, 0);

    let rule_summary = PasswordRuleSummary {
        name: "n".to_string(),
        pattern: "p".to_string(),
        priority: 1,
        enabled: true,
        password_configured: true,
    };
    assert!(rule_summary.enabled);

    let rule_input = PasswordRuleInput {
        name: "n".to_string(),
        pattern: "p".to_string(),
        priority: 1,
        enabled: true,
        password: Some(SecretInput::new("s".to_string())),
    };
    assert_eq!(rule_input.name, "n");

    let entry = SessionArchiveEntry {
        source_path: PathBuf::from("/a.zip"),
    };
    assert_eq!(entry.source_path, PathBuf::from("/a.zip"));

    // PatchValue and BackendModeDto round trip through JSON with the
    // adjacently-tagged/snake_case shape the contract specifies.
    let patch_value: PatchValue<bool> = PatchValue::Set(true);
    let json = serde_json::to_string(&patch_value).unwrap();
    assert_eq!(json, r#"{"operation":"set","value":true}"#);
    let clear: PatchValue<bool> = serde_json::from_str(r#"{"operation":"clear"}"#).unwrap();
    assert!(matches!(clear, PatchValue::Clear));
}
