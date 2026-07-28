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
        runtime
            .block_on(facade.move_vault(PathBuf::from(dest_path)))
            .map_err(|error| describe_facade_error("moving the vault", error))?;
        self.refresh_settings_from_facade(facade)
    }

    pub fn rekey_vault(
        &mut self,
        facade: &ArclainApp,
        runtime: &Runtime,
        new_key_file_path: &str,
    ) -> Result<()> {
        runtime
            .block_on(facade.rekey_vault(PathBuf::from(new_key_file_path)))
            .map_err(|error| describe_facade_error("rekeying the vault", error))?;
        self.refresh_settings_from_facade(facade)
    }
}
