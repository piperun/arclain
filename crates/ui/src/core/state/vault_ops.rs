//! Vault and preferences operations
//!
//! The actual persistence and vault coordination this file used to own
//! directly (opening/re-opening SQLCipher-adjacent databases, writing
//! config overrides, reloading password rules) now lives behind
//! `arclain_app::ArclainApp`'s settings facade -- see
//! `ArclainApp::update_settings`/`move_vault`/`rekey_vault`. This file's
//! job is now the same one every other migrated call site has: build a
//! patch/request from the UI's draft form values, submit it to the
//! facade, and mirror the result back into `AppState` via
//! `refresh_settings_from_facade` (`config_ops.rs`) so the ~200
//! not-yet-migrated call sites elsewhere in this crate that still read
//! `AppState.dbs`/`user_config`/`pass_rules`/`db_paths` directly keep
//! seeing accurate values.
//!
//! Settings facade calls are `async`, but every caller here is a plain
//! synchronous egui action handler (`settings_controller.rs`). `runtime`
//! is `shared.services.tokio_runtime` -- in a real bootstrapped app this
//! is, perhaps surprisingly, the *same* `Arc<tokio::runtime::Runtime>`
//! `ArclainApp` uses internally (both derive from one `Arc` `bootstrap::
//! run` constructs and hands to both `CoreServices::new` and its own
//! `RuntimeOwner` -- see that struct's doc comment). That does not make
//! blocking on it from egui's frame callback unsafe: Tokio's actual rule
//! is "never call `block_on` from a thread already driving a task on
//! that runtime", not "never reuse the same `Runtime` object" -- egui's
//! frame thread is driven by eframe/winit's own event loop and never
//! itself runs as one of this runtime's workers, so entering it via
//! `block_on` here is exactly the same safe "foreign thread awaits a
//! facade future" pattern this crate's facade-integration tests already
//! prove (see `crates/app/tests/bootstrap.rs`'s own foreign-runtime
//! tests) -- "foreign" meaning the calling thread, not necessarily a
//! distinct `Runtime` instance. This also costs no more than what this
//! file did before: the actual database I/O these calls perform is the
//! same synchronous rusqlite/redb work that always ran directly inline
//! in the egui frame here, just now routed through the facade instead of
//! touching `self.dbs` by hand.

use super::config_ops::describe_facade_error;
use super::AppState;
use anyhow::Result;
use arclain_app::settings::{PatchValue, SecuritySettingsPatch, SettingsPatch};
use arclain_app::ArclainApp;
use std::path::PathBuf;
use tokio::runtime::Runtime;

fn optional_path_patch(value: Option<String>) -> PatchValue<PathBuf> {
    match value {
        Some(value) => PatchValue::Set(PathBuf::from(value)),
        None => PatchValue::Keep,
    }
}

fn optional_string_patch(value: Option<String>) -> PatchValue<String> {
    match value {
        Some(value) => PatchValue::Set(value),
        None => PatchValue::Keep,
    }
}

impl AppState {
    /// Apply Preferences changes: persist `Set` overrides through the
    /// facade's `update_settings`, then re-sync this state's mirror.
    /// `None` for any parameter means "leave that setting unchanged" --
    /// the same meaning it always had here, now expressed as
    /// `PatchValue::Keep` rather than an omitted `set_config` call.
    pub fn apply_preferences(
        &mut self,
        facade: &ArclainApp,
        runtime: &Runtime,
        key_file_path: Option<String>,
        secrets_db_path: Option<String>,
        encrypted_crc_policy: Option<String>,
    ) -> Result<()> {
        self.submit_settings_patch(facade, runtime, |expected_revision| SettingsPatch {
            expected_revision,
            archive: None,
            network: None,
            security: Some(SecuritySettingsPatch {
                secrets_database_path: optional_path_patch(secrets_db_path),
                key_file_path: optional_path_patch(key_file_path),
                encrypted_crc_policy: optional_string_patch(encrypted_crc_policy),
            }),
        })?;
        Ok(())
    }

