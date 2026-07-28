//! Integration tests for the settings/secrets/vault facade surface
//! (`ArclainApp::settings`/`update_settings`/`organization_profiles`/
//! `set_gameta_api_key`/`set_socks5_password`/`move_vault`/`rekey_vault`/
//! `password_rules`/`upsert_password_rule`/`delete_password_rule`).
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

use std::path::{Path, PathBuf};

use arclain_app::challenge::SecretInput;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::settings::{
    ArchiveSettingsPatch, BackendModeDto, NetworkSettingsPatch, PasswordRuleInput, PatchValue,
    SecuritySettingsPatch, SettingsPatch,
};
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
/// `archive_sessions.rs::bootstrap_app`'s identical doc comment for why
/// the dummy 7-Zip seeding is required even though no test here touches
/// an archive backend at all (`BackendSelector::select`'s fallback probe
/// runs unconditionally during bootstrap).
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

fn keep_archive_patch() -> ArchiveSettingsPatch {
    ArchiveSettingsPatch {
        backend_mode: PatchValue::Keep,
        cache_directory: PatchValue::Keep,
        temp_directory: PatchValue::Keep,
        transfer_directory: PatchValue::Keep,
        sevenzip_path: PatchValue::Keep,
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

fn no_op_patch(expected_revision: u64) -> SettingsPatch {
    SettingsPatch {
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

    assert_eq!(snapshot.security.encrypted_crc_policy, "on_access");
    assert!(snapshot.security.vault_available);
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

#[test]
fn organization_profiles_lists_seeded_system_defaults() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let profiles = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");

    assert!(
        !profiles.is_empty(),
        "a fresh bootstrap seeds default archive profiles"
    );
    for profile in &profiles {
        assert!(!profile.id.is_empty());
        assert!(!profile.name.is_empty());
        assert!(!profile.output_format.is_empty());
    }
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

#[test]
fn update_settings_changes_directories_and_network_fields_together() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let patch = SettingsPatch {
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

#[test]
fn clear_on_the_plugin_proxy_map_resets_it_to_empty() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut map = std::collections::BTreeMap::new();
    map.insert("dlsite".to_string(), false);
    let set_patch = SettingsPatch {
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
        ArchiveSettingsDto, NetworkSettingsDto, OrganizationProfileSummary, PasswordRuleSummary,
        SecuritySettingsDto, SessionArchiveEntry, SettingsSnapshot,
    };

    let archive = ArchiveSettingsDto {
        backend_mode: BackendModeDto::Native,
        cache_directory: None,
        temp_directory: None,
        transfer_directory: None,
        sevenzip_path: None,
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
        encrypted_crc_policy: "on_access".to_string(),
        vault_available: false,
    };
    let snapshot = SettingsSnapshot {
        revision: 0,
        archive,
        network,
        security,
    };
    assert_eq!(snapshot.revision, 0);

    let profile = OrganizationProfileSummary {
        id: "1".to_string(),
        name: "profile".to_string(),
        output_format: "7z".to_string(),
    };
    assert_eq!(profile.output_format, "7z");

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
