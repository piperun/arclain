//! Unified Plugin Page
//!
//! Coordinator for plugin list and detail views.
//! Dispatches rendering to the appropriate view based on state.

use crate::features::plugins::application::PluginSlot;
use crate::features::plugins::domain::types::PluginsListState;
use crate::features::settings::domain::types::SettingsAction;

use crate::shared::image_assets::ImageOwner;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use eframe::egui;

/// Releases everything the detail view holds on `plugin_id`'s behalf, for
/// the moment that plugin stops being the selected one.
///
/// Two resources, one lifetime: the images its `MainPage` document
/// referenced, and the facade session that produced that document. They
/// are freed together because they are acquired together -- the detail
/// view is the only host of either, and a `MainPage` slot is
/// window-scoped, so `PluginSessions::retain_hosts` (which only reaches
/// tab-scoped slots) will never sweep it. Leaving the session open would
/// also mean returning to a plugin re-drew a document fetched before the
/// user left, where every pre-facade path re-read it.
///
/// Called from both places a selection can end -- the render-time
/// comparison below and the header's Back button -- so the two cannot
/// drift apart on what "no longer selected" releases.
fn release_selected_plugin(shared: &SharedState, plugin_id: &str) {
    if let Some(facade) = shared.facade.as_ref() {
        shared.plugin_sessions.close(
            facade,
            shared.services.tokio_runtime.handle(),
            &PluginSlot::MainPage {
                plugin_id: plugin_id.to_string(),
            },
        );
    }
    shared
        .image_assets
        .release_owner(&ImageOwner::plugin_settings(plugin_id));
}

/// Render the Plugin Page (coordinator)
/// Dispatches to list_view or detail_view based on selection state
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut PluginsListState,
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    let action = None;
    let selected_before_render = state.selected_plugin.clone();

    // `PluginsFeature` holds two independent `PluginsListState`s (the
    // standalone Plugins page and the Plugins settings page), each
    // reaching this same coordinator with only its own state in scope --
    // a toggle applied through one is otherwise invisible to the other's
    // own `snapshot_status` gate. Every render of *either* page passes
    // through here, so comparing against the shared epoch on every call
    // (not just the one where a toggle happened) is what lets the
    // *other* page notice on its own next render -- see `AppSignals::
    // plugin_list_epoch`'s own doc comment for why this is an epoch
    // rather than a one-shot flag.
    if let Some(shared) = shared {
        let current_epoch = shared
            .signals()
            .plugin_list_epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        sync_plugin_list_epoch(state, current_epoch);
    }

    if state.selected_plugin.is_some() {
        // Detail View
        let needs_refresh = crate::features::plugins::presentation::views::detail_view::render(
            ui, theme, state, shared,
        );

        if needs_refresh {
            state.invalidate_snapshot();
            if let Some(shared) = shared {
                shared.plugin_ui_jobs.invalidate_plugin_snapshots();
                crate::features::plugins::application::request_plugin_snapshot(shared, state);
            }
        }
    } else {
        // List View
        crate::features::plugins::presentation::views::list_view::render(ui, theme, state);
    }

    if state.selected_plugin != selected_before_render {
        if let (Some(shared), Some(previous_plugin_id)) = (shared, selected_before_render) {
            release_selected_plugin(shared, &previous_plugin_id);
        }
    }

    action
}

/// Invalidates `state`'s snapshot if `current_epoch` (`AppSignals::
/// plugin_list_epoch`, read fresh by every call to [`render`]) has moved
/// past the value `state` last synced against, recording the new value
/// either way. Extracted from `render` so the cross-`PluginsListState`
/// invalidation this exists for (see `AppSignals::plugin_list_epoch`'s own
/// doc comment) can be unit-tested directly without an `egui::Ui`.
fn sync_plugin_list_epoch(state: &mut PluginsListState, current_epoch: u64) {
    if state.plugin_list_epoch_seen != current_epoch {
        state.plugin_list_epoch_seen = current_epoch;
        state.invalidate_snapshot();
    }
}