    pub fn move_vault(
        &mut self,
        facade: &ArclainApp,
        runtime: &Runtime,
        dest_path: &str,
    ) -> Result<()> {
        let result = runtime
            .block_on(facade.move_vault(PathBuf::from(dest_path)))
            .map_err(|error| describe_facade_error("moving the vault", error));
        self.refresh_mirror_after_vault_operation(facade, result)
    }

    pub fn rekey_vault(
        &mut self,
        facade: &ArclainApp,
        runtime: &Runtime,
        new_key_file_path: &str,
    ) -> Result<()> {
        let result = runtime
            .block_on(facade.rekey_vault(PathBuf::from(new_key_file_path)))
            .map_err(|error| describe_facade_error("rekeying the vault", error));
        self.refresh_mirror_after_vault_operation(facade, result)
    }

    /// Refreshes this state's `dbs`/`user_config`/`pass_rules` mirror
    /// after a vault operation regardless of whether it succeeded or
    /// failed, then returns the operation's own outcome.
    ///
    /// A *failed* `move_vault`/`rekey_vault` still closes the shared
    /// vault handle on its way in, before the actual move/rekey I/O
    /// runs (see `ReDb::close`'s own doc comment in `arclain_db`) --
    /// so skipping the refresh on the error path, as this used to do,
    /// left `self.dbs` holding a stale `Some(_)` whose `secrets` handle
    /// was already permanently closed underneath it. Any of this
    /// crate's ~200 not-yet-migrated call sites that only check
    /// "`self.dbs` is `Some`" to decide the vault is available would
    /// take that branch and then fail at first actual use, instead of
    /// failing closed consistently with the facade's own
    /// `mutable.dbs == None` state. Calling this on both outcomes
    /// keeps the two copies in agreement either way.
    ///
    /// If the refresh *itself* fails while the operation had already
    /// failed, the operation's own (more actionable) error is what the
    /// caller sees -- the refresh failure is logged, not returned, so
    /// one failure never masks the other. If the operation succeeded
    /// but the refresh failed, the refresh error is returned as-is
    /// (there is no more-relevant error to prefer over it).
    fn refresh_mirror_after_vault_operation(
        &mut self,
        facade: &ArclainApp,
        result: Result<()>,
    ) -> Result<()> {
        match self.refresh_settings_from_facade(facade) {
            Ok(()) => result,
            Err(refresh_error) => match result {
                Ok(()) => Err(refresh_error),
                Err(operation_error) => {
                    tracing::warn!(
                        "failed to refresh the settings mirror after a vault operation that \
                         itself failed: {refresh_error:?}"
                    );
                    Err(operation_error)
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::signals::AppSignals;
    use tempfile::TempDir;

    #[cfg(windows)]
    fn sevenzip_exe_name() -> &'static str {
        "7zz.exe"
    }

    #[cfg(not(windows))]
    fn sevenzip_exe_name() -> &'static str {
        "7zz"
    }

    /// Bootstraps a real `ArclainApp` against an isolated temp profile.
    /// Near-identical to `settings_controller.rs`'s own test-module-
    /// private `bootstrap_test_facade` -- see that copy's doc comment
    /// for why this seam is reimplemented per test module rather than
    /// shared (no cross-module test-helper wiring exists in this crate
    /// yet, matching `arclain_app`'s own `tests/support` being
    /// unreachable from here too).
    fn bootstrap_test_facade(temp: &TempDir) -> ArclainApp {
        let paths = arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };

        let sevenzip_path = temp.path().join(sevenzip_exe_name());
        std::fs::write(
            &sevenzip_path,
            b"not a real binary, only its path is checked",
        )
        .expect("write dummy 7-Zip executable");

        let databases_dir = paths.data_dir.join("databases");
        std::fs::create_dir_all(&databases_dir).expect("create databases dir");
        let config_db_path = databases_dir.join("config.sqlite");
        let db = arclain_core::config::ConfigDb::open(&config_db_path).expect("open config db");
        let conn = db.into_sqlite_db();
        conn.with_connection(|conn| {
            arclain_core::UserConfig::ensure_table(conn)?;
            let mut config = arclain_core::UserConfig::new();
            config.sevenzip_path = Some(sevenzip_path.to_string_lossy().into_owned());
            config.save(conn)?;
            Ok(())
        })
        .expect("seed sevenzip_path into test config db");

        ArclainApp::bootstrap(arclain_app::BootstrapConfig {
            paths_override: Some(paths),
            worker_threads: None,
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
        })
        .expect("bootstrap the vault-ops test facade")
    }

    /// Builds an `AppState` mirror from a real facade's current
    /// composition -- the same unpacking `AppState::new`/
    /// `ProxySaveFixture` both do.
    fn app_state_from_facade(facade: &ArclainApp) -> AppState {
        let legacy = facade
            .take_legacy_composition()
            .expect("take legacy composition for the test fixture");
        let signals = AppSignals::new();
        signals.user_config.set(legacy.user_config.clone());
        signals.pass_rules.set(legacy.pass_rules.clone());
        AppState {
            user_config: legacy.user_config,
            pass_rules: legacy.pass_rules,
            backend_selector: legacy.backend_selector,
            fallback_backend: legacy.fallback_backend,
            last_entries: vec![],
            encrypted_crc_policy: legacy.encrypted_crc_policy,
            db_paths: legacy.db_paths,
            dbs: legacy.dbs,
            signals,
        }
    }

    /// The "NB2" fix: a *failed* `move_vault` still closes the shared
    /// vault handle on its way in (`run_move_vault` calls
    /// `close_vault_handle` before ever attempting the actual file
    /// move -- see that function's own doc comment in
    /// `arclain_app::runtime::settings_ops`), so `AppState.dbs`,
    /// obtained earlier via `take_legacy_composition`, is left holding
    /// a `Some(_)` whose `secrets` handle is now permanently closed
    /// unless this mirror is refreshed on the error path too.
    #[test]
    fn move_vault_failure_still_refreshes_the_mirror_to_none() {
        let temp = tempfile::tempdir().unwrap();
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
        let mut state = app_state_from_facade(&facade);
        assert!(
            state.dbs.is_some(),
            "a fresh bootstrap must have a usable vault"
        );

        // A destination whose parent is an existing plain file, not a
        // directory -- `fs::create_dir_all` on it deterministically
        // fails on every platform, forcing `move_vault` to fail after
        // the shared vault handle has already been closed.
        let blocker = temp.path().join("blocker-not-a-directory");
        std::fs::write(&blocker, b"not a directory").expect("write blocking file");
        let dest_path = blocker.join("pass.redb");

        state
            .move_vault(&facade, &runtime, dest_path.to_string_lossy().as_ref())
            .expect_err("moving into a blocked destination must fail");

        assert!(
            state.dbs.is_none(),
            "a failed move_vault must refresh the mirror to None, matching the facade's own \
             mutable.dbs -- not leave a stale Some(_) whose secrets handle is already closed"
        );
    }

    /// Sibling of the test above for `rekey_vault`.
    #[test]
    fn rekey_vault_failure_still_refreshes_the_mirror_to_none() {
        let temp = tempfile::tempdir().unwrap();
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
        let mut state = app_state_from_facade(&facade);
        assert!(
            state.dbs.is_some(),
            "a fresh bootstrap must have a usable vault"
        );

        // A key file that does not exist: `SecretsService::rekey_vault`
        // fails trying to load it, after the shared vault handle has
        // already been closed.
        let missing_key_file = temp.path().join("does-not-exist.key");

        state
            .rekey_vault(
                &facade,
                &runtime,
                missing_key_file.to_string_lossy().as_ref(),
            )
            .expect_err("rekeying with a missing key file must fail");

        assert!(
            state.dbs.is_none(),
            "a failed rekey_vault must refresh the mirror to None, matching the facade's own \
             mutable.dbs -- not leave a stale Some(_) whose secrets handle is already closed"
        );
    }
}
