//! Plugin detail view
//!
//! Renders plugin settings, permissions, and custom UI when a plugin is selected.

use crate::features::plugins::domain::types::PluginsListState;

use crate::features::plugins::presentation::rendering as ui;

use crate::shared::components::Form;
use crate::shared::image_assets::ImageOwner;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_core::utilities::effective_plugin_proxy_map;
use arclain_core::UserConfig;
use arclain_widgets::toggle_switch::ToggleSwitch;
use arclain_widgets::Chips;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

fn plugin_proxy_toggle_value(user_config: &UserConfig, plugin_id: &str) -> bool {
    effective_plugin_proxy_map(user_config)
        .get(plugin_id)
        .copied()
        .unwrap_or(false)
}

/// The raw (sparse, override-only) per-plugin proxy map with `plugin_id`'s
/// entry set to `enabled` -- the shape `NetworkSettingsPatch::
/// plugin_proxy_enabled` persists (a full `Set` replaces the whole map, so
/// this must carry forward every *other* plugin's existing override, not
/// just this one). Pure: builds the patch's payload; persisting it and
/// applying live routing is the facade's `update_settings`'s job (see
/// `render`'s Proxy Settings toggle handler).
fn plugin_proxy_override_map(
    user_config: &UserConfig,
    plugin_id: &str,
    enabled: bool,
) -> std::collections::BTreeMap<String, bool> {
    let mut settings = user_config.get_plugin_proxy_settings();
    settings.insert(plugin_id.to_string(), enabled);
    settings.into_iter().collect()
}

