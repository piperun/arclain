//! Plugin detail view
//!
//! Renders plugin settings, permissions, and custom UI when a plugin is selected.

use crate::features::plugins::domain::types::PluginsListState;

use crate::features::plugins::presentation::rendering as ui;

use crate::shared::components::Form;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::PluginManager;
use arclain_widgets::toggle_switch::ToggleSwitch;
use arclain_widgets::Chips;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Render the plugin detail view
/// Returns true if the plugin list needs to be refreshed
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    state: &mut PluginsListState,
    app_state: &Arc<Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
    content_cache: Option<&Arc<arclain_core::ContentCache>>,
) -> bool {
    let mut needs_refresh = false;

    // Drain any plugin actions that background `send_ui_event` threads
    // pushed since the last render. Without this, the actions sit in
    // `state.pending_plugin_actions` indefinitely (or, pre-fix, were
    // pushed into a per-frame local sink that got dropped on return).
    //
    // Also: scan the queue BEFORE draining for a `RefreshPanel` action
    // targeting `MainPage`. If present, invalidate `cached_main_layout`
    // so the next render fetches a fresh layout (audit P4 invalidation).
    invalidate_main_layout_on_refresh_panel(state);

    if let Some(shared) = shared {
        let mut toaster = shared.toaster.lock();
        let dialog_signal = shared.signals().plugin_dialog_state.clone();
        let mut dialog_state = dialog_signal.get();
        drain_pending_plugin_actions(
            &state.pending_plugin_actions,
            &mut toaster,
            &mut dialog_state,
            Some(&shared.refresh_requests),
            Some(&shared.signals().lightbox_state),
            Some(shared),
        );
        dialog_signal.set(dialog_state);
    } else {
        // No shared state available (unusual); drop the queue to avoid
        // unbounded growth.
        state.pending_plugin_actions.lock().clear();
    }

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

    // Capture config_service for use in closures
    let config_service: Option<Arc<arclain_core::ConfigService>> = if let Some(s) = shared {
        s.services.config_service.clone()
    } else {
        None
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
                    if let Some(mgr) = plugin_manager {
                        let res = if enabled {
                            mgr.enable_plugin(&plugin_info.id)
                        } else {
                            mgr.disable_plugin(&plugin_info.id)
                        };

                        if res.is_ok() {
                            let mut app = app_state.lock();
                            let mut enabled_list = app.user_config.get_enabled_plugins();
                            if enabled {
                                if !enabled_list.contains(&plugin_info.id) {
                                    enabled_list.push(plugin_info.id.clone());
                                }
                            } else {
                                enabled_list.retain(|id| id != &plugin_info.id);
                            }
                            app.user_config.set_enabled_plugins(&enabled_list);

                            if let Some(ref cfg_svc) = config_service {
                                let res: anyhow::Result<()> =
                                    cfg_svc.save_user_config(&app.user_config);
                                if let Err(e) = res {
                                    tracing::error!("Failed to save user config: {}", e);
                                }
                            }
                            needs_refresh = true;
                        }
                    }
                }
            })
            .show(ui, &theme.colors);

        // Proxy Settings
        let proxy_enabled = {
            let app = app_state.lock();
            app.user_config
                .get_plugin_proxy_settings()
                .get(&plugin_info.id)
                .cloned()
                .unwrap_or(false)
        };
        let mut proxy_toggle_val = proxy_enabled;

        // Extract http client from services for use in closure
        let http_client = shared.map(|s| s.services.async_http_client.clone());

        crate::shared::components::settings_form::SettingsRow::new("Network Proxy")
            .description("Route this plugin's traffic through the configured SOCKS5 proxy.")
            .action(|ui| {
                if ui.add(ToggleSwitch::new(&mut proxy_toggle_val)).changed() {
                    let mut app = app_state.lock();
                    app.user_config
                        .set_plugin_proxy_enabled(&plugin_info.id, proxy_toggle_val);

                    if let Some(ref cfg_svc) = config_service {
                        let res: anyhow::Result<()> = cfg_svc.save_user_config(&app.user_config);
                        if let Err(e) = res {
                            tracing::error!("Failed to save user config: {}", e);
                        }
                    }

                    // Update Client from services
                    let map = app.user_config.get_plugin_proxy_settings();
                    if let Some(client) = &http_client {
                        client.update_plugin_proxy_map(map);
                    }

                    needs_refresh = true;
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

        if let Some(manager) = plugin_manager {
            if plugin_info.loaded {
                render_plugin_ui(
                    ui,
                    theme,
                    manager,
                    &plugin_info.id,
                    app_state,
                    shared,
                    content_cache,
                    &state.pending_plugin_actions,
                    &mut state.cached_main_layout,
                );
            } else {
                ui.label(
                    egui::RichText::new("Plugin is not loaded.")
                        .color(theme.colors.on_surface_variant),
                );
            }
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
            ui.label(egui::RichText::new(domain).strong());

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
    manager: &PluginManager,
    plugin_id: &str,
    app_state: &Arc<Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
    content_cache: Option<&Arc<arclain_core::ContentCache>>,
    pending_plugin_actions: &Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>>,
    cached_main_layout: &mut Option<(String, arclain_plugins::types::PluginLayout)>,
) {
    let mgr_arc = if let Some(shared) = shared {
        if let Some(mgr_mutex) = shared.services.plugin_manager.clone() {
            mgr_mutex
        } else {
            return;
        }
    } else {
        // No shared state means no plugin_manager available
        return;
    };

    // Capture config_service for use in closures
    let config_service: Option<Arc<arclain_core::ConfigService>> = if let Some(s) = shared {
        s.services.config_service.clone()
    } else {
        None
    };

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
    } else if let Some(instance_arc) = manager.get_plugin_instance(plugin_id) {
        if let Some(mut instance) = instance_arc.try_lock() {
            let res = instance.get_ui_layout(
                arclain_plugins::types::PluginExtensionPoint::MainPage,
            );
            if let Ok(ref layout) = res {
                *cached_main_layout = Some((plugin_id.to_string(), layout.clone()));
            }
            Some(res)
        } else {
            None // Busy
        }
    } else {
        Some(Err(arclain_plugins::types::PluginError::NotFound(
            plugin_id.to_string(),
        )))
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
                let mgr_clone = mgr_arc.clone();
                let app_state_clone = app_state.clone();
                let config_service_clone: Option<Arc<arclain_core::ConfigService>> =
                    config_service.clone();
                // Use the existing tokio runtime instead of std::thread::spawn
                // (audit P7). Falls back to thread::spawn if no runtime is
                // available (e.g. headless test contexts).
                let tokio_runtime = shared.map(|s| s.services.tokio_runtime.clone());

                // Push actions into the long-lived `pending_plugin_actions`
                // sink so the next render picks them up via
                // `drain_pending_plugin_actions`. The previous design used a
                // per-render `Arc<Mutex<Vec<_>>>` whose only owner was this
                // function — when render returned, the spawned thread's
                // pushes ended up in a dropped buffer.
                let actions_sink = pending_plugin_actions.clone();

                let mut event_callback = Box::new(move |id: &str, value: Option<String>| {
                    let mgr_thread = mgr_clone.clone();
                    let pid_thread = plugin_id_clone.clone();
                    let id_thread = id.to_string();
                    let val_thread = value.clone();
                    let state_thread = app_state_clone.clone();
                    let sink = actions_sink.clone();
                    let cfg_svc_thread: Option<Arc<arclain_core::ConfigService>> =
                        config_service_clone.clone();

                    let work = move || {
                        // Audit P7: only fetch this plugin's settings,
                        // not the whole map. send_ui_event is the slow
                        // step (WASM call); the settings snapshot used
                        // to clone every plugin's HashMap unnecessarily.
                        let (settings_to_save, actions) = {
                            let mgr = mgr_thread.lock();
                            if let Some(instance_arc) = mgr.get_plugin_instance(&pid_thread) {
                                let mut instance = instance_arc.lock();
                                let actions = instance.send_ui_event(&id_thread, val_thread).ok();
                                drop(instance);
                                let snapshot = mgr.get_settings_for(&pid_thread);
                                (snapshot, actions)
                            } else {
                                (None, None)
                            }
                        };

                        if let Some(actions) = actions {
                            let mut s = sink.lock();
                            for a in actions {
                                s.push((pid_thread.clone(), a));
                            }
                        }

                        if let Some(settings_to_save) = settings_to_save {
                            let mut state = state_thread.lock();
                            state
                                .user_config
                                .set_plugin_settings(&pid_thread, settings_to_save);

                            if let Some(ref cfg_svc) = cfg_svc_thread {
                                let res: anyhow::Result<()> =
                                    cfg_svc.save_user_config(&state.user_config);
                                if let Err(e) = res {
                                    tracing::error!("Failed to save plugin settings: {}", e);
                                }
                            }
                        }
                    };

                    // Dispatch on the existing tokio runtime via
                    // spawn_blocking (the work locks parking_lot mutexes
                    // and calls into WASM, so it stays on a blocking
                    // thread — but managed by tokio rather than as a
                    // raw OS thread per click).
                    if let Some(rt) = &tokio_runtime {
                        rt.spawn(async move {
                            let _ = tokio::task::spawn_blocking(work).await;
                        });
                    } else {
                        // No runtime (e.g. headless test); fall back to
                        // a raw thread so the work still happens.
                        std::thread::spawn(work);
                    }
                }) as ui::UiEventCallback;

                let flat_elements = ui_elements.flatten();
                ui::render_ui_elements(
                    ui,
                    &flat_elements,
                    &mut event_callback,
                    &theme.colors,
                    content_cache,
                    shared,
                    Some(plugin_id),
                );
                // Actions pushed by the spawned thread are now visible to
                // `drain_pending_plugin_actions` on the NEXT render of this
                // panel. Toasts, RefreshPanel, etc. propagate within ~1
                // frame instead of being silently dropped.
            }
        }
        Some(Err(e)) => {
            ui.label(
                egui::RichText::new(format!("Error loading UI: {}", e)).color(theme.colors.error),
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

/// Drop `state.cached_main_layout` if `pending_plugin_actions`
/// contains a `RefreshPanel` action targeting the `MainPage`
/// extension point. Called once per render of the detail view, before
/// `drain_pending_plugin_actions` removes the action from the queue.
pub(crate) fn invalidate_main_layout_on_refresh_panel(state: &mut PluginsListState) {
    let needs_invalidation = {
        let pending = state.pending_plugin_actions.lock();
        pending.iter().any(|(_, action)| {
            matches!(
                action,
                arclain_plugins::types::PluginAction::RefreshPanel { extension_point }
                    if extension_point == "MainPage"
            )
        })
    };
    if needs_invalidation {
        state.cached_main_layout = None;
    }
}

/// Drain a queue of plugin actions into `process_plugin_actions`.
///
/// Background `send_ui_event` threads in this view push actions into a
/// long-lived sink on `PluginsListState`. Each render of the detail
/// panel calls this to forward them to toaster / refresh_requests /
/// dialog state. Without it, `ShowToast`, `ShowMessage`, `RefreshPanel`,
/// `EmitMetadata`, etc. emitted by plugin events are silently dropped.
///
/// Takes its dependencies explicitly rather than going through
/// `SharedState` so it remains unit-testable without bootstrapping the
/// full app state.
pub(crate) fn drain_pending_plugin_actions(
    pending: &Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>>,
    toaster: &mut arclain_widgets::Toaster,
    dialog_state: &mut crate::features::plugins::domain::state::PluginDialogState,
    refresh_requests: Option<&Arc<Mutex<Vec<String>>>>,
    lightbox_signal: Option<&arclain_signals::Signal<crate::shared::dialogs::LightboxState>>,
    shared_state: Option<&SharedState>,
) {
    let drained: Vec<(String, arclain_plugins::types::PluginAction)> =
        std::mem::take(&mut *pending.lock());
    for (plugin_id, action) in drained {
        crate::features::plugins::presentation::controllers::plugin_controller::process_plugin_actions(
            vec![action],
            &plugin_id,
            dialog_state,
            toaster,
            refresh_requests,
            lightbox_signal,
            shared_state,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_plugins::types::PluginAction;
    use arclain_widgets::Toaster;
    use crate::features::plugins::domain::state::PluginDialogState;

    /// Regression test for C7 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// Pre-fix, plugin actions returned by `send_ui_event` in the
    /// detail view were pushed into a function-local
    /// `Arc<Mutex<Vec<PluginAction>>>` whose only owner was the render
    /// function itself. When render returned the local was dropped and
    /// the actions were lost — the inline comment at the bottom of the
    /// match arm even said "we can't process them synchronously here".
    ///
    /// Post-fix, actions land in `state.pending_plugin_actions` (a
    /// long-lived sink on `PluginsListState`) and the next render
    /// calls `drain_pending_plugin_actions`. This test pre-loads a
    /// `RefreshPanel` action, runs the drain, and asserts the queue
    /// is empty AND the observable side effect (push into the
    /// `refresh_requests` Vec) happened.
    #[test]
    fn c7_drain_processes_pending_actions() {
        let pending: Arc<Mutex<Vec<(String, PluginAction)>>> = Arc::new(Mutex::new(vec![(
            "test_plugin".to_string(),
            PluginAction::RefreshPanel {
                extension_point: "info_panel".to_string(),
            },
        )]));
        let mut toaster = Toaster::new();
        let mut dialog_state = PluginDialogState::default();
        let refresh_requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        drain_pending_plugin_actions(
            &pending,
            &mut toaster,
            &mut dialog_state,
            Some(&refresh_requests),
            None,
            None,
        );

        assert!(
            pending.lock().is_empty(),
            "C7 fix regressed: pending queue should be empty after drain",
        );
        assert_eq!(
            refresh_requests.lock().len(),
            1,
            "C7 fix regressed: RefreshPanel should have pushed an entry into refresh_requests",
        );
        assert_eq!(
            refresh_requests.lock()[0],
            "info_panel",
            "C7 fix regressed: pushed entry should match the action's extension_point",
        );
    }

    /// Drain on an empty queue should be a no-op.
    #[test]
    fn c7_drain_on_empty_queue_is_noop() {
        let pending: Arc<Mutex<Vec<(String, PluginAction)>>> = Arc::new(Mutex::new(Vec::new()));
        let mut toaster = Toaster::new();
        let mut dialog_state = PluginDialogState::default();

        drain_pending_plugin_actions(
            &pending,
            &mut toaster,
            &mut dialog_state,
            None,
            None,
            None,
        );

        assert!(pending.lock().is_empty());
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
            arclain_plugins::types::PluginLayout::default(),
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
            arclain_plugins::types::PluginLayout::default(),
        ));

        invalidate_main_layout_on_plugin_change(&mut state);

        assert!(
            state.cached_main_layout.is_some(),
            "Cache for plugin_a should not be dropped when plugin_a is still selected",
        );
    }

    /// `RefreshPanel { extension_point: "MainPage" }` in the pending
    /// queue must invalidate `cached_main_layout` even if the same
    /// plugin is still selected (e.g. the plugin's settings changed).
    #[test]
    fn p4_invalidate_main_layout_on_refresh_panel_for_main_page() {
        let mut state = PluginsListState::default();
        state.selected_plugin = Some("plugin_a".to_string());
        state.cached_main_layout = Some((
            "plugin_a".to_string(),
            arclain_plugins::types::PluginLayout::default(),
        ));
        state.pending_plugin_actions.lock().push((
            "plugin_a".to_string(),
            arclain_plugins::types::PluginAction::RefreshPanel {
                extension_point: "MainPage".to_string(),
            },
        ));

        invalidate_main_layout_on_refresh_panel(&mut state);

        assert!(
            state.cached_main_layout.is_none(),
            "RefreshPanel for MainPage should drop the cache",
        );
    }

    /// `RefreshPanel` for a different extension point must NOT
    /// invalidate `cached_main_layout`.
    #[test]
    fn p4_keep_main_layout_when_refresh_targets_other_extension() {
        let mut state = PluginsListState::default();
        state.selected_plugin = Some("plugin_a".to_string());
        state.cached_main_layout = Some((
            "plugin_a".to_string(),
            arclain_plugins::types::PluginLayout::default(),
        ));
        state.pending_plugin_actions.lock().push((
            "plugin_a".to_string(),
            arclain_plugins::types::PluginAction::RefreshPanel {
                extension_point: "Settings".to_string(),
            },
        ));

        invalidate_main_layout_on_refresh_panel(&mut state);

        assert!(
            state.cached_main_layout.is_some(),
            "RefreshPanel for Settings should not drop the MainPage cache",
        );
    }
}
