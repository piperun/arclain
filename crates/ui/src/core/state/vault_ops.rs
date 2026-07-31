//! Vault and preferences operations
//!
//! The actual persistence and vault coordination this file used to own
//! directly (opening/re-opening SQLCipher-adjacent databases, writing
//! config overrides, reloading password rules) now lives behind
//! `arclain_app::ArclainApp`'s settings facade -- see
//! `ArclainApp::update_settings`/`move_vault`/`rekey_vault`. This file's
//! job is now the same one every other migrated call site has: build a
//! patch/request from the UI's draft form values, submit it to the
//! facade, and refresh the frontend's non-secret settings signals.
//!
//! Settings facade calls are `async`, but every caller here is a plain
//! synchronous egui action handler (`settings_controller.rs`). `runtime`
//! is the egui-owned `shared.services.tokio_runtime`, deliberately
//! separate from the runtime `ArclainApp` owns internally. The facade
//! contract explicitly permits awaiting its futures from any executor;
//! its own work dispatches back onto the application runtime. The egui
//! frame thread is driven by eframe/winit, not by this frontend runtime,
//! so `block_on` here does not nest a Tokio runtime. This also costs no
//! more than what this file did before: the actual database I/O these
//! calls perform is the same synchronous rusqlite/redb work that once ran
//! directly inline in the frame, now routed through the facade instead
//! of touching database handles by hand.

use super::config_ops::describe_facade_error;
use super::AppState;
use anyhow::Result;
use arclain_app::settings::{PatchValue, SecuritySettingsPatch, SettingsPatch};
use arclain_app::ArclainApp;
use std::path::PathBuf;
use tokio::runtime::Handle;

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
    /// facade's `update_settings`, then refresh the settings signals.
    /// `None` for any parameter means "leave that setting unchanged" --
    /// the same meaning it always had here, now expressed as
    /// `PatchValue::Keep` rather than an omitted `set_config` call.
    pub fn apply_preferences(
        &self,
        facade: &ArclainApp,
        runtime: &Handle,
        key_file_path: Option<String>,
        secrets_db_path: Option<String>,
        encrypted_crc_policy: Option<String>,
    ) -> Result<()> {
        self.submit_settings_patch(facade, runtime, |expected_revision| SettingsPatch {
            expected_revision,
            archive: None,
            network: None,
            general: None,
            security: Some(SecuritySettingsPatch {
                secrets_database_path: optional_path_patch(secrets_db_path),
                key_file_path: optional_path_patch(key_file_path),
                encrypted_crc_policy: optional_string_patch(encrypted_crc_policy),
            }),
        })?;
        Ok(())
    }

    pub fn move_vault(&self, facade: &ArclainApp, runtime: &Handle, dest_path: &str) -> Result<()> {
        let result = runtime
            .block_on(facade.move_vault(PathBuf::from(dest_path)))
            .map_err(|error| describe_facade_error("moving the vault", error));
        self.refresh_mirror_after_vault_operation(facade, runtime, result)
    }

    pub fn rekey_vault(
        &self,
        facade: &ArclainApp,
        runtime: &Handle,
        new_key_file_path: &str,
    ) -> Result<()> {
        let result = runtime
            .block_on(facade.rekey_vault(PathBuf::from(new_key_file_path)))
            .map_err(|error| describe_facade_error("rekeying the vault", error));
        self.refresh_mirror_after_vault_operation(facade, runtime, result)
    }

    /// Refreshes the frontend's settings signals after a vault operation
    /// regardless of whether it succeeded or failed, then returns the
    /// operation's own outcome.
    ///
    /// A *failed* `move_vault`/`rekey_vault` still closes the shared
    /// vault handle on its way in, before the actual move/rekey I/O
    /// runs (see `ReDb::close`'s own doc comment in `arclain_db`) --
    /// so skipping the refresh on the error path would leave the UI's
    /// `vault_available` signal stale even though the application has
    /// already failed closed.
    ///
    /// If the refresh *itself* fails while the operation had already
    /// failed, the operation's own (more actionable) error is what the
    /// caller sees -- the refresh failure is logged, not returned, so
    /// one failure never masks the other. If the operation succeeded
    /// but the refresh failed, the refresh error is returned as-is
    /// (there is no more-relevant error to prefer over it).
    fn refresh_mirror_after_vault_operation(
        &self,
        facade: &ArclainApp,
        runtime: &Handle,
        result: Result<()>,
    ) -> Result<()> {
        match self.refresh_settings_signals(facade, runtime) {
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
    use crate::test_support::{app_state_from_facade, bootstrap_test_facade};

    /// A failed move closes the application-owned vault before the file
    /// operation runs, so the frontend must refresh its availability
    /// signal even on the error path.
    #[test]
    fn move_vault_failure_still_refreshes_the_mirror_to_none() {
        let temp = tempfile::tempdir().unwrap();
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
        let state = app_state_from_facade(&facade);
        state
            .refresh_settings_signals(&facade, runtime.handle())
            .expect("read initial vault availability");
        assert!(
            state.signals.security_settings.read().vault_available,
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
            .move_vault(
                &facade,
                runtime.handle(),
                dest_path.to_string_lossy().as_ref(),
            )
            .expect_err("moving into a blocked destination must fail");

        assert!(
            !state.signals.security_settings.read().vault_available,
            "a failed move_vault must refresh the signal to unavailable"
        );
    }

    /// Sibling of the test above for `rekey_vault`.
    #[test]
    fn rekey_vault_failure_still_refreshes_the_mirror_to_none() {
        let temp = tempfile::tempdir().unwrap();
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
        let state = app_state_from_facade(&facade);
        state
            .refresh_settings_signals(&facade, runtime.handle())
            .expect("read initial vault availability");
        assert!(
            state.signals.security_settings.read().vault_available,
            "a fresh bootstrap must have a usable vault"
        );

        // A key file that does not exist: `SecretsService::rekey_vault`
        // fails trying to load it, after the shared vault handle has
        // already been closed.
        let missing_key_file = temp.path().join("does-not-exist.key");

        state
            .rekey_vault(
                &facade,
                runtime.handle(),
                missing_key_file.to_string_lossy().as_ref(),
            )
            .expect_err("rekeying with a missing key file must fail");

        assert!(
            !state.signals.security_settings.read().vault_available,
            "a failed rekey_vault must refresh the signal to unavailable"
        );
    }
}