/// Render the plugin detail view
/// Returns true if the plugin list needs to be refreshed
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut PluginsListState,
    app_state: &Arc<Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
    _content_cache: Option<&Arc<arclain_core::ContentCache>>,
) -> bool {
    let mut needs_refresh = false;

    // Drop cached MainPage layout if the user switched plugins, so
    // render_plugin_ui fetches the new plugin's layout on the next
    // call below (audit P4).
    invalidate_main_layout_on_plugin_change(state);

    let selected_id = match &state.selected_plugin {
        Some(id) => id.clone(),
        None => return false,
    };

    let plugin_info = match state.plugins.iter().find(|p| p.id == selected_id) {
        Some(info) => info.clone(),
        None => {
            // ID not found, reset
            state.selected_plugin = None;
            return false;
        }
    };

    // Fetch whitelist entries for this plugin
    let whitelist_entries = if let Some(shared) = shared {
        let whitelist = shared.services.domain_whitelist.read();
        whitelist
            .get_all_entries()
            .into_iter()
            .filter(|e| e.plugin_id == selected_id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    Form::new().show(ui, theme, |ui| {
        // Global Settings
        crate::shared::components::settings_form::SectionHeader::new("Global Settings")
            .show(ui, &theme.colors);

        // Enabled/Disabled Toggle using SettingsRow
        let mut enabled = plugin_info.enabled;
        crate::shared::components::settings_form::SettingsRow::new("Plugin Status")
            .description("Enable or disable this plugin completely.")
            .action(|ui| {
                if ui
                    .add(ToggleSwitch::new(&mut enabled).icons(
                        egui_phosphor::regular::LIGHTNING,
                        egui_phosphor::regular::POWER,
                    ))
                    .changed()
                {
                    let Some(shared) = shared else {
                        return;
                    };
                    let Some(facade) = shared.facade.as_ref() else {
                        tracing::error!(
                            "Failed to save plugin enabled state: application facade is unavailable"
                        );
                        shared.toaster.lock().error(
                            "Plugin enabled state was not saved: application facade is unavailable",
                        );
                        return;
                    };
                    let result = shared
                        .services
                        .tokio_runtime
                        .block_on(facade.set_plugin_enabled(plugin_info.id.clone(), enabled));
                    match result {
                        Ok(()) => {
                            // The document tree a MainPage/PluginButton/Panel
                            // session already fetched may depend on this
                            // plugin's enabled state (e.g. a disabled
                            // plugin's toolbar button should disappear) --
                            // drop every cached snapshot/layout so the next
                            // frame re-fetches instead of showing stale data.
                            shared.plugin_ui_jobs.invalidate_plugin_snapshots();
                            shared.plugin_ui_jobs.invalidate_chrome_snapshot();
                            shared.plugin_ui_jobs.invalidate_all_layouts();
                            // Facade-backed slots hold a live session
                            // against the plugin's old enabled state;
                            // closing them makes the next frame open
                            // against the new one.
                            shared.plugin_sessions.close_plugin(
                                facade,
                                shared.services.tokio_runtime.handle(),
                                &plugin_info.id,
                            );
                            // Bumps the shared epoch `plugins_page::render`
                            // checks for *both* of `PluginsFeature`'s
                            // independent list states (this detail view
                            // only has the one -- see that field's own doc
                            // comment for why an epoch, not a direct
                            // cross-state call, is what reaches the other).
                            shared
                                .signals()
                                .plugin_list_epoch
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            needs_refresh = true;
                        }
                        Err(error) => {
                            tracing::error!("Failed to save plugin enabled state: {error:?}");
                            shared.toaster.lock().error(format!(
                                "Plugin enabled state was not saved: {}",
                                error.summary
                            ));
                        }
                    }
                }
            })
            .show(ui, &theme.colors);

        // Proxy Settings
        let proxy_enabled = {
            let app = app_state.lock();
            plugin_proxy_toggle_value(&app.user_config, &plugin_info.id)
        };
        let mut proxy_toggle_val = proxy_enabled;

        crate::shared::components::settings_form::SettingsRow::new("Network Proxy")
            .description("Route this plugin's traffic through the configured SOCKS5 proxy.")
            .action(|ui| {
                if ui.add(ToggleSwitch::new(&mut proxy_toggle_val)).changed() {
                    let Some(shared) = shared else {
                        return;
                    };
                    let Some(facade) = shared.facade.as_ref() else {
                        tracing::error!(
                            "Failed to save plugin proxy setting: application facade is unavailable"
                        );
                        shared.toaster.lock().error(
                            "Plugin proxy setting was not saved: application facade is unavailable",
                        );
                        return;
                    };

                    let mut app = app_state.lock();
                    let plugin_proxy_enabled =
                        plugin_proxy_override_map(&app.user_config, &plugin_info.id, proxy_toggle_val);
                    let patch_result = app.submit_settings_patch(
                        facade,
                        &shared.services.tokio_runtime,
                        |expected_revision| arclain_app::settings::SettingsPatch {
                            expected_revision,
                            archive: None,
                            general: None,
                            security: None,
                            network: Some(arclain_app::settings::NetworkSettingsPatch {
                                socks5_enabled: arclain_app::settings::PatchValue::Keep,
                                socks5_address: arclain_app::settings::PatchValue::Keep,
                                socks5_username: arclain_app::settings::PatchValue::Keep,
                                plugin_proxy_enabled: arclain_app::settings::PatchValue::Set(
                                    plugin_proxy_enabled.clone(),
                                ),
                                gameta_server_enabled: arclain_app::settings::PatchValue::Keep,
                                gameta_server_url: arclain_app::settings::PatchValue::Keep,
                            }),
                        },
                    );
                    match patch_result {
                        // Live routing (`async_http_client.apply_plugin_proxy_map`)
                        // is already applied by `update_settings` itself
                        // (see `run_update_settings`'s `touches_plugin_proxy_map`
                        // branch) -- no separate call needed here.
                        Ok(_) => needs_refresh = true,
                        Err(error) => {
                            tracing::error!("Failed to save plugin proxy setting: {error}");
                            shared.toaster.lock().error(
                                "Plugin proxy setting was not saved: configuration persistence failed",
                            );
                        }
                    }
                }
            })
            .show(ui, &theme.colors);

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Permissions / Capabilities
        crate::shared::components::settings_form::SectionHeader::new("Permissions")
            .show(ui, &theme.colors);
        if plugin_info.capabilities.is_empty() {
            ui.label(
                egui::RichText::new("None declared")
                    .italics()
                    .color(theme.colors.on_surface_variant),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for cap in &plugin_info.capabilities {
                    ui.add(Chips::new(cap));
                }
            });
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(16.0);

        // Domain Access
        crate::shared::components::settings_form::SectionHeader::new("Domain Access")
            .show(ui, &theme.colors);

        if whitelist_entries.is_empty() {
            ui.label(
                egui::RichText::new("No network domains requested.")
                    .italics()
                    .color(theme.colors.on_surface_variant),
            );
        } else {
            for entry in &whitelist_entries {
                if render_domain_row(ui, theme, entry, shared) {
                    needs_refresh = true;
                }
                ui.add_space(8.0);
            }
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(16.0);

        // Plugin Custom Settings
        crate::shared::components::settings_form::SectionHeader::new("Plugin Configuration")
            .show(ui, &theme.colors);

        if plugin_info.loaded {
            if let Some(shared) = shared {
                render_plugin_ui(
                    ui,
                    theme,
                    &plugin_info.id,
                    shared,
                    &mut state.cached_main_layout,
                );
            }
        } else {
            ui.label(
                egui::RichText::new("Plugin is not loaded.")
                    .color(theme.colors.on_surface_variant),
            );
        }
    });

    needs_refresh
}

fn render_domain_row(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entry: &arclain_network::features::whitelist::WhitelistEntry,
    shared: Option<&SharedState>,
) -> bool {
    let mut changed = false;
    let domain = &entry.domain;
    let is_approved = entry.approved;

    ui.horizontal(|ui| {
        // Status Icon
        if is_approved {
            ui.label(
                egui::RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                    .color(theme.colors.success)
                    .size(16.0),
            );
        } else {
            ui.label(
                egui::RichText::new(egui_phosphor::regular::WARNING)
                    .color(theme.colors.warning)
                    .size(16.0),
            );
        }

        ui.vertical(|ui| {
            // Explicit color: in egui 0.33, RichText::strong() switches the
            // colour to visuals.strong_text_color() and bypasses our
            // override_text_color, which made the domain render as black on
            // the dark plugin-detail background.
            ui.label(
                egui::RichText::new(domain)
                    .strong()
                    .color(theme.colors.on_surface),
            );

            // Security Analysis
            let url_for_check = format!("https://{}", domain);
            if let Ok(info) = arclain_network::features::security::analyze_url(&url_for_check) {
                if !info.warnings.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for warning in info.warnings {
                            ui.label(
                                egui::RichText::new(format!("⚠ {}", warning.description()))
                                    .small()
                                    .color(theme.colors.error),
                            );
                        }
                    });
                }
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut approved_state = is_approved;
            if ui.add(ToggleSwitch::new(&mut approved_state)).changed() {
                // Audit reactive-smells: "mutates whitelist signal then
                // DB in non-atomic sequence" — if the DB write failed,
                // the in-memory whitelist diverged from the on-disk
                // truth and stayed approved across restarts only if
                // DB happened to win on the next save.
                //
                // Safer ordering: persist to DB first, mirror to the
                // in-memory whitelist only on success. A failed DB
                // write now keeps both halves consistent.
                if let Some(shared) = shared {
                    let plugin_id = entry.plugin_id.as_str();
                    let db_result =
                        if let Some(config_svc) = shared.services.config_service.as_ref() {
                            if approved_state {
                                config_svc.approve_plugin_domain(plugin_id, domain)
                            } else {
                                config_svc.revoke_plugin_domain(plugin_id, domain)
                            }
                        } else {
                            // No config service wired: skip the DB
                            // step and keep the in-memory mirror
                            // working (test/headless contexts).
                            Ok(())
                        };

                    if let Err(e) = &db_result {
                        tracing::error!(
                            "Failed to {} domain '{}' for plugin '{}': {}",
                            if approved_state { "approve" } else { "revoke" },
                            domain,
                            plugin_id,
                            e,
                        );
                    } else {
                        let wl = shared.services.domain_whitelist.write();
                        if approved_state {
                            wl.approve(plugin_id, domain);
                        } else {
                            wl.revoke(plugin_id, domain);
                        }
                        changed = true;
                    }
                }
            }
        });
    });

    changed
}

/// Render the plugin's custom UI elements
fn render_plugin_ui(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_id: &str,
    shared: &SharedState,
    cached_main_layout: &mut Option<(String, Arc<arclain_plugins::types::PluginLayout>)>,
) {
    let origin_tab = shared.signals().tabs.get().active_id();
    // Audit P4: cached_main_layout serves the layout for the
    // currently-selected plugin. Fetch fresh only when the cache is
    // empty or holds a different plugin's layout. Re-fetches happen
    // when the cache is explicitly invalidated (selected plugin
    // change in the parent render, or `RefreshPanel` action targeting
    // `MainPage` drained on the next frame).
    let cache_hit_for_this_plugin = cached_main_layout
        .as_ref()
        .is_some_and(|(id, _)| id == plugin_id);

    let ui_result = if cache_hit_for_this_plugin {
        cached_main_layout
            .as_ref()
            .map(|(_, layout)| Ok(layout.clone()))
    } else {
        let layout = shared.plugin_ui_jobs.layout(
            plugin_id,
            crate::features::plugins::application::PluginUiTarget::MainPage,
            Some(origin_tab),
        );
        if let Some(Ok(ref layout)) = layout {
            *cached_main_layout = Some((plugin_id.to_string(), layout.clone()));
        }
        layout
    };

    match ui_result {
        Some(Ok(ui_elements)) => {
            if ui_elements.is_empty() {
                ui.label(
                    egui::RichText::new("This plugin does not provide configuration.")
                        .color(theme.colors.on_surface_variant),
                );
            } else {
                let plugin_id_clone = plugin_id.to_string();
                // Render path requires a SharedState — render_plugin_ui
                // returned earlier above when shared is None. Cloning a
                // SharedState is just refcount bumps; no allocations.
                let shared_clone = shared.clone();

                let mut event_callback = Box::new(move |id: &str, value: Option<String>| {
                    crate::features::plugins::presentation::dispatch::dispatch_plugin_event_for_tab(
                        &shared_clone,
                        origin_tab,
                        plugin_id_clone.clone(),
                        id.to_string(),
                        value,
                    );
                }) as ui::UiEventCallback;

                let mut render = |elements: &[arclain_plugins::types::PluginUiElement]| {
                    let image_owner = ImageOwner::plugin_settings(plugin_id);
                    ui::render_ui_elements_owned(
                        ui,
                        elements,
                        &mut event_callback,
                        &theme.colors,
                        Some(shared),
                        Some(plugin_id),
                        Some(&image_owner),
                    );
                };
                match ui_elements.as_ref() {
                    arclain_plugins::types::PluginLayout::Single { elements } => render(elements),
                    arclain_plugins::types::PluginLayout::Split {
                        sidebar, content, ..
                    } => {
                        render(sidebar);
                        render(content);
                    }
                }
            }
        }
        Some(Err(error)) => {
            ui.label(
                egui::RichText::new(format!("Plugin UI error: {error}")).color(theme.colors.error),
            );
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new("Plugin is busy...")
                        .italics()
                        .color(theme.colors.on_surface_variant),
                );
            });
        }
    }
}

