//! Settings action handlers
//!
//! Extracted from ui.rs to reduce file size and improve organization.

use crate::core::navigation::SettingsPage;
use crate::features::plugins::domain::types::PluginsListState;

use crate::features::settings::domain::types::{
    ArchivesSettingsState, ConnectionTestResult, SecuritySettingsState, ServerSettingsState,
    SettingsAction, TestStepResult,
};

use crate::shared::SharedState;

/// Check if action is navigation and extract target page
pub fn extract_navigation(action: &SettingsAction) -> Option<SettingsPage> {
    match action {
        SettingsAction::NavigateTo(page) => Some(page.clone()),
        _ => None,
    }
}

/// Turns one row of the password-rules dialog's in-memory draft into the
/// application's write shape.
///
/// `password` is always `Some`: the dialog edits a full rule list with
/// the password visible in the form, so every row it submits carries the
/// password that should be in effect afterward. (The facade's "`None`
/// means keep the stored password" convention is for callers editing a
/// rule *without* seeing its secret; this dialog is not one.)
fn password_rule_input(
    rule: crate::features::password_management::dialogs::zip_pass_rules::PasswordRule,
) -> arclain_app::settings::PasswordRuleEditInput {
    arclain_app::settings::PasswordRuleEditInput {
        original_name: rule.original_name,
        name: rule.name,
        pattern: rule.pattern,
        priority: rule.priority,
        enabled: rule.enabled,
        password: (!rule.replacement_password.is_empty())
            .then(|| arclain_app::challenge::SecretInput::new(rule.replacement_password)),
    }
}

/// Splits a `host:port` proxy authority into the pair
/// [`arclain_app::ArclainApp::test_socks5_proxy`] takes.
///
/// `rsplit_once`, not `split_once`: an IPv6 literal authority
/// (`[::1]:1080`) is full of colons and only the last one separates the
/// port. The bracketed host is handed over intact, which is exactly the
/// form the facade reassembles an authority from.
fn split_proxy_authority(address: &str) -> Option<(String, u16)> {
    let (host, port) = address.trim().rsplit_once(':')?;
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let port: u16 = port.trim().parse().ok()?;
    Some((host.to_string(), port))
}

/// The probe's own trace, in the shape the result panel renders.
///
/// Step for step: the panel shows a row per step with its name, verdict,
/// and detail line, then a "Connected via {ip} ({country})" footer on
/// success -- and the clipboard export is built from the same values, so
/// nothing here may summarize or drop a step.
fn probe_result(report: arclain_app::settings::NetworkProbeReport) -> ConnectionTestResult {
    let success = report.succeeded();
    ConnectionTestResult {
        steps: report
            .steps
            .into_iter()
            .map(|step| TestStepResult {
                name: step.name,
                passed: step.passed,
                message: step.message,
            })
            .collect(),
        success,
        result_message: success.then(|| {
            format!(
                "{} ({})",
                report.ip.unwrap_or_default(),
                report.country.unwrap_or_default()
            )
        }),
    }
}

/// A one-step failed result for a probe that never ran -- no facade, or a
/// candidate address the page could not even make an authority out of.
///
/// `name` is the step the probe *would* have started with, so the panel
/// reads the same whether the failure came from here or from the probe
/// itself: `SOCKS5` when testing through a proxy, `HTTP` when testing the
/// direct path.
fn failed_probe(name: &str, message: impl Into<String>) -> ConnectionTestResult {
    ConnectionTestResult {
        steps: vec![TestStepResult {
            name: name.to_string(),
            passed: false,
            message: Some(message.into()),
        }],
        success: false,
        result_message: None,
    }
}

/// Runs one cache-maintenance command through the application boundary.
///
/// The facade owns the live cache root, metadata connection, locking, and
/// error classification. Keeping those details here as typed commands and
/// reports means this frontend never has to reconstruct a cache path or
/// borrow the legacy database mirror.
fn handle_cache_maintenance(
    shared: &SharedState,
    task: arclain_app::settings::CacheMaintenanceTask,
) {
    let Some(facade) = shared.facade.as_ref() else {
        tracing::error!("Cannot maintain cache: application facade is unavailable");
        shared
            .toaster
            .lock()
            .error("Cache maintenance is unavailable right now");
        return;
    };

    match shared
        .services
        .tokio_runtime
        .block_on(facade.maintain_cache(task))
    {
        Ok(arclain_app::settings::CacheMaintenanceReport::IndexCleared) => {
            tracing::info!("Cache index cleared successfully");
        }
        Ok(arclain_app::settings::CacheMaintenanceReport::ContentCleared) => {
            tracing::info!("Cache content cleared successfully");
        }
        Ok(arclain_app::settings::CacheMaintenanceReport::OrphansRemoved { entries }) => {
            tracing::info!(entries, "Garbage collected orphaned cache entries");
        }
        Ok(arclain_app::settings::CacheMaintenanceReport::OldSearchEntriesRemoved { entries }) => {
            tracing::info!(entries, "Cleaned old search cache entries");
        }
        Ok(arclain_app::settings::CacheMaintenanceReport::EntriesRepaired {
            cache_types,
            product_ids,
        }) => {
            tracing::info!(cache_types, product_ids, "Repaired cache entries");
        }
        Err(error) => {
            tracing::error!(
                kind = ?error.kind,
                correlation_id = ?error.correlation_id,
                diagnostic = ?error.diagnostic,
                "Cache maintenance failed: {}",
                error.summary
            );
            shared.toaster.lock().error(error.summary);
        }
    }
}

