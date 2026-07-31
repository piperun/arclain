//! Plugin detail view
//!
//! Renders plugin settings, permissions, and custom UI when a plugin is selected.

use crate::features::plugins::application::{document_is_empty, PluginSlot, SlotView};
use crate::features::plugins::domain::types::PluginsListState;
use crate::features::plugins::presentation::document_dispatch;
use crate::features::plugins::presentation::rendering::{
    render_document, DocumentContext, DocumentExtent,
};

use crate::shared::components::Form;
use crate::shared::image_assets::ImageOwner;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_app::settings::NetworkSettingsDto;
use arclain_widgets::toggle_switch::ToggleSwitch;
use arclain_widgets::Chips;
use eframe::egui;

/// How much vertical room a plugin's `MainPage` document may claim.
///
/// Only a `Split` document cares (see [`DocumentExtent`]), and only
/// because of where this host draws it: the plugin's configuration is the
/// last section of a `Form`, which stacks its sections inside a vertical
/// `ScrollArea`. A `Split` drawn with [`DocumentExtent::Full`] gives its
/// `SidePanel`/`CentralPanel` *all remaining* height of the `Ui` they are
/// shown inside, and a scroll area's content `Ui` is sized to the visible
/// viewport -- so one plugin's two-pane layout would take the whole form
/// over instead of sitting in it as one section among the others.
/// Bounding keeps the real two-pane layout the plugin asked for while
/// leaving the form's own stacking intact; the archive browser's
/// properties panel bounds its documents for the same reason, at a
/// smaller cap because it is a narrow side panel rather than a
/// full-width settings page.
const MAIN_PAGE_SPLIT_MAX_HEIGHT: u32 = 480;

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
                            // drop every cached snapshot and live facade
                            // session so the next frame re-fetches instead
                            // of showing stale data.
                            shared.plugin_ui_jobs.invalidate_plugin_snapshots();
                            shared.plugin_ui_jobs.invalidate_chrome_snapshot();
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
                    let app = shared.app_state.lock();
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
                render_domain_row(ui, theme, entry, shared);
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
                render_plugin_ui(ui, theme, &plugin_info.id, shared);
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
/// it through the bounded UI coordinator. The coordinator keeps the last
/// result for one second: short enough for a background request to appear
/// promptly, while keeping database/plugin policy work off the render
/// thread.
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
    if shared.facade.is_none() {
        return Vec::new();
    }
    match shared.plugin_ui_jobs.domain_whitelist(plugin_id) {
        Some(Ok(entries)) => entries.as_ref().clone(),
        Some(Err(error)) => {
            tracing::warn!(
                "Failed to read the domain whitelist for plugin '{plugin_id}': {}",
                error
            );
            Vec::new()
        }
        None => Vec::new(),
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
) {
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
            let mutation_pending = shared.is_some_and(|shared| {
                shared
                    .plugin_ui_jobs
                    .domain_approval_pending(&entry.plugin_id, domain)
            });
            if ui
                .add_enabled(!mutation_pending, ToggleSwitch::new(&mut approved_state))
                .changed()
            {
                if let Some(shared) = shared {
                    shared.plugin_ui_jobs.request(
                        crate::features::plugins::application::PluginUiRequest::SetDomainApproved {
                            plugin_id: entry.plugin_id.clone(),
                            domain: domain.clone(),
                            approved: approved_state,
                        },
                    );
                }
            }
        });
    });
}

/// Render the selected plugin's own configuration UI -- its `MainPage`
/// extension point, served by the application facade's session contract
/// (see `crate::features::plugins::application::facade_sessions`).
///
/// The slot is window-scoped rather than tab-scoped: this view draws
/// exactly one `MainPage` for the whole window, so a document event
/// resolves against whichever tab is active when it happens -- the same
/// fallback `crate::core::operation_bridge` applies to this slot's
/// asynchronous action results.
///
/// Safe to call every frame, which is what this function exists to make
/// true: the slot holds its session *and* its document, so a frame that
/// finds the slot open reads a cached tree instead of re-entering the
/// WASM guest. That is what retires the per-plugin `cached_main_layout`
/// this view used to hold to keep a per-frame `get-ui-layout` off the
/// render thread.
fn render_plugin_ui(ui: &mut egui::Ui, theme: &AppTheme, plugin_id: &str, shared: &SharedState) {
    let Some(facade) = shared.facade.as_ref() else {
        // Stated rather than silent: without a facade there is no session
        // to open, and drawing nothing under the section header would
        // read as "this plugin has no configuration".
        ui.label(
            egui::RichText::new(
                "Plugin configuration is unavailable: application facade is unavailable",
            )
            .color(theme.colors.on_surface_variant),
        );
        return;
    };

    let slot = PluginSlot::MainPage {
        plugin_id: plugin_id.to_string(),
    };
    match shared
        .plugin_sessions
        .view(facade, shared.services.tokio_runtime.handle(), &slot)
    {
        SlotView::Ready(document) => {
            if document_is_empty(&document.root) {
                ui.label(
                    egui::RichText::new("This plugin does not provide configuration.")
                        .color(theme.colors.on_surface_variant),
                );
                return;
            }
            let image_owner = ImageOwner::plugin_settings(plugin_id);
            let events = render_document(
                ui,
                &document,
                DocumentContext {
                    colors: &theme.colors,
                    shared_state: Some(shared),
                    image_owner: Some(&image_owner),
                    extent: DocumentExtent::Bounded(MAIN_PAGE_SPLIT_MAX_HEIGHT),
                },
            );
            let origin_tab = shared.signals().tabs.get().active_id();
            document_dispatch::apply_document_events(shared, &slot, origin_tab, events);
        }
        SlotView::Opening => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new("Plugin is busy...")
                        .italics()
                        .color(theme.colors.on_surface_variant),
                );
            });
        }
        SlotView::Failed(error) => {
            ui.label(
                egui::RichText::new(format!("Plugin UI error: {error}")).color(theme.colors.error),
            );
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
}