/// Drop `state.cached_main_layout` if it doesn't belong to the
/// currently-selected plugin. Called once per render of the detail
/// view, before `render_plugin_ui` reads the cache.
pub(crate) fn invalidate_main_layout_on_plugin_change(state: &mut PluginsListState) {
    if let (Some(sel), Some((cached_id, _))) = (
        state.selected_plugin.as_ref(),
        state.cached_main_layout.as_ref(),
    ) {
        if sel != cached_id {
            state.cached_main_layout = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_core::UserConfig;

    #[test]
    fn plugin_proxy_toggle_uses_inherited_dlsite_defaults() {
        let mut config = UserConfig::new();
        config.socks5_enabled = true;

        assert!(plugin_proxy_toggle_value(&config, "dlsite"));
        assert!(plugin_proxy_toggle_value(&config, "dlsite-metadata"));
        assert!(plugin_proxy_toggle_value(&config, "dlsite-api"));
        assert!(plugin_proxy_toggle_value(&config, "dlsite-html"));
        assert!(!plugin_proxy_toggle_value(&config, "custom"));
    }

    #[test]
    fn plugin_proxy_toggle_preserves_explicit_overrides() {
        let mut config = UserConfig::new();
        config.socks5_enabled = true;
        config.set_plugin_proxy_enabled("dlsite-api", false);
        config.set_plugin_proxy_enabled("custom", true);

        assert!(!plugin_proxy_toggle_value(&config, "dlsite-api"));
        assert!(plugin_proxy_toggle_value(&config, "custom"));
    }

    #[test]
    fn plugin_proxy_override_map_preserves_other_explicit_overrides() {
        let mut config = UserConfig::new();
        config.socks5_enabled = true;
        config.set_plugin_proxy_enabled("dlsite-api", false);

        let map = plugin_proxy_override_map(&config, "custom", true);

        assert_eq!(map.get("custom"), Some(&true));
        assert_eq!(map.get("dlsite-api"), Some(&false));
        // Never-overridden plugins are not baked in here -- the raw stored
        // map carries only explicit overrides; `dlsite`/`dlsite-html`'s
        // inherited defaults resolve at read time via
        // `effective_plugin_proxy_map`, not at write time.
        assert_eq!(map.get("dlsite"), None);
        assert_eq!(map.get("dlsite-html"), None);
    }

    #[test]
    fn plugin_proxy_override_map_updates_an_existing_override_in_place() {
        let mut config = UserConfig::new();
        config.set_plugin_proxy_enabled("existing", true);

        let map = plugin_proxy_override_map(&config, "existing", false);

        assert_eq!(map.get("existing"), Some(&false));
        assert_eq!(map.len(), 1);
    }

    /// Regression test for P4 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// `cached_main_layout` is per-plugin. When the user switches the
    /// selected plugin in the detail view, the cache from the previous
    /// plugin must be dropped so render_plugin_ui fetches the new
    /// plugin's layout (rather than reusing the stale one for the
    /// wrong plugin).
    #[test]
    fn p4_invalidate_main_layout_when_plugin_changes() {
        let mut state = PluginsListState::default();
        state.selected_plugin = Some("plugin_b".to_string());
        state.cached_main_layout = Some((
            "plugin_a".to_string(),
            Arc::new(arclain_plugins::types::PluginLayout::default()),
        ));

        invalidate_main_layout_on_plugin_change(&mut state);

        assert!(
            state.cached_main_layout.is_none(),
            "Cache held plugin_a's layout while plugin_b is selected; should have dropped",
        );
    }

    /// Same selected plugin → keep the cache.
    #[test]
    fn p4_keep_main_layout_when_same_plugin() {
        let mut state = PluginsListState::default();
        state.selected_plugin = Some("plugin_a".to_string());
        state.cached_main_layout = Some((
            "plugin_a".to_string(),
            Arc::new(arclain_plugins::types::PluginLayout::default()),
        ));

        invalidate_main_layout_on_plugin_change(&mut state);

        assert!(
            state.cached_main_layout.is_some(),
            "Cache for plugin_a should not be dropped when plugin_a is still selected",
        );
    }
}