/// Handle a settings action, mutating the appropriate state.
///
/// `plugins_state` remains optional because most call sites don't carry
/// the PluginsFeature borrow (for example, password-rule dialogs).
pub fn handle_action(
    action: SettingsAction,
    security_state: &mut SecuritySettingsState,
    _archives_state: &mut ArchivesSettingsState,
    plugins_state: Option<&mut PluginsListState>,
    network_state: &mut crate::features::settings::domain::types::NetworkSettingsState,
    server_state: &mut ServerSettingsState,

    shared: &SharedState,
) {
    match action {
        SettingsAction::SaveSecurity {
            key_file_path,
            secrets_db_path,
            encrypted_crc_policy,
        } => {
            let Some(facade) = shared.facade.as_ref() else {
                *security_state.error.write() = "Settings are unavailable right now".to_string();
                return;
            };
            let state = shared.app_state.lock();
            if let Err(e) = state.apply_preferences(
                facade,
                &shared.services.tokio_runtime,
                key_file_path,
                secrets_db_path,
                encrypted_crc_policy,
            ) {
                *security_state.error.write() = format!("Failed to save settings: {}", e);
            } else {
                *security_state.info.write() = "Settings saved successfully".to_string();
            }
        }
        SettingsAction::SaveArchives { temp_dir } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save archive settings: settings facade is unavailable");
                return;
            };
            let state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: Some(arclain_app::settings::ArchiveSettingsPatch {
                        backend_mode: arclain_app::settings::PatchValue::Keep,
                        cache_directory: arclain_app::settings::PatchValue::Keep,
                        temp_directory: match temp_dir {
                            Some(ref value) if !value.is_empty() => {
                                arclain_app::settings::PatchValue::Set(std::path::PathBuf::from(
                                    value,
                                ))
                            }
                            _ => arclain_app::settings::PatchValue::Clear,
                        },
                        transfer_directory: arclain_app::settings::PatchValue::Keep,
                        sevenzip_path: arclain_app::settings::PatchValue::Keep,
                        default_collision_policy: arclain_app::settings::PatchValue::Keep,
                    }),
                    network: None,
                    security: None,
                },
            );
            if let Err(e) = patch_result {
                tracing::error!("Failed to save user config: {}", e);
            }
        }
        SettingsAction::SaveCollisionPolicy { policy } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save the collision policy: settings facade is unavailable");
                return;
            };
            let state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: Some(arclain_app::settings::ArchiveSettingsPatch {
                        backend_mode: arclain_app::settings::PatchValue::Keep,
                        cache_directory: arclain_app::settings::PatchValue::Keep,
                        temp_directory: arclain_app::settings::PatchValue::Keep,
                        transfer_directory: arclain_app::settings::PatchValue::Keep,
                        sevenzip_path: arclain_app::settings::PatchValue::Keep,
                        default_collision_policy: arclain_app::settings::PatchValue::Set(
                            policy.as_settings_str().to_string(),
                        ),
                    }),
                    network: None,
                    security: None,
                },
            );
            if let Err(e) = patch_result {
                tracing::error!("Failed to persist the collision policy: {}", e);
            }
        }
        SettingsAction::SaveKeyboardMouse { bindings } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save hotkey bindings: settings facade is unavailable");
                return;
            };
            let hotkey_bindings_json = serde_json::to_string(&bindings).ok();
            let state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    archive: None,
                    network: None,
                    security: None,
                    general: Some(arclain_app::settings::GeneralSettingsPatch {
                        hotkey_bindings: match hotkey_bindings_json {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                        open_nested_in_new_tab: arclain_app::settings::PatchValue::Keep,
                        drop_behavior: arclain_app::settings::PatchValue::Keep,
                        restore_tabs_on_launch: arclain_app::settings::PatchValue::Keep,
                    }),
                },
            );
            match patch_result {
                Ok(_) => tracing::info!("Hotkey bindings saved successfully"),
                Err(e) => tracing::error!("Failed to save hotkey bindings: {}", e),
            }

            // Signal app to reload hotkeys
            state.signals.hotkeys_updated.set(true);
        }
        SettingsAction::MoveVault { dest_path } => {
            let Some(facade) = shared.facade.as_ref() else {
                *security_state.error.write() = "Settings are unavailable right now".to_string();
                return;
            };
            let state = shared.app_state.lock();
            if let Err(e) = state.move_vault(facade, &shared.services.tokio_runtime, &dest_path) {
                *security_state.error.write() = format!("Failed to move vault: {}", e);
            } else {
                *security_state.info.write() = "Vault moved successfully".to_string();
            }
        }
        SettingsAction::RekeyVault { new_key_file_path } => {
            let Some(facade) = shared.facade.as_ref() else {
                *security_state.error.write() = "Settings are unavailable right now".to_string();
                return;
            };
            let state = shared.app_state.lock();
            if let Err(e) =
                state.rekey_vault(facade, &shared.services.tokio_runtime, &new_key_file_path)
            {
                *security_state.error.write() = format!("Failed to rekey vault: {}", e);
            } else {
                *security_state.info.write() = "Vault rekeyed successfully".to_string();
            }
        }
        SettingsAction::SavePasswordRules { rules } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save password rules: settings facade is unavailable");
                return;
            };
            let inputs = rules.into_iter().map(password_rule_input).collect();
            if let Err(error) = shared
                .services
                .tokio_runtime
                .block_on(facade.replace_password_rules(inputs))
            {
                tracing::error!(
                    diagnostic = ?error.diagnostic,
                    "Failed to save password rules: {}",
                    error.summary
                );
                shared.toaster.lock().error(error.summary);
            }
        }
        SettingsAction::InspectPluginPackage { package_path } => {
            let request_id = shared.plugin_ui_jobs.request(
                crate::features::plugins::application::PluginUiRequest::InspectPackage {
                    package_path: package_path.clone(),
                },
            );
            if let Some(plugins_state) = plugins_state {
                plugins_state.begin_package_inspection(package_path, request_id);
            }
        }
        SettingsAction::ApprovePluginPackage {
            package_path,
            expected_fingerprint,
        } => {
            let Some(plugins_state) = plugins_state else {
                shared
                    .toaster
                    .lock()
                    .error("Plugin approval is no longer available");
                return;
            };
            let is_current = matches!(
                plugins_state.pending_install.as_ref(),
                Some(pending)
                    if pending.package_path == package_path
                        && pending.preview.as_ref().is_some_and(|preview| {
                            preview.fingerprint == expected_fingerprint
                        })
                        && !pending.loading
            );
            if !is_current {
                shared
                    .toaster
                    .lock()
                    .error("Plugin approval no longer matches the inspected package");
                return;
            }
            let request_id = shared.plugin_ui_jobs.request(
                crate::features::plugins::application::PluginUiRequest::InstallPackage {
                    package_path,
                    expected_fingerprint,
                },
            );
            let started = plugins_state.begin_package_install(request_id);
            debug_assert!(
                started,
                "a current package approval must start exactly once"
            );
        }
        SettingsAction::CancelPluginInstall => {
            if let Some(plugins_state) = plugins_state {
                plugins_state.cancel_package_install();
            }
        }
        SettingsAction::ClearCacheIndex => {
            handle_cache_maintenance(
                shared,
                arclain_app::settings::CacheMaintenanceTask::ClearIndex,
            );
        }
        SettingsAction::ClearCacheContent => {
            handle_cache_maintenance(
                shared,
                arclain_app::settings::CacheMaintenanceTask::ClearContent,
            );
        }
        SettingsAction::GarbageCollectCache => {
            handle_cache_maintenance(
                shared,
                arclain_app::settings::CacheMaintenanceTask::GarbageCollect,
            );
        }
        SettingsAction::CleanOldSearchCache => {
            handle_cache_maintenance(
                shared,
                arclain_app::settings::CacheMaintenanceTask::CleanOldSearch,
            );
        }
        SettingsAction::MigrateCacheEntries => {
            handle_cache_maintenance(
                shared,
                arclain_app::settings::CacheMaintenanceTask::RepairEntries,
            );
        }
        SettingsAction::SaveGeneral {
            open_nested_in_new_tab,
            drop_behavior,
            restore_tabs_on_launch,
        } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save general settings: settings facade is unavailable");
                return;
            };
            let state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    archive: None,
                    network: None,
                    security: None,
                    general: Some(arclain_app::settings::GeneralSettingsPatch {
                        hotkey_bindings: arclain_app::settings::PatchValue::Keep,
                        open_nested_in_new_tab: arclain_app::settings::PatchValue::Set(
                            open_nested_in_new_tab,
                        ),
                        drop_behavior: arclain_app::settings::PatchValue::Set(
                            drop_behavior.as_settings_str().to_string(),
                        ),
                        restore_tabs_on_launch: arclain_app::settings::PatchValue::Set(
                            restore_tabs_on_launch,
                        ),
                    }),
                },
            );
            match patch_result {
                Ok(_) => tracing::info!("General settings saved"),
                Err(e) => tracing::error!("Failed to save general settings: {}", e),
            }
        }
        SettingsAction::SaveNetwork {
            socks5_enabled,
            socks5_address,
            socks5_username,
            socks5_password,
        } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("[SaveNetwork] settings facade is unavailable");
                shared
                    .toaster
                    .lock()
                    .error("Network settings were not saved: application facade is unavailable");
                return;
            };

            // Validation (no embedded userinfo in the address, etc.) and
            // atomic persistence -- including preserving whatever SOCKS5
            // password is already stored -- now happen inside
            // `update_settings` itself (see `arclain_app::runtime::
            // settings_ops::validate_proxy_for_storage`); a rejected
            // patch here means nothing was written, matching this
            // handler's pre-facade "validate before any mutation"
            // guarantee.
            let state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: None,
                    network: Some(arclain_app::settings::NetworkSettingsPatch {
                        socks5_enabled: arclain_app::settings::PatchValue::Set(socks5_enabled),
                        socks5_address: match socks5_address.clone() {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                        socks5_username: match socks5_username.clone() {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                        plugin_proxy_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_url: arclain_app::settings::PatchValue::Keep,
                    }),
                    security: None,
                },
            );
            drop(state);

            if let Err(error) = patch_result {
                // `error`'s message is already the short, safe-to-show
                // `ApplicationError::summary` (see `submit_settings_patch`'s
                // own doc comment) -- covers both an invalid address
                // (rejected before any write) and a persistence
                // failure with one consistent, specific message.
                tracing::error!("[SaveNetwork] Failed to save network settings: {error}");
                shared
                    .toaster
                    .lock()
                    .error(format!("Network settings were not saved: {error}"));
                return;
            }

            // The password is a dedicated secret write, deliberately
            // separate from the patch above. The facade only reports
            // whether one is configured, never the credential itself,
            // so blank means "preserve" and only a non-empty candidate
            // is written. Removal has its own explicit action below.
            let password = socks5_password
                .filter(|password| !password.is_empty())
                .map(arclain_app::challenge::SecretInput::new);
            if let Some(password) = password {
                if let Err(error) = shared
                    .services
                    .tokio_runtime
                    .block_on(facade.set_socks5_password(Some(password)))
                {
                    tracing::error!("[SaveNetwork] Failed to save the SOCKS5 password: {error:?}");
                    // Unlike the `patch_result` branch above, the
                    // address/identity patch already committed before this
                    // call ever ran. "Network settings were not saved" would
                    // be false in exactly this branch, so this message says
                    // precisely what did and didn't happen instead of
                    // reusing that wording.
                    shared.toaster.lock().error(
                        "Network address and identity settings were saved, but the password \
                         change failed: the old password remains in effect",
                    );
                    return;
                }
            }
            let _ = shared
                .app_state
                .lock()
                .refresh_settings_signals(facade, &shared.services.tokio_runtime);
            network_state.socks5_password_configured = shared
                .signals()
                .network_settings
                .read()
                .socks5_password_configured;
            network_state.socks5_password.set(String::new());

            // Live routing on the facade-owned HTTP client is already in
            // effect by this point (see `runtime::settings_ops::
            // apply_live_proxy_routing`), and `update_settings`
            // validated and persisted the proxy identity itself -- so
            // recording that the save happened is all this handler has
            // left to do. It used to also rebuild a proxy config from the
            // returned snapshot purely to log a summary of it, which said
            // nothing the facade had not already decided (and logged, via
            // `apply_proxy_to_client`) on the way to returning that very
            // snapshot.
            tracing::info!("Network settings saved");
        }
        SettingsAction::ClearSocks5Password => {
            let Some(facade) = shared.facade.as_ref() else {
                shared
                    .toaster
                    .lock()
                    .error("The saved proxy password could not be cleared: application facade is unavailable");
                return;
            };
            match shared
                .services
                .tokio_runtime
                .block_on(facade.set_socks5_password(None))
            {
                Ok(()) => {
                    let _ = shared
                        .app_state
                        .lock()
                        .refresh_settings_signals(facade, &shared.services.tokio_runtime);
                    network_state.socks5_password.set(String::new());
                    network_state.socks5_password_configured = false;
                    tracing::info!("Saved SOCKS5 password cleared");
                }
                Err(error) => {
                    tracing::error!("Failed to clear the SOCKS5 password: {error:?}");
                    shared
                        .toaster
                        .lock()
                        .error("The saved proxy password could not be cleared");
                }
            }
        }
        SettingsAction::TestNetwork {
            socks5_enabled,
            socks5_address,
            socks5_username,
            socks5_password,
        } => {
            use crate::features::settings::domain::types::ConnectionTestStatus;

            // The toggle picks the *mode*: on, the probe goes through the
            // candidate proxy; off, it goes out directly and reports the
            // egress the machine actually has. Both are useful answers,
            // and comparing them is the point of testing with the toggle
            // off.
            let step_name = if socks5_enabled { "SOCKS5" } else { "HTTP" };

            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("[TestNetwork] settings facade is unavailable");
                network_state
                    .connection_test_status
                    .set(ConnectionTestStatus::Complete(failed_probe(
                        step_name,
                        "application facade is unavailable",
                    )));
                return;
            };

            let proxy = if socks5_enabled {
                let address = socks5_address.unwrap_or_default();
                let Some((host, port)) = split_proxy_authority(&address) else {
                    network_state
                        .connection_test_status
                        .set(ConnectionTestStatus::Complete(failed_probe(
                            step_name,
                            "Enter a proxy address as host:port before testing",
                        )));
                    return;
                };
                Some(arclain_app::settings::Socks5Candidate {
                    host,
                    port,
                    username: socks5_username,
                    password: socks5_password
                        .filter(|password| !password.is_empty())
                        .map(arclain_app::challenge::SecretInput::new),
                })
            } else {
                None
            };

            network_state
                .connection_test_status
                .set(ConnectionTestStatus::Testing);
            let status_signal = network_state.connection_test_status.clone();
            let facade = facade.clone();

            // The probe belongs to the facade, the same way the gameta
            // one below does: it owns the proxy client and the runtime the
            // request runs on, and none of these candidate values is
            // saved anywhere. This handler hands them over and renders
            // the trace that comes back.
            let runtime = shared.services.tokio_runtime.handle().clone();
            runtime.spawn(async move {
                let status = match facade.probe_network(proxy).await {
                    Ok(report) => probe_result(report),
                    // Only an unprobeable candidate or a shutting-down
                    // application lands here -- a probe that ran reports
                    // its own failed step instead. `summary` is already
                    // credential-free.
                    Err(error) => failed_probe(step_name, error.summary),
                };
                status_signal.set(ConnectionTestStatus::Complete(status));
            });
        }
        SettingsAction::SaveServer {
            enabled,
            url,
            api_key,
        } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("[SaveServer] settings facade is unavailable");
                return;
            };
            let state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: None,
                    network: Some(arclain_app::settings::NetworkSettingsPatch {
                        socks5_enabled: arclain_app::settings::PatchValue::Keep,
                        socks5_address: arclain_app::settings::PatchValue::Keep,
                        socks5_username: arclain_app::settings::PatchValue::Keep,
                        plugin_proxy_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_enabled: arclain_app::settings::PatchValue::Set(enabled),
                        gameta_server_url: match url.clone() {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                    }),
                    security: None,
                },
            );
            drop(state);
            match patch_result {
                Ok(_) => tracing::info!(
                    "[SaveServer] Server settings saved: enabled={}, url={:?}",
                    enabled,
                    url
                ),
                Err(error) => {
                    tracing::error!("[SaveServer] Failed to save server settings: {error}")
                }
            }

            // The API key is a dedicated secret write, kept separate
            // from the patch above -- see `ArclainApp::
            // set_gameta_api_key`'s own doc comment on why it has no
            // "clear" affordance.
            if let Some(key) = api_key {
                let result = shared.services.tokio_runtime.block_on(
                    facade.set_gameta_api_key(arclain_app::challenge::SecretInput::new(key)),
                );
                match result {
                    Ok(()) => {
                        let _ = shared
                            .app_state
                            .lock()
                            .refresh_settings_signals(facade, &shared.services.tokio_runtime);
                        server_state.api_key.set(String::new());
                        server_state.api_key_configured = true;
                    }
                    Err(error) => {
                        tracing::error!("[SaveServer] Failed to save API key: {error:?}");
                    }
                }
            }
        }
        SettingsAction::TestServer { url, api_key } => {
            use crate::features::settings::domain::types::ServerConnectionStatus;

            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("[TestServer] settings facade is unavailable");
                server_state
                    .connection_status
                    .set(ServerConnectionStatus::Failed(
                        "application facade is unavailable".to_string(),
                    ));
                return;
            };

            server_state
                .connection_status
                .set(ServerConnectionStatus::Testing);
            let status_signal = server_state.connection_status.clone();
            let facade = facade.clone();
            let api_key = api_key
                .filter(|key| !key.trim().is_empty())
                .map(arclain_app::challenge::SecretInput::new);

            // The probe belongs to the facade: it owns the blocking gameta
            // client and the pool that client runs on, so this handler
            // only hands over the form's candidate values (neither of
            // which it saves) and routes the answer back into the page's
            // status signal -- the same shape `TestNetwork` above uses.
            let runtime = shared.services.tokio_runtime.handle().clone();
            runtime.spawn(async move {
                let status = match facade.test_gameta_connection(url, api_key).await {
                    // The server's own reported version, which is what
                    // makes this message evidence the probe reached a real
                    // gameta server rather than any HTTP endpoint.
                    Ok(info) => ServerConnectionStatus::Connected(format!(
                        "gameta server v{} is reachable",
                        info.version
                    )),
                    // `summary` is the short, already-redaction-safe half
                    // of the error (the API key never reaches it, and any
                    // userinfo in the URL is stripped) -- the diagnostic
                    // half stays out of the UI.
                    Err(error) => ServerConnectionStatus::Failed(error.summary),
                };
                status_signal.set(status);
            });
        }
        SettingsAction::NavigateTo(_) => {
            // Navigation is handled by extract_navigation before this function is called
        }
        SettingsAction::SaveEditedRule => {
            // Handled specially in SettingsFeature::render where rules_page is available
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::services::Services;
    use crate::features::settings::domain::types::{ConnectionTestStatus, ServerConnectionStatus};
    use crate::shared::theme::AppTheme;
    use crate::test_support::{app_state_from_facade, bootstrap_test_facade};
    use arclain_widgets::Toaster;
    use parking_lot::Mutex;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tracing_test::traced_test;

    const PROXY_PLUGIN_ID: &str = "proxy-map-test-plugin";

    /// Every editable field reaches the replacement write shape while the
    /// stored password remains represented only as an optional replacement.
    #[test]
    fn a_password_rule_draft_becomes_the_applications_write_shape() {
        let draft = crate::features::password_management::dialogs::zip_pass_rules::PasswordRule {
            original_name: Some("Original rule".to_string()),
            name: "Maker archives".to_string(),
            pattern: r"^\[Maker\]".to_string(),
            replacement_password: "draft-password-4f21".to_string(),
            password_configured: true,
            priority: 7,
            enabled: false,
        };

        let input = password_rule_input(draft);

        assert_eq!(input.original_name.as_deref(), Some("Original rule"));
        assert_eq!(input.name, "Maker archives");
        assert_eq!(input.pattern, r"^\[Maker\]");
        assert_eq!(input.priority, 7);
        assert!(!input.enabled);
        assert!(
            input.password.is_some(),
            "a replacement typed in this edit session must reach the application"
        );
        assert!(
            !format!("{input:?}").contains("draft-password-4f21"),
            "the password must not be printable off the write shape"
        );
    }

    /// The round trip the settings page actually performs: a draft row
    /// submitted through the facade comes back as a stored rule with the
    /// same non-secret fields and a configured password.
    #[test]
    fn a_submitted_password_rule_draft_round_trips_through_the_facade() {
        let temp = tempfile::tempdir().unwrap();
        let facade = bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create test runtime");

        let input = password_rule_input(
            crate::features::password_management::dialogs::zip_pass_rules::PasswordRule {
                original_name: None,
                name: "Maker archives".to_string(),
                pattern: r"^\[Maker\]".to_string(),
                replacement_password: "draft-password-9c07".to_string(),
                password_configured: false,
                priority: 7,
                enabled: false,
            },
        );
        runtime
            .block_on(facade.replace_password_rules(vec![input]))
            .expect("replace password rules with the drafted rule");

        let stored = runtime
            .block_on(facade.password_rules())
            .expect("read password rules");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Maker archives");
        assert_eq!(stored[0].pattern, r"^\[Maker\]");
        assert_eq!(stored[0].priority, 7);
        assert!(!stored[0].enabled);
        assert!(stored[0].password_configured);
        assert!(
            !format!("{stored:?}").contains("draft-password-9c07"),
            "the stored summary must never carry the password itself"
        );
    }

    /// The archives page commits its collision-policy dropdown on
    /// change, so this covers the whole loop the user drives: the
    /// dropdown's action reaches persistence, and the reactive mirror
    /// the page reads back on its next hydration reports the new value.
    #[test]
    fn saving_the_collision_policy_persists_it_and_refreshes_the_mirror() {
        use crate::features::settings::domain::types::CollisionPolicy;

        let temp = tempfile::tempdir().expect("create test directory");
        let facade = bootstrap_test_facade(&temp);
        let shared = shared_state_from_facade(facade);

        assert_eq!(
            shared
                .signals()
                .archive_settings
                .read()
                .default_collision_policy,
            "smart",
            "a fresh profile starts on the pipeline's own default"
        );

        handle_action(
            SettingsAction::SaveCollisionPolicy {
                policy: CollisionPolicy::Overwrite,
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &shared,
        );

        assert_eq!(
            shared
                .signals()
                .archive_settings
                .read()
                .default_collision_policy,
            "overwrite",
            "the mirror the archives page hydrates from must reflect the save"
        );

        let facade = shared.facade.as_ref().expect("facade");
        let persisted = shared
            .services
            .tokio_runtime
            .block_on(facade.settings())
            .expect("read settings back");
        assert_eq!(persisted.archive.default_collision_policy, "overwrite");

        // And the page turns that token back into the same selection it
        // submitted -- the round trip the dropdown depends on.
        assert_eq!(
            CollisionPolicy::from_settings_str(&persisted.archive.default_collision_policy),
            CollisionPolicy::Overwrite
        );
    }

    /// Builds a `SharedState` around an already-bootstrapped facade, the
    /// same unpacking `AppState::new` performs at startup. The caller
    /// keeps the `TempDir` the facade was bootstrapped against alive for
    /// as long as it uses the result.
    fn shared_state_from_facade(facade: arclain_app::ArclainApp) -> SharedState {
        let services = Arc::new(Services::new(
            tokio::runtime::Runtime::new().expect("create frontend test runtime"),
        ));
        let app_state = app_state_from_facade(&facade);
        let signals = app_state.signals.clone();
        app_state
            .refresh_settings_signals(&facade, &services.tokio_runtime)
            .expect("seed the settings signals for the fixture");

        let plugin_ui_jobs = crate::features::plugins::application::PluginUiJobs::new(
            Some(facade.clone()),
            services.tokio_runtime.handle().clone(),
        );
        let image_assets = crate::shared::image_assets::ImageAssetStore::without_source(
            services.tokio_runtime.handle().clone(),
        );
        SharedState {
            app_state: Arc::new(Mutex::new(app_state)),
            services,
            theme: AppTheme::new(false),
            toaster: Arc::new(Mutex::new(Toaster::new())),
            plugin_ui_jobs,
            plugin_sessions: crate::features::plugins::application::PluginSessions::new(),
            image_assets,
            signals,
            facade: Some(facade),
            operation_origins: crate::core::operation_bridge::OperationOrigins::new(),
            materialization_actions: crate::core::operation_bridge::MaterializationActions::new(),
            external_open_leases: crate::core::operation_bridge::ExternalOpenLeases::new(),
        }
    }

    struct ProxySaveFixture {
        shared: SharedState,
        previous: arclain_app::settings::NetworkSettingsDto,
        _proxy_listener: TcpListener,
        _temp: TempDir,
    }

    impl ProxySaveFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("create proxy settings test directory");
            let facade = bootstrap_test_facade(&temp);

            let proxy_listener =
                TcpListener::bind("127.0.0.1:0").expect("reserve previous proxy address");
            let proxy_address = proxy_listener
                .local_addr()
                .expect("read previous proxy address");
            let previous_password = "previous-proxy-password-4e7a".to_string();

            // Seed the "previous" settings through the facade itself
            // (the same public surface a real prior session would have
            // used), not by reaching into its databases directly.
            let mut plugin_proxy_map = std::collections::BTreeMap::new();
            plugin_proxy_map.insert(PROXY_PLUGIN_ID.to_string(), true);
            let seed_runtime = tokio::runtime::Runtime::new().expect("create seeding runtime");
            seed_runtime.block_on(async {
                let current = facade.settings().await.expect("read first-run settings");
                facade
                    .update_settings(arclain_app::settings::SettingsPatch {
                        expected_revision: current.revision,
                        general: None,
                        archive: None,
                        network: Some(arclain_app::settings::NetworkSettingsPatch {
                            socks5_enabled: arclain_app::settings::PatchValue::Set(true),
                            socks5_address: arclain_app::settings::PatchValue::Set(
                                proxy_address.to_string(),
                            ),
                            socks5_username: arclain_app::settings::PatchValue::Set(
                                "previous-proxy-user-2c8f".to_string(),
                            ),
                            plugin_proxy_enabled: arclain_app::settings::PatchValue::Set(
                                plugin_proxy_map,
                            ),
                            gameta_server_enabled: arclain_app::settings::PatchValue::Keep,
                            gameta_server_url: arclain_app::settings::PatchValue::Keep,
                        }),
                        security: None,
                    })
                    .await
                    .expect("persist previous proxy settings");
                facade
                    .set_socks5_password(Some(arclain_app::challenge::SecretInput::new(
                        previous_password.clone(),
                    )))
                    .await
                    .expect("persist previous proxy password");
            });

            let previous = seed_runtime
                .block_on(facade.settings())
                .expect("read seeded proxy settings")
                .network;
            let shared = shared_state_from_facade(facade);

            Self {
                shared,
                previous,
                _proxy_listener: proxy_listener,
                _temp: temp,
            }
        }

        fn assert_previous_settings_unchanged(&self) {
            let signal = self.shared.signals.network_settings.get();
            assert_network_signal_matches(&signal, &self.previous);
            let facade = self.shared.facade.as_ref().expect("fixture facade");
            let persisted = self
                .shared
                .services
                .tokio_runtime
                .block_on(facade.settings())
                .expect("reload persisted settings")
                .network;
            assert_network_signal_matches(&persisted, &self.previous);
        }
    }

    /// Compares the non-secret network identity plus the facade's
    /// configured/not-configured secret indicator.
    fn assert_network_signal_matches(
        actual: &arclain_app::settings::NetworkSettingsDto,
        expected: &arclain_app::settings::NetworkSettingsDto,
    ) {
        assert_eq!(
            actual.socks5_enabled, expected.socks5_enabled,
            "network signal proxy enablement changed"
        );
        assert_eq!(
            actual.socks5_address, expected.socks5_address,
            "network signal proxy address changed"
        );
        assert_eq!(
            actual.socks5_username, expected.socks5_username,
            "network signal proxy username changed"
        );
        assert_eq!(
            actual.socks5_password_configured, expected.socks5_password_configured,
            "network signal proxy password status changed"
        );
        assert_eq!(
            actual.plugin_proxy_enabled, expected.plugin_proxy_enabled,
            "network signal plugin proxy choices changed"
        );
    }

    fn assert_invalid_proxy_save_is_atomic(
        enabled: bool,
        address_user: &str,
        address_password: &str,
        direct_user: &str,
        direct_password: &str,
    ) {
        let fixture = ProxySaveFixture::new();
        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: enabled,
                socks5_address: Some(format!(
                    "{address_user}:{address_password}@proxy.example:1080"
                )),
                socks5_username: Some(direct_user.to_string()),
                socks5_password: Some(direct_password.to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        fixture.assert_previous_settings_unchanged();

        let observable_state = format!(
            "{:?} {:?}",
            fixture.shared.signals.network_settings.get(),
            fixture.shared.toaster.lock()
        );
        for secret in [address_user, address_password, direct_user, direct_password] {
            assert!(
                !observable_state.contains(secret),
                "proxy secret reached state, persistence, or error surface: {observable_state}"
            );
        }
        assert!(observable_state.contains("<invalid address>"));
    }

    #[traced_test]
    #[test]
    fn save_network_rejects_enabled_address_userinfo_before_any_mutation() {
        let secrets = [
            "enabled-address-user-secret-a14f",
            "enabled-address-password-secret-b25e",
            "enabled-direct-user-secret-c36d",
            "enabled-direct-password-secret-d47c",
        ];

        assert_invalid_proxy_save_is_atomic(true, secrets[0], secrets[1], secrets[2], secrets[3]);

        for secret in secrets {
            assert!(!logs_contain(secret), "proxy secret leaked in tracing");
        }
        assert!(logs_contain("<invalid address>"));
    }

    #[traced_test]
    #[test]
    fn save_network_rejects_disabled_nonempty_address_userinfo_before_any_mutation() {
        let secrets = [
            "disabled-address-user-secret-e58b",
            "disabled-address-password-secret-f69a",
            "disabled-direct-user-secret-a70c",
            "disabled-direct-password-secret-b81d",
        ];

        assert_invalid_proxy_save_is_atomic(false, secrets[0], secrets[1], secrets[2], secrets[3]);

        for secret in secrets {
            assert!(!logs_contain(secret), "proxy secret leaked in tracing");
        }
        assert!(logs_contain("<invalid address>"));
    }

    #[traced_test]
    #[test]
    fn save_network_accepts_blank_disabled_proxy_and_preserves_password() {
        let fixture = ProxySaveFixture::new();

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: false,
                socks5_address: Some(String::new()),
                socks5_username: None,
                socks5_password: None,
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        let signal = fixture.shared.signals.network_settings.get();
        assert!(!signal.socks5_enabled);
        assert_eq!(signal.socks5_address.as_deref(), Some(""));
        assert!(signal.socks5_username.is_none());
        assert!(
            signal.socks5_password_configured,
            "blank password must preserve the configured secret"
        );

        let facade = fixture.shared.facade.as_ref().expect("fixture facade");
        let persisted = fixture
            .shared
            .services
            .tokio_runtime
            .block_on(facade.settings())
            .expect("reload persisted settings")
            .network;
        assert!(!persisted.socks5_enabled);
        assert_eq!(persisted.socks5_address.as_deref(), Some(""));
        assert!(persisted.socks5_username.is_none());
        assert!(persisted.socks5_password_configured);
        assert!(!format!("{:?}", fixture.shared.toaster.lock()).contains("Error"));
        assert!(!logs_contain("Refusing to save invalid proxy"));
        assert!(logs_contain("Network settings saved"));
    }

    #[test]
    fn clear_socks5_password_removes_only_the_explicitly_selected_secret() {
        let fixture = ProxySaveFixture::new();
        let mut network_state =
            crate::features::settings::domain::types::NetworkSettingsState::default();
        network_state.socks5_password_configured = true;

        handle_action(
            SettingsAction::ClearSocks5Password,
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut network_state,
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        assert!(!network_state.socks5_password_configured);
        let facade = fixture.shared.facade.as_ref().expect("fixture facade");
        let persisted = fixture
            .shared
            .services
            .tokio_runtime
            .block_on(facade.settings())
            .expect("reload persisted settings")
            .network;
        assert!(!persisted.socks5_password_configured);
    }

    #[traced_test]
    #[test]
    fn unavailable_secrets_db_does_not_persist_or_apply_proxy() {
        // Reproduces "the vault genuinely never opened" by corrupting
        // `config.sqlite` before bootstrap -- the same technique
        // `arclain_app`'s own `tests/bootstrap.rs::
        // corrupt_configuration_database_is_tolerated` test uses
        // (`open_databases` composes config+secrets+metadata as one
        // atomic unit; a failure in any one part leaves `dbs: None`
        // overall -- see `arclain_app::tests::settings_facade::
        // secret_writing_methods_fail_cleanly_and_leak_nothing_when_the_
        // vault_never_opened` for the equivalent facade-level coverage
        // this mirrors at the UI layer). Unlike `ProxySaveFixture`, this
        // cannot seed any "previous" proxy settings first -- there is no
        // open vault to write them into -- so the meaningful assertion
        // here is "still at first-run defaults, nothing leaked", not
        // "the prior custom config survived".
        const NEW_PASSWORD: &str = "unavailable-db-password-8f32";
        let temp = tempfile::tempdir().expect("create test directory");
        let paths = arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        let databases_dir = paths.data_dir.join("databases");
        std::fs::create_dir_all(&databases_dir).expect("create databases dir");
        std::fs::write(
            databases_dir.join("config.sqlite"),
            b"this is not a sqlite database, just noise to corrupt the file",
        )
        .expect("write corrupt config.sqlite");
        let sevenzip_path = temp
            .path()
            .join(if cfg!(windows) { "7zz.exe" } else { "7zz" });
        std::fs::write(&sevenzip_path, b"frontend fixture only")
            .expect("write dummy 7-Zip fixture");
        let facade = arclain_app::ArclainApp::bootstrap_with_overrides(
            arclain_app::BootstrapConfig {
                paths_override: Some(paths),
                worker_threads: None,
                archive_backend_override: None,
                extract_runner_override: None,
                materialization_lease_ttl_override: None,
                materialization_cleanup_interval_override: None,
            },
            arclain_app::BootstrapOverrides {
                sevenzip_path: Some(sevenzip_path),
            },
        )
        .expect("corrupt config.sqlite must not fail bootstrap");

        let initial = tokio::runtime::Runtime::new()
            .expect("create settings runtime")
            .block_on(facade.settings())
            .expect("read degraded settings snapshot");
        assert!(
            !initial.security.vault_available,
            "test setup must reproduce a genuinely unavailable vault"
        );
        let shared = shared_state_from_facade(facade);

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some("127.0.0.1:1080".to_string()),
                socks5_username: None,
                socks5_password: Some(NEW_PASSWORD.to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &shared,
        );

        // Nothing persisted: still at first-run defaults.
        let network = shared.signals.network_settings.get();
        assert!(
            !network.socks5_enabled,
            "no vault available -- the save must not have applied"
        );

        let diagnostics = format!("{:?}", shared.toaster.lock());
        assert!(diagnostics.contains("Network settings were not saved"));
        assert!(!diagnostics.contains(NEW_PASSWORD));
        assert!(!logs_contain(NEW_PASSWORD));
    }

    // ---------------------------------------------------------------
    // TestServer: the gameta health check, now served by the facade.
    // ---------------------------------------------------------------

    /// A single-request HTTP stub answering `GET /api/v1/health` the way
    /// a live gameta server would, then closing the connection. The probe
    /// itself now runs inside the facade, so this only has to be a real
    /// server on a real port.
    fn serve_one_health_check(listener: TcpListener) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut chunk = [0_u8; 512];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match socket.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&chunk[..read]),
                }
            }
            const BODY: &str = r#"{"status":"ok","version":"9.9.9"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
                BODY.len(),
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
        })
    }

    /// Blocks until the connection test leaves `Testing`. The handler
    /// dispatches the probe onto the runtime and returns immediately --
    /// that non-blocking shape is itself part of what these tests pin
    /// down, so the result has to be waited for rather than read inline.
    fn await_server_status(state: &ServerSettingsState) -> ServerConnectionStatus {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let status = state.connection_status.read().clone();
            if !matches!(status, ServerConnectionStatus::Testing) {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the gameta connection test never produced a result"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn dispatch_test_server(
        fixture: &ProxySaveFixture,
        server_state: &mut ServerSettingsState,
        url: String,
        api_key: Option<String>,
    ) {
        handle_action(
            SettingsAction::TestServer { url, api_key },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            server_state,
            &fixture.shared,
        );
    }

    #[test]
    fn test_server_reports_a_reachable_gameta_server() {
        let fixture = ProxySaveFixture::new();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the health stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let stub = serve_one_health_check(listener);
        let mut server_state = ServerSettingsState::default();

        dispatch_test_server(
            &fixture,
            &mut server_state,
            url,
            Some("probe-key".to_string()),
        );

        match await_server_status(&server_state) {
            ServerConnectionStatus::Connected(message) => {
                // The server's own version, not a generic "reachable":
                // it is what tells the user they reached a gameta server
                // and which one.
                assert_eq!(message, "gameta server v9.9.9 is reachable");
            }
            other => panic!("expected Connected, got {other:?}"),
        }
        stub.join().expect("health stub thread panicked");
    }

    /// The probe is a probe: nothing the user typed into the form may be
    /// persisted by testing it.
    #[test]
    fn test_server_does_not_persist_the_candidate_settings() {
        let fixture = ProxySaveFixture::new();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the health stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let stub = serve_one_health_check(listener);
        let mut server_state = ServerSettingsState::default();
        let facade = fixture
            .shared
            .facade
            .as_ref()
            .expect("fixture must have a real facade")
            .clone();
        let runtime = fixture.shared.services.tokio_runtime.handle().clone();
        let before = runtime
            .block_on(facade.settings())
            .expect("read settings before the probe");

        dispatch_test_server(
            &fixture,
            &mut server_state,
            url.clone(),
            Some("probe-key".to_string()),
        );
        let status = await_server_status(&server_state);
        stub.join().expect("health stub thread panicked");
        assert!(matches!(status, ServerConnectionStatus::Connected(_)));

        let after = runtime
            .block_on(facade.settings())
            .expect("read settings after the probe");
        assert_eq!(before.revision, after.revision);
        assert_eq!(
            after.network.gameta_server_url,
            before.network.gameta_server_url
        );
        assert!(
            !after.network.gameta_api_key_configured,
            "testing a connection stored the candidate API key"
        );
    }

    #[traced_test]
    #[test]
    fn test_server_reports_an_unreachable_server_without_leaking_the_api_key() {
        const API_KEY: &str = "ui-gameta-probe-key-9d2a";
        let fixture = ProxySaveFixture::new();
        // Reserve a port and release it, so the connection is refused
        // rather than hanging until the client's own probe timeout.
        let closed = TcpListener::bind("127.0.0.1:0").expect("reserve a closed port");
        let url = format!("http://{}", closed.local_addr().expect("closed address"));
        drop(closed);
        let mut server_state = ServerSettingsState::default();

        dispatch_test_server(&fixture, &mut server_state, url, Some(API_KEY.to_string()));

        let status = await_server_status(&server_state);
        let ServerConnectionStatus::Failed(message) = &status else {
            panic!("expected Failed, got {status:?}");
        };
        assert!(
            !message.contains(API_KEY),
            "API key reached the UI: {message}"
        );
        assert!(!logs_contain(API_KEY), "API key leaked in tracing");
    }

    /// Without a facade there is nothing to probe with. The page must say
    /// so rather than sit on `Testing` forever waiting for a result that
    /// can never arrive.
    #[test]
    fn test_server_without_a_facade_reports_a_failure_rather_than_hanging() {
        let fixture = ProxySaveFixture::new();
        let mut shared = fixture.shared.clone();
        shared.facade = None;
        let mut server_state = ServerSettingsState::default();

        handle_action(
            SettingsAction::TestServer {
                url: "http://gameta.invalid".to_string(),
                api_key: None,
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut server_state,
            &shared,
        );

        assert!(matches!(
            server_state.connection_status.read().clone(),
            ServerConnectionStatus::Failed(_)
        ));
    }

    // ---------------------------------------------------------------
    // TestNetwork: the SOCKS5 probe, now served by the facade.
    // ---------------------------------------------------------------

    /// Blocks until the proxy test leaves `Testing`, for the same reason
    /// [`await_server_status`] exists.
    fn await_network_status(
        state: &crate::features::settings::domain::types::NetworkSettingsState,
    ) -> ConnectionTestResult {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let ConnectionTestStatus::Complete(result) =
                state.connection_test_status.read().clone()
            {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the proxy connection test never produced a result"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn dispatch_test_network(
        shared: &SharedState,
        network_state: &mut crate::features::settings::domain::types::NetworkSettingsState,
        socks5_enabled: bool,
        socks5_address: Option<String>,
        socks5_password: Option<String>,
    ) {
        handle_action(
            SettingsAction::TestNetwork {
                socks5_enabled,
                socks5_address,
                socks5_username: Some("probe-user".to_string()),
                socks5_password,
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            network_state,
            &mut ServerSettingsState::default(),
            shared,
        );
    }

    #[test]
    fn proxy_authority_splits_host_from_port_including_ipv6_literals() {
        assert_eq!(
            split_proxy_authority("127.0.0.1:1080"),
            Some(("127.0.0.1".to_string(), 1080)),
        );
        assert_eq!(
            split_proxy_authority(" proxy.example:9050 "),
            Some(("proxy.example".to_string(), 9050)),
        );
        // The brackets stay on: they are part of the authority form the
        // facade reassembles, not decoration.
        assert_eq!(
            split_proxy_authority("[::1]:1080"),
            Some(("[::1]".to_string(), 1080)),
        );
        assert_eq!(split_proxy_authority(""), None);
        assert_eq!(split_proxy_authority("proxy.example"), None);
        assert_eq!(split_proxy_authority(":1080"), None);
        assert_eq!(split_proxy_authority("proxy.example:not-a-port"), None);
        assert_eq!(split_proxy_authority("proxy.example:99999"), None);
    }

    /// The panel's whole content, end to end: the real per-step trace the
    /// probe produced, not a synthesized one-line verdict. Plus the guard
    /// that the candidate password reaches neither the rendered result
    /// nor tracing.
    #[traced_test]
    #[test]
    fn test_network_renders_the_probes_own_step_trace_without_leaking_the_password() {
        const PASSWORD: &str = "ui-socks5-probe-password-8a4d";
        let fixture = ProxySaveFixture::new();
        // Reserve a port and release it so the TCP step fails fast
        // instead of waiting out the probe timeout.
        let closed = TcpListener::bind("127.0.0.1:0").expect("reserve a closed port");
        let address = closed.local_addr().expect("read the closed address");
        drop(closed);
        let mut network_state =
            crate::features::settings::domain::types::NetworkSettingsState::default();

        dispatch_test_network(
            &fixture.shared,
            &mut network_state,
            true,
            Some(address.to_string()),
            Some(PASSWORD.to_string()),
        );

        let result = await_network_status(&network_state);
        assert!(!result.success);
        // The trace, not a summary: DNS resolved, TCP did not, and the
        // panel can say which is which.
        assert_eq!(
            result
                .steps
                .iter()
                .map(|step| (step.name.as_str(), step.passed))
                .collect::<Vec<_>>(),
            vec![("DNS", true), ("TCP", false)],
        );
        assert!(
            result.steps[1].message.is_some(),
            "the failed step must carry its detail line",
        );
        assert_eq!(result.result_message, None);

        let rendered = format!("{result:?}");
        assert!(
            !rendered.contains(PASSWORD),
            "the proxy password reached the UI: {rendered}",
        );
        assert!(
            !logs_contain(PASSWORD),
            "the proxy password leaked in tracing"
        );
    }

    /// The toggle is a mode switch, not just an on/off: with the proxy
    /// disabled the page probes the direct path, which never walks a
    /// proxy's DNS or TCP steps.
    #[test]
    fn test_network_with_the_proxy_disabled_probes_the_direct_path() {
        let fixture = ProxySaveFixture::new();
        let mut network_state =
            crate::features::settings::domain::types::NetworkSettingsState::default();

        // A proxy address is present and deliberately ignored: the
        // disabled toggle, not the empty address, is what selects direct
        // mode.
        dispatch_test_network(
            &fixture.shared,
            &mut network_state,
            false,
            Some("127.0.0.1:1080".to_string()),
            None,
        );

        let result = await_network_status(&network_state);
        let names = result
            .steps
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            !names.contains(&"DNS") && !names.contains(&"TCP") && !names.contains(&"SOCKS5"),
            "the direct probe walked proxy steps: {names:?}",
        );
    }

    /// An address the page cannot split into host and port never reaches
    /// the facade at all -- the user gets a specific message instead of a
    /// probe that was always going to fail.
    #[test]
    fn test_network_without_a_usable_address_reports_a_failure_instead_of_probing() {
        let fixture = ProxySaveFixture::new();
        let mut network_state =
            crate::features::settings::domain::types::NetworkSettingsState::default();

        dispatch_test_network(&fixture.shared, &mut network_state, true, None, None);

        let result = await_network_status(&network_state);
        assert!(!result.success);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].name, "SOCKS5");
        assert!(
            result.steps[0]
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("host:port"),
            "{:?}",
            result.steps[0],
        );
    }

    /// Same contract as the gameta twin: no facade means a reported
    /// failure, never an indefinite "Testing...". The step is named for
    /// the mode that was selected, so the panel reads the same as a real
    /// failure would.
    #[test]
    fn test_network_without_a_facade_reports_a_failure_rather_than_hanging() {
        let fixture = ProxySaveFixture::new();
        let mut shared = fixture.shared.clone();
        shared.facade = None;
        let mut network_state =
            crate::features::settings::domain::types::NetworkSettingsState::default();

        dispatch_test_network(
            &shared,
            &mut network_state,
            true,
            Some("127.0.0.1:1080".to_string()),
            None,
        );
        let proxied = await_network_status(&network_state);
        assert!(!proxied.success);
        assert_eq!(proxied.steps[0].name, "SOCKS5");

        let mut direct_state =
            crate::features::settings::domain::types::NetworkSettingsState::default();
        dispatch_test_network(&shared, &mut direct_state, false, None, None);
        let direct = await_network_status(&direct_state);
        assert!(!direct.success);
        assert_eq!(direct.steps[0].name, "HTTP");
    }

    /// The report the facade returns is rendered whole: every step in
    /// order with its detail line, and the egress footer the panel and
    /// its clipboard export both read.
    #[test]
    fn probe_result_renders_every_step_and_the_egress_footer() {
        let rendered = probe_result(arclain_app::settings::NetworkProbeReport {
            steps: vec![
                arclain_app::settings::ProbeStepDto {
                    name: "DNS".to_string(),
                    passed: true,
                    message: Some("Resolved to 203.0.113.7:1080".to_string()),
                },
                arclain_app::settings::ProbeStepDto {
                    name: "SOCKS5".to_string(),
                    passed: true,
                    message: None,
                },
            ],
            ip: Some("198.51.100.9".to_string()),
            country: Some("Nowhere".to_string()),
        });

        assert!(rendered.success);
        assert_eq!(
            rendered
                .steps
                .iter()
                .map(|step| (step.name.as_str(), step.passed, step.message.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("DNS", true, Some("Resolved to 203.0.113.7:1080")),
                ("SOCKS5", true, None),
            ],
        );
        assert_eq!(
            rendered.result_message.as_deref(),
            Some("198.51.100.9 (Nowhere)"),
        );
    }

    /// A failed report has no egress to show, so the footer stays absent
    /// rather than rendering "Connected via  ()".
    #[test]
    fn probe_result_omits_the_egress_footer_when_a_step_failed() {
        let rendered = probe_result(arclain_app::settings::NetworkProbeReport {
            steps: vec![arclain_app::settings::ProbeStepDto {
                name: "TCP".to_string(),
                passed: false,
                message: Some("connection refused".to_string()),
            }],
            ip: None,
            country: None,
        });

        assert!(!rendered.success);
        assert_eq!(rendered.result_message, None);
    }
}
