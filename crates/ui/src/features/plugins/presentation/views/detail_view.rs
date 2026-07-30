//! Plugin detail view
//!
//! Renders plugin settings, permissions, and custom UI when a plugin is selected.

use crate::features::plugins::domain::types::PluginsListState;

use crate::features::plugins::presentation::rendering as ui;

use crate::shared::components::Form;
use crate::shared::image_assets::ImageOwner;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_app::settings::NetworkSettingsDto;
use arclain_widgets::toggle_switch::ToggleSwitch;
use arclain_widgets::Chips;
use eframe::egui;
use std::sync::Arc;

/// Whether this plugin's traffic is routed through the proxy right now.
/// The *effective* answer, not the stored override -- a default-proxied
/// plugin with no stored entry is routed, and nothing is while the
/// global proxy is off. See `NetworkSettingsDto::plugin_proxy_effective`.
fn plugin_proxy_toggle_value(network: &NetworkSettingsDto, plugin_id: &str) -> bool {
    network.plugin_proxy_effective(plugin_id)
}

/// The raw (sparse, override-only) per-plugin proxy map with `plugin_id`'s
/// entry set to `enabled` -- the shape `NetworkSettingsPatch::
/// plugin_proxy_enabled` persists (a full `Set` replaces the whole map, so
/// this must carry forward every *other* plugin's existing override, not
/// just this one). Pure: builds the patch's payload; persisting it and
/// applying live routing is the facade's `update_settings`'s job (see
/// `render`'s Proxy Settings toggle handler).
fn plugin_proxy_override_map(
    network: &NetworkSettingsDto,
    plugin_id: &str,
    enabled: bool,
) -> std::collections::BTreeMap<String, bool> {
    let mut settings = network.plugin_proxy_enabled.clone();
    settings.insert(plugin_id.to_string(), enabled);
    settings
}

/// Render the plugin detail view
/// Returns true if the plugin list needs to be refreshed
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut PluginsListState,
    shared: Option<&SharedState>,
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

    let whitelist_entries = fetch_whitelist_entries(shared, &selected_id);

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
        let network_settings = shared.map(|shared| shared.signals().network_settings.get());
        let proxy_enabled = network_settings
            .as_ref()
            .map(|network| plugin_proxy_toggle_value(network, &plugin_info.id))
            .unwrap_or(false);
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

                    let Some(network) = network_settings.as_ref() else {
                        return;
                    };
                    let plugin_proxy_enabled =
                        plugin_proxy_override_map(network, &plugin_info.id, proxy_toggle_val);
                    let mut app = shared.app_state.lock();
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

/// Every domain the selected plugin has requested, as the facade reports
/// them. Read fresh each frame rather than cached: a plugin can request a
/// new domain at any time from a background fetch, so a cache would leave
/// that request invisible until something else happened to invalidate it.
///
/// A failed read shows an empty domain list and says why in the log,
/// rather than taking the whole detail view down with it. The only
/// reachable failure is the application shutting down underneath a
/// still-rendering frame, so the log line cannot repeat for long.
fn fetch_whitelist_entries(
    shared: Option<&SharedState>,
    plugin_id: &str,
) -> Vec<arclain_app::plugins::DomainWhitelistEntryDto> {
    let Some(shared) = shared else {
        return Vec::new();
    };
    let Some(facade) = shared.facade.as_ref() else {
        return Vec::new();
    };
    match shared
        .services
        .tokio_runtime
        .block_on(facade.plugin_domain_whitelist(plugin_id.to_string()))
    {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(
                "Failed to read the domain whitelist for plugin '{plugin_id}': {}",
                error.summary
            );
            Vec::new()
        }
    }
}

/// The security warnings to show under a whitelisted `domain`, already
/// rendered to display text.
///
/// A whitelist entry stores a bare domain, so the analysis is run against
/// the `https://` URL the plugin would actually request. A domain that
/// cannot be analyzed at all shows no warnings rather than an error: the
/// row's job is to flag domains that look dangerous, and "unparseable"
/// is not a security finding to put in front of the user.
fn domain_security_warnings(domain: &str) -> Vec<String> {
    let Ok(info) = arclain_app::analyze_url(&format!("https://{domain}")) else {
        return Vec::new();
    };
    info.warnings
        .iter()
        .map(|warning| warning.description())
        .collect()
}