/// Generate header configuration for the Plugins page
///
/// Takes the whole [`SharedState`] rather than just the image store
/// because Back is a deselection like any other, and a deselection
/// releases both of the detail view's holdings (see
/// [`release_selected_plugin`]). It cannot fall through to `render`'s own
/// comparison: this closure runs during the header, so by the time the
/// page body renders the selection has already changed and the
/// comparison sees no difference.
pub fn get_header_config<'a>(
    state: &'a mut PluginsListState,
    page: &crate::core::SettingsPage,
    install_clicked_cell: &'a std::cell::Cell<bool>,
    shared: &'a SharedState,
) -> crate::features::settings::presentation::views::header_config::SettingsHeaderConfig<'a> {
    use crate::features::settings::presentation::views::header_config::SettingsHeaderConfig;

    // Check if we are in Detail View
    if let Some(plugin_id) = state.selected_plugin.clone() {
        if let Some(plugin) = state.plugins.iter().find(|p| &p.id == &plugin_id) {
            let selected_plugin = &mut state.selected_plugin;

            let mut config = SettingsHeaderConfig::new(&plugin.name)
                .sub_description(format!(
                    "v{} by {}",
                    plugin.version,
                    plugin.author.as_deref().unwrap_or("Unknown")
                ))
                .has_changes(false) // Plugin settings save immediately, no Save button needed
                .on_back(move || {
                    release_selected_plugin(shared, &plugin_id);
                    *selected_plugin = None;
                });

            // Add actual plugin description if available
            if let Some(desc) = &plugin.description {
                config = config.description(desc.clone());
            }

            return config;
        }
    }

    // Default List View Header
    let filter_text = &mut state.filter_text;
    let show_permissions = &mut state.show_permissions;
    let show_disabled = &mut state.show_disabled;

    SettingsHeaderConfig::new(page.display_name())
        .icon(page.icon())
        .description(page.description())
        .secondary_row(move |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    crate::shared::components::SearchBar::new(filter_text)
                        .hint("Search plugins...")
                        .width(200.0),
                );
                ui.add_space(8.0);
                if ui
                    .add(arclain_widgets::TextButton::new(
                        "+ Install Plugin",
                        arclain_widgets::ButtonSize::Medium,
                    ))
                    .clicked()
                {
                    install_clicked_cell.set(true);
                }
            });
        })
        .tertiary_row(move |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(show_permissions, "Show Permission Tags");
                ui.add_space(16.0);
                ui.checkbox(show_disabled, "Show Disabled");
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::plugins::domain::types::SnapshotStatus;

    fn ready_state() -> PluginsListState {
        let mut state = PluginsListState::default();
        state.snapshot_status = SnapshotStatus::Ready;
        state
    }

    #[test]
    fn sync_plugin_list_epoch_is_a_no_op_when_already_current() {
        let mut state = ready_state();
        state.plugin_list_epoch_seen = 3;

        sync_plugin_list_epoch(&mut state, 3);

        assert_eq!(state.snapshot_status, SnapshotStatus::Ready);
        assert_eq!(state.plugin_list_epoch_seen, 3);
    }

    #[test]
    fn sync_plugin_list_epoch_invalidates_a_stale_state_and_records_the_new_epoch() {
        let mut state = ready_state();
        state.plugin_list_epoch_seen = 1;

        sync_plugin_list_epoch(&mut state, 2);

        assert_eq!(
            state.snapshot_status,
            SnapshotStatus::Idle,
            "a state that missed a toggle made through its sibling PluginsListState \
             must re-fetch rather than keep showing the stale enabled flag"
        );
        assert_eq!(state.plugin_list_epoch_seen, 2);
    }

    /// The scenario the fix exists for: `PluginsFeature`'s two independent
    /// states (standalone Plugins page, Plugins settings page) both start
    /// in sync; a toggle bumps the shared epoch once; each state's own
    /// *next* render call independently notices and invalidates -- neither
    /// consumes the signal in a way that hides it from the other.
    #[test]
    fn sync_plugin_list_epoch_reaches_both_sibling_states_after_one_bump() {
        let mut list_state = ready_state();
        let mut settings_list_state = ready_state();
        list_state.plugin_list_epoch_seen = 5;
        settings_list_state.plugin_list_epoch_seen = 5;

        let bumped_epoch = 6;
        sync_plugin_list_epoch(&mut list_state, bumped_epoch);
        sync_plugin_list_epoch(&mut settings_list_state, bumped_epoch);

        assert_eq!(list_state.snapshot_status, SnapshotStatus::Idle);
        assert_eq!(settings_list_state.snapshot_status, SnapshotStatus::Idle);
    }
}
