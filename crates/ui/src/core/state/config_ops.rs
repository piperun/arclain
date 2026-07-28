//! Configuration reload operations
//!
//! The startup-time sync this file used to also hold
//! (`sync_configuration`: seeding organization rules and title filters
//! from TOML defaults into the database) moved into `arclain_app::
//! runtime::bootstrap::run` -- it needed nothing from `AppState` beyond
//! `self.dbs`, which that function now has its own copy of during
//! composition. `sync_configuration`'s only caller
//! (`crates/ui/src/core/state/init.rs`) was removed along with it.

use super::AppState;

/// Formats a structured facade error as a human-presentable one-liner
/// for this crate's still-`anyhow`-based `AppState` methods. `summary`
/// alone is often too generic ("invalid SOCKS5 proxy settings"); when
/// `diagnostic` is present, appending it gives the specific reason
/// (which field, which value). `diagnostic` is bounded and path-redacted
/// at construction (see `arclain_app::error::ApplicationError::
/// with_diagnostic`'s own doc comment), so it is always safe to show a
/// user, not just a log. `context` says which operation was attempting
/// the call, since the facade error itself doesn't know that.
///
/// The one place this crate turns a structured facade error into a
/// plain string -- `vault_ops.rs`/`password_ops.rs` share this via
/// `pub(super)` rather than each keeping their own copy.
pub(super) fn describe_facade_error(
    context: &str,
    error: arclain_app::error::ApplicationError,
) -> anyhow::Error {
    match error.diagnostic {
        Some(diagnostic) => anyhow::anyhow!("{context}: {} ({diagnostic})", error.summary),
        None => anyhow::anyhow!("{context}: {}", error.summary),
    }
}

impl AppState {
    /// Refresh UI configuration (toolbar/info panel/context menu items)
    /// from UiService. Called from the settings header save handlers
    /// after a layout-editor save lands.
    pub fn reload_ui_config(&mut self, ui_service: &arclain_core::UiService) {
        if let Ok(items) = ui_service.list_toolbar_items() {
            self.signals.toolbar_items.set(items);
        }
        if let Ok(items) = ui_service.list_info_panel_items() {
            self.signals.info_panel_items.set(items);
        }
        if let Ok(items) = ui_service.list_items(arclain_core::UiRegion::ContextMenu) {
            self.signals.context_menu_items.set(items);
        }
    }

    /// Re-syncs this state's still-legacy settings/vault mirror
    /// (`user_config`, `pass_rules`, `encrypted_crc_policy`, `db_paths`,
    /// `dbs`) from `arclain_app::ArclainApp`'s own live state, and
    /// updates the two reactive signal mirrors (`user_config`,
    /// `pass_rules`) so already-rendered UI picks up the change on the
    /// next frame.
    ///
    /// Settings/secrets/vault mutations now go through the facade first
    /// (see `vault_ops.rs`/`password_ops.rs`/`settings_controller.rs`);
    /// this is the one place that pulls the result back into `AppState`
    /// afterward, closing the loop the facade's own `take_legacy_
    /// composition` doc comment describes -- call this once, right
    /// after any such facade call succeeds. `backend_selector`/
    /// `fallback_backend`/`last_entries` are untouched: nothing this
    /// task's facade surface does ever changes them.
    pub fn refresh_settings_from_facade(
        &mut self,
        facade: &arclain_app::ArclainApp,
    ) -> anyhow::Result<()> {
        let legacy = facade
            .take_legacy_composition()
            .map_err(|error| anyhow::anyhow!("refreshing settings from facade: {error:?}"))?;
        self.user_config = legacy.user_config;
        self.pass_rules = legacy.pass_rules;
        self.encrypted_crc_policy = legacy.encrypted_crc_policy;
        self.db_paths = legacy.db_paths;
        self.dbs = legacy.dbs;
        self.signals.user_config.set(self.user_config.clone());
        self.signals.pass_rules.set(self.pass_rules.clone());
        Ok(())
    }

    /// Reads the facade's current settings revision, lets `build_patch`
    /// construct the archive/network/security sub-patches against it,
    /// submits the result, and refreshes this state's mirror on success.
    ///
    /// Every settings-page save handler that touches an archive/network/
    /// security field goes through this, so none of them separately
    /// juggles "read the current revision, then build my patch against
    /// it" -- see `settings_controller.rs`'s `SaveArchives`/`SaveNetwork`/
    /// `SaveServer` handlers and `vault_ops.rs::apply_preferences`.
    /// Blocks briefly on `runtime` -- see `vault_ops.rs`'s own module doc
    /// comment for why that's the right choice from a synchronous egui
    /// action handler.
    pub fn submit_settings_patch(
        &mut self,
        facade: &arclain_app::ArclainApp,
        runtime: &tokio::runtime::Runtime,
        build_patch: impl FnOnce(u64) -> arclain_app::settings::SettingsPatch,
    ) -> anyhow::Result<arclain_app::settings::SettingsSnapshot> {
        let snapshot = runtime.block_on(async {
            let current = facade
                .settings()
                .await
                .map_err(|error| describe_facade_error("reading current settings", error))?;
            facade
                .update_settings(build_patch(current.revision))
                .await
                .map_err(|error| describe_facade_error("saving settings", error))
        })?;
        self.refresh_settings_from_facade(facade)?;
        Ok(snapshot)
    }
}