fn render_domain_row(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entry: &arclain_app::plugins::DomainWhitelistEntryDto,
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
            let warnings = domain_security_warnings(domain);
            if !warnings.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for warning in warnings {
                        ui.label(
                            egui::RichText::new(format!("⚠ {warning}"))
                                .small()
                                .color(theme.colors.error),
                        );
                    }
                });
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

    /// A network snapshot with the proxy on and `overrides` stored.
    fn network_with(overrides: &[(&str, bool)]) -> NetworkSettingsDto {
        NetworkSettingsDto {
            socks5_enabled: true,
            plugin_proxy_enabled: overrides
                .iter()
                .map(|(id, enabled)| ((*id).to_string(), *enabled))
                .collect(),
            ..NetworkSettingsDto::default()
        }
    }

    #[test]
    fn plugin_proxy_toggle_uses_inherited_dlsite_defaults() {
        let network = network_with(&[]);

        assert!(plugin_proxy_toggle_value(&network, "dlsite"));
        assert!(plugin_proxy_toggle_value(&network, "dlsite-metadata"));
        assert!(plugin_proxy_toggle_value(&network, "dlsite-api"));
        assert!(plugin_proxy_toggle_value(&network, "dlsite-html"));
        assert!(!plugin_proxy_toggle_value(&network, "custom"));
    }

    #[test]
    fn plugin_proxy_toggle_preserves_explicit_overrides() {
        let network = network_with(&[("dlsite-api", false), ("custom", true)]);

        assert!(!plugin_proxy_toggle_value(&network, "dlsite-api"));
        assert!(plugin_proxy_toggle_value(&network, "custom"));
    }

    /// With the global proxy off nothing is routed, whatever the stored
    /// overrides say -- so the toggle must read off too.
    #[test]
    fn plugin_proxy_toggle_is_off_while_the_global_proxy_is_disabled() {
        let network = NetworkSettingsDto {
            socks5_enabled: false,
            ..network_with(&[("custom", true)])
        };

        assert!(!plugin_proxy_toggle_value(&network, "custom"));
        assert!(!plugin_proxy_toggle_value(&network, "dlsite"));
    }

    #[test]
    fn plugin_proxy_override_map_preserves_other_explicit_overrides() {
        let network = network_with(&[("dlsite-api", false)]);

        let map = plugin_proxy_override_map(&network, "custom", true);

        assert_eq!(map.get("custom"), Some(&true));
        assert_eq!(map.get("dlsite-api"), Some(&false));
        // Never-overridden plugins are not baked in here -- the raw stored
        // map carries only explicit overrides; `dlsite`/`dlsite-html`'s
        // inherited defaults resolve at read time, not at write time.
        assert_eq!(map.get("dlsite"), None);
        assert_eq!(map.get("dlsite-html"), None);
    }

    #[test]
    fn plugin_proxy_override_map_updates_an_existing_override_in_place() {
        let network = network_with(&[("existing", true)]);

        let map = plugin_proxy_override_map(&network, "existing", false);

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

    /// The domain rows are analyzed through the application facade. The
    /// text the user reads is the facade's mirror of the analysis' own
    /// wording, and must stay identical to it -- this pins the exact
    /// sentence for the case the plugin detail view exists to warn about.
    #[test]
    fn domain_security_warnings_flag_an_abused_top_level_domain() {
        let warnings = domain_security_warnings("secure-login.google.com.evil.tk");

        assert!(
            warnings
                .iter()
                .any(|warning| warning == "Unusual top-level domain: .tk"),
            "expected the abused-TLD wording, got {warnings:?}",
        );
    }

    #[test]
    fn domain_security_warnings_are_empty_for_an_ordinary_domain() {
        assert!(domain_security_warnings("dlsite.com").is_empty());
    }

    /// A domain that cannot be turned into a URL at all yields no
    /// warnings rather than propagating an error into the row -- the same
    /// thing the pre-facade code's `if let Ok(..)` did.
    #[test]
    fn domain_security_warnings_are_empty_for_an_unanalyzable_domain() {
        assert!(domain_security_warnings("").is_empty());
        assert!(domain_security_warnings("not a domain").is_empty());
    }

    /// The detail view renders in contexts that carry no shared state at
    /// all (and, in test fixtures, no facade); it must show "no domains
    /// requested" there rather than panicking on a missing facade.
    #[test]
    fn whitelist_entries_without_shared_state_are_empty() {
        assert!(fetch_whitelist_entries(None, "any-plugin").is_empty());
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
