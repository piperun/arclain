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
    /// Refresh the canonical chrome-item signals (toolbar, info panel,
    /// context menu) from the application. Called from the settings
    /// header save handlers after a layout-editor save lands.
    ///
    /// A region that cannot be read keeps its previous signal value
    /// rather than being emptied: the arrangement already on screen is a
    /// better answer than no chrome at all, and the save that triggered
    /// this has already reported its own outcome. Unlike the settings
    /// mirrors, nothing acts destructively on a stale item list.
    ///
    /// Blocks briefly on `runtime` -- see `vault_ops.rs`'s own module doc
    /// comment for why that's the right choice from a synchronous egui
    /// action handler.
    pub fn reload_ui_config(
        &self,
        facade: &arclain_app::ArclainApp,
        runtime: &tokio::runtime::Handle,
    ) {
        for (region, signal) in [
            (
                arclain_app::layout::UiRegionDto::Toolbar,
                &self.signals.toolbar_items,
            ),
            (
                arclain_app::layout::UiRegionDto::InfoPanel,
                &self.signals.info_panel_items,
            ),
            (
                arclain_app::layout::UiRegionDto::ContextMenu,
                &self.signals.context_menu_items,
            ),
        ] {
            match runtime.block_on(facade.list_ui_items(region)) {
                Ok(items) => signal.set(items),
                Err(error) => tracing::warn!(
                    "{}",
                    describe_facade_error(&format!("reloading the {region:?} layout"), error)
                ),
            }
        }
    }

    /// Re-syncs this state's still-legacy settings/vault mirror
    /// (`user_config`, `pass_rules`, `encrypted_crc_policy`, `db_paths`,
    /// `dbs`) from `arclain_app::ArclainApp`'s own live state, and
    /// refreshes the reactive settings signals from the facade's own
    /// snapshot so already-rendered UI picks up the change on the next
    /// frame.
    ///
    /// The signals are filled from `settings()` rather than from the
    /// legacy composition beside them: that is the shape every reader
    /// now works in, and reading it from the facade means the frontend
    /// never has to know how a preference is stored to display it.
    ///
    /// Settings/secrets/vault mutations go through the facade first
    /// (see `vault_ops.rs`/`password_ops.rs`/`settings_controller.rs`);
    /// this is the one place that pulls the result back afterward,
    /// closing the loop the facade's own `take_legacy_composition` doc
    /// comment describes -- call this once, right after any such facade
    /// call succeeds. `backend_selector`/`fallback_backend` are
    /// untouched: no settings/vault mutation ever changes them.
    ///
    /// Blocks briefly on `runtime` -- see `vault_ops.rs`'s own module
    /// doc comment for why that's the right choice from a synchronous
    /// egui action handler.
    pub fn refresh_settings_from_facade(
        &mut self,
        facade: &arclain_app::ArclainApp,
        runtime: &tokio::runtime::Handle,
    ) -> anyhow::Result<()> {
        let legacy = facade
            .take_legacy_composition()
            .map_err(|error| anyhow::anyhow!("refreshing settings from facade: {error:?}"))?;
        self.user_config = legacy.user_config;
        self.pass_rules = legacy.pass_rules;
        self.encrypted_crc_policy = legacy.encrypted_crc_policy;
        self.db_paths = legacy.db_paths;
        self.dbs = legacy.dbs;
        self.signals
            .plugin_visibility
            .set(self.user_config.plugin_visibility.clone());
        self.refresh_settings_signals(facade, runtime)
    }

    /// Fills the reactive settings mirrors from the facade's own
    /// snapshot. Split out of [`Self::refresh_settings_from_facade`] so
    /// startup can populate them once without also re-taking a legacy
    /// composition it already holds.
    ///
    /// **A failure here must not be swallowed at startup.** Until this
    /// lands, the mirrors hold `Default` DTOs, and those are not a
    /// neutral "unknown" -- `GeneralSettingsDto::default()` reports
    /// `restore_tabs_on_launch: false`, which
    /// `app_lifecycle::save_tabs_on_exit_to` acts on by *deleting*
    /// `session.json`. A caller that logs this error and carries on
    /// therefore destroys the user's saved session. `AppState::new`
    /// propagates it for exactly that reason; pinned by
    /// `a_failed_settings_read_leaves_the_destructive_placeholder_in_place`.
    pub fn refresh_settings_signals(
        &self,
        facade: &arclain_app::ArclainApp,
        runtime: &tokio::runtime::Handle,
    ) -> anyhow::Result<()> {
        let snapshot = runtime.block_on(async {
            facade
                .settings()
                .await
                .map_err(|error| describe_facade_error("reading current settings", error))
        })?;
        self.signals.general_settings.set(snapshot.general);
        self.signals.archive_settings.set(snapshot.archive);
        self.signals.network_settings.set(snapshot.network);
        self.signals.security_settings.set(snapshot.security);
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
        runtime: &tokio::runtime::Handle,
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
        self.refresh_settings_from_facade(facade, runtime)?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{app_state_from_facade, bootstrap_test_facade};

    /// The guard on the startup data-loss path.
    ///
    /// A failed settings read leaves the mirrors holding placeholder
    /// DTOs, and the placeholder is *destructive*: its
    /// `restore_tabs_on_launch` is `false`, which
    /// `app_lifecycle::save_tabs_on_exit_to` acts on by deleting
    /// `session.json` (pinned by that module's own
    /// `save_tabs_on_exit_to_clears_a_stale_session_json_when_restore_is_disabled`).
    /// So the error must actually be an error -- a caller that logs it
    /// and continues destroys the user's saved session, which is why
    /// `AppState::new` propagates it with `?` rather than a `warn!`.
    ///
    /// A shut-down facade is the failure: `settings()` returns a real
    /// shutdown error, no mock or injected seam involved.
    #[test]
    fn a_failed_settings_read_leaves_the_destructive_placeholder_in_place() {
        let temp = tempfile::tempdir().expect("create test directory");
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
        // Built before the shutdown below, which also closes
        // `take_legacy_composition`.
        let state = app_state_from_facade(&facade);

        // The placeholder these mirrors start on is the dangerous value:
        // if this ever stops being `false`, the hazard this test guards
        // has changed shape and the test needs revisiting.
        assert!(
            !state.signals.general_settings.read().restore_tabs_on_launch,
            "the un-filled placeholder must still be the value save_tabs_on_exit_to treats as \
             \"delete the saved session\" -- that is what makes swallowing this error destructive"
        );

        runtime
            .block_on(facade.shutdown())
            .expect("shut the facade down");

        let result = state.refresh_settings_signals(&facade, runtime.handle());

        assert!(
            result.is_err(),
            "a settings read that cannot be served must report an error, not quietly leave the \
             mirrors on placeholder values a caller would then act on"
        );
        assert!(
            !state.signals.general_settings.read().restore_tabs_on_launch,
            "a failed read must leave the placeholder untouched -- proving the caller really is \
             the only thing standing between the failure and the deletion"
        );
        assert_eq!(
            state
                .signals
                .archive_settings
                .read()
                .default_collision_policy,
            "smart",
            "no mirror may be half-written by a failed read"
        );
        assert!(!state.signals.security_settings.read().vault_available);
        assert!(state
            .signals
            .network_settings
            .read()
            .socks5_address
            .is_none());
    }

    /// The same read against a healthy facade fills every mirror, so the
    /// test above is observing a real failure rather than a fixture that
    /// could never have succeeded.
    #[test]
    fn a_successful_settings_read_fills_every_mirror() {
        let temp = tempfile::tempdir().expect("create test directory");
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
        let state = app_state_from_facade(&facade);

        state
            .refresh_settings_signals(&facade, runtime.handle())
            .expect("a healthy facade must serve its own settings");

        assert!(
            state.signals.general_settings.read().restore_tabs_on_launch,
            "the seeded profile has restore enabled, so a filled mirror must not read as the \
             placeholder"
        );
        assert!(state.signals.security_settings.read().vault_available);
        assert!(state
            .signals
            .security_settings
            .read()
            .default_secrets_database_path
            .is_some());
    }
}
