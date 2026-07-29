//! Event dispatching for plugin manager

use super::request_fetch::RequestFetchOutcome;
use super::types::ManagedPlugin;
use super::{PluginEventScheduler, PluginManager};
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginEvent, PluginIdentityKey, PluginResponse, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Snapshot the `(plugin_id, instance_arc)` pairs for currently-enabled
/// plugins. Briefly acquires both the `plugins` and `enabled_plugins`
/// read locks, then drops them before returning so callers can iterate
/// without holding the maps. Used by every dispatch path
/// (`dispatch_event_async`, `event_worker`, `dispatch_event`) — keeps
/// the lock-acquire-drop semantics consistent (audit finding D4).
fn enabled_plugin_snapshot(
    plugins: &Arc<RwLock<HashMap<PluginIdentityKey, ManagedPlugin>>>,
    enabled_plugins: &Arc<RwLock<HashMap<PluginIdentityKey, bool>>>,
) -> Vec<(String, Arc<Mutex<PluginInstance>>)> {
    let enabled = enabled_plugins.read();
    let map = plugins.read();
    map.iter()
        .filter(|(identity_key, _)| enabled.get(*identity_key).copied().unwrap_or(false))
        .map(|(_, plugin)| (plugin.metadata.id.clone(), plugin.instance.clone()))
        .collect()
}

fn trace_event_received(event: &PluginEvent) {
    match event {
        PluginEvent::OnArchiveOpen { kind, entries, .. } => {
            debug!(
                archive_kind = ?kind,
                entry_count = entries.len(),
                "Event worker processing archive-opened event"
            );
        }
    }
}

fn trace_event_dispatch_failure(plugin_id: &str, _error: &PluginError) {
    error!(plugin_id, "Plugin event dispatch failed");
}

fn trace_native_fetch_dispatch_failure(_error: &PluginError) {
    error!("Plugin native fetch dispatch failed");
}

fn with_event_context<T>(
    instance: &mut PluginInstance,
    event_ctx: crate::host_functions::EventContext,
    operation: impl FnOnce(&mut PluginInstance) -> T,
) -> T {
    instance.set_event_context(Some(event_ctx));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(instance)));
    instance.set_event_context(None);
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn dispatch_archive_opened_event<Dispatch>(
    instance: &Arc<Mutex<PluginInstance>>,
    plugin_id: &str,
    event_ctx: &crate::host_functions::EventContext,
    dispatch: Dispatch,
) -> Option<Result<Vec<crate::types::PluginAction>>>
where
    Dispatch: FnOnce(
        &mut PluginInstance,
        &str,
        Option<String>,
    ) -> Result<Vec<crate::types::PluginAction>>,
{
    let mut instance = instance.lock();
    if !instance.has_capabilities(&[crate::types::PluginCapability::ArchiveMetadataRead]) {
        tracing::warn!(
            plugin_id,
            "Skipped archive-opened event without ArchiveMetadataRead capability"
        );
        return None;
    }

    let result = with_event_context(&mut instance, event_ctx.clone(), |instance| {
        dispatch(
            instance,
            "event:archive_opened",
            Some(event_ctx.archive_path.clone()),
        )
    });
    Some(result)
}

/// Writes metadata for the session an event-worker RequestFetch is
/// resolving on behalf of, via the bridge (if one is installed and still
/// knows about that session). Replaces a direct `Signal::set` now that
/// the event only carries an opaque `archive_session_id` -- see
/// `PluginEvent::OnArchiveOpen`'s doc comment.
fn set_event_session_metadata(
    active_tab: Option<&Arc<dyn crate::ActiveTabBridge>>,
    archive_session_id: u64,
    metadata: serde_json::Value,
) {
    if let Some(bridge) = active_tab {
        bridge.set_session_metadata(archive_session_id, Some(metadata));
    }
}

/// Execute the event-worker RequestFetch route: the shared policy in
/// [`super::request_fetch::resolve_request_fetch`], with this path's own
/// metadata sink (the *originating* event's session, not whichever is
/// active now -- the user may have switched tabs during the HTTP round
/// trip) and its own capability resolution. The injected operations keep
/// the exact production route testable without making a real network
/// request.
#[allow(clippy::too_many_arguments)]
fn process_event_worker_request_fetch<Permit, Get, Fetch, Fallback>(
    instance: &Arc<Mutex<PluginInstance>>,
    plugin_id: &str,
    key: &str,
    active_tab: Option<&Arc<dyn crate::ActiveTabBridge>>,
    archive_session_id: u64,
    gameta_available: bool,
    acquire_host_service_permit: Permit,
    get_from_server: Get,
    fetch_from_server: Fetch,
    native_fallback: Fallback,
) -> RequestFetchOutcome
where
    Permit: FnMut() -> std::result::Result<(), String>,
    Get: FnOnce(&str, &str) -> std::result::Result<Option<serde_json::Value>, String>,
    Fetch: FnOnce(&str, &str) -> std::result::Result<Option<serde_json::Value>, String>,
    Fallback: FnOnce(&str),
{
    let authorized = instance
        .lock()
        .has_capabilities(&crate::types::REQUEST_FETCH_CAPABILITIES);
    super::request_fetch::resolve_request_fetch(
        plugin_id,
        key,
        authorized,
        gameta_available,
        acquire_host_service_permit,
        get_from_server,
        fetch_from_server,
        |metadata| set_event_session_metadata(active_tab, archive_session_id, metadata),
        native_fallback,
    )
}

impl PluginManager {
    /// Background worker that processes events from the channel.
    /// Runs on a dedicated thread and never blocks the caller.
    pub(crate) fn event_worker(
        receiver: std::sync::mpsc::Receiver<PluginEvent>,
        plugins: Arc<RwLock<HashMap<PluginIdentityKey, ManagedPlugin>>>,
        enabled_plugins: Arc<RwLock<HashMap<PluginIdentityKey, bool>>>,
    ) {
        info!("Plugin event worker started");

        while let Ok(event) = receiver.recv() {
            trace_event_received(&event);

            // Build the per-event context once per event — the same
            // context goes into every enabled plugin's instance for
            // the duration of THIS event's handler. Pre-payload-
            // change this used to live in a held `current_archive`
            // Mutex on the host functions; now the event carries
            // the originating tab's snapshots directly.
            let event_ctx = {
                let PluginEvent::OnArchiveOpen {
                    path,
                    password,
                    entries,
                    archive_session_id,
                    ..
                } = &event;
                crate::host_functions::EventContext {
                    archive_path: path.clone(),
                    password: password.clone(),
                    entries: entries.clone(),
                    archive_session_id: *archive_session_id,
                }
            };

            for (plugin_id, instance_arc) in enabled_plugin_snapshot(&plugins, &enabled_plugins) {
                // Phase 1: under lock — install the event context on
                // this instance, dispatch the event, then clear the
                // context. While the context is set, every
                // host-function call the plugin makes inside its
                // handler resolves to the *originating tab*, not to
                // whatever tab the user is currently looking at.
                // This is what makes queued events (5 archives drag-
                // dropped at once) each land their metadata on the
                // right tab instead of all stomping the active tab.
                let Some(result) = dispatch_archive_opened_event(
                    &instance_arc,
                    &plugin_id,
                    &event_ctx,
                    |instance, id, value| instance.send_ui_event(id, value),
                ) else {
                    continue;
                };
                let actions = match result {
                    Ok(actions) => actions,
                    Err(e) => {
                        trace_event_dispatch_failure(&plugin_id, &e);
                        continue;
                    }
                };

                // Phase 2: process actions WITHOUT holding the instance
                // lock, so concurrent UI renders and other operations on
                // the same plugin can proceed during the gameta HTTP
                // round-trip. We already have the event's metadata
                // signal in `event_ctx` — the snapshot from event-fire
                // time, pinned to the originating tab. If the user
                // switches tabs while the HTTP is in flight, the
                // metadata still lands on the originally-targeted tab
                // because we never went through the bridge for this
                // write.
                for action in crate::action_policy::bound_plugin_actions(actions) {
                    let crate::types::PluginAction::RequestFetch { key } = action else {
                        continue;
                    };
                    info!("[EventWorker] Processing plugin RequestFetch");
                    let (gameta_available, active_tab_bridge) = {
                        let locked = instance_arc.lock();
                        (
                            locked.get_gameta_client().is_some(),
                            locked.get_active_tab_bridge(),
                        )
                    };
                    process_event_worker_request_fetch(
                        &instance_arc,
                        &plugin_id,
                        &key,
                        active_tab_bridge.as_ref(),
                        event_ctx.archive_session_id,
                        gameta_available,
                        || {
                            instance_arc
                                .lock()
                                .try_acquire_network_host_service("gameta")
                        },
                        |source, product_id| {
                            let (gameta_client, materialization_limit) = {
                                let instance = instance_arc.lock();
                                (
                                    instance.get_gameta_client(),
                                    instance.data_materialization_limit(),
                                )
                            };
                            let client = gameta_client
                                .ok_or_else(|| "gameta client unavailable".to_string())?;
                            let metadata = client
                                .get_metadata_with_limit(source, product_id, materialization_limit)
                                .map_err(|error| error.to_string())?;
                            metadata
                                .map(serde_json::to_value)
                                .transpose()
                                .map_err(|error| error.to_string())
                        },
                        |source, product_id| {
                            let (gameta_client, materialization_limit) = {
                                let instance = instance_arc.lock();
                                (
                                    instance.get_gameta_client(),
                                    instance.data_materialization_limit(),
                                )
                            };
                            let client = gameta_client
                                .ok_or_else(|| "gameta client unavailable".to_string())?;
                            let metadata = client
                                .fetch_metadata_with_limit(
                                    source,
                                    product_id,
                                    false,
                                    materialization_limit,
                                )
                                .map_err(|error| error.to_string())?
                                .metadata;
                            metadata
                                .map(serde_json::to_value)
                                .transpose()
                                .map_err(|error| error.to_string())
                        },
                        |key| {
                            // Re-install the originating event context while
                            // the plugin's native HTTP fallback runs.
                            let event_name = format!("do_native_fetch:{}", key);
                            info!("[EventWorker] Dispatching native fetch");
                            let mut instance = instance_arc.lock();
                            let result =
                                with_event_context(&mut instance, event_ctx.clone(), |instance| {
                                    instance.send_ui_event(&event_name, None)
                                });
                            if let Err(e) = result {
                                trace_native_fetch_dispatch_failure(&e);
                            }
                        },
                    );
                }
            }
        }

        info!("Plugin event worker stopped");
    }

    /// Canonical event-dispatch API. `Full` from
    /// [`PluginEventScheduler::try_schedule`] means retain/coalesce the event
    /// for a later frame instead of blocking the UI.
    ///
    /// The synchronous `dispatch_event` / `dispatch_event_to_plugin`
    /// below are test fixtures — production should always go through
    /// the channel.
    pub fn event_scheduler(&self) -> PluginEventScheduler {
        self.event_scheduler.clone()
    }

    /// Synchronously dispatch an event to every enabled plugin and
    /// collect their responses. **Test fixture only** — production
    /// should use [`PluginManager::event_scheduler`] so events go
    /// through the background worker. Kept `pub` because the
    /// integration tests in `crates/plugins/tests` need it.
    pub fn dispatch_event(&mut self, event: &PluginEvent) -> Vec<PluginResponse> {
        // The historical implementation called `PluginInstance::on_event`
        // which always returned `Ok(PluginResponse::None)`. That helper
        // was deleted in the 2026-05-19 audit. The synchronous test-only
        // path now mirrors the production worker's behavior: real events
        // flow through `event_scheduler` -> event_worker; sync callers
        // observe "zero responses from the enabled plugin set" which is
        // what the integration tests were always asserting on anyway.
        let _ = event;
        debug!("Dispatching plugin event through test fixture");
        Vec::new()
    }

    /// Synchronously dispatch an event to a specific plugin. **Test
    /// fixture only** — same caveat as [`PluginManager::dispatch_event`].
    pub fn dispatch_event_to_plugin(
        &mut self,
        plugin_id: &str,
        event: &PluginEvent,
    ) -> Result<PluginResponse> {
        let _ = event;
        debug!("Dispatching plugin event to plugin '{}'", plugin_id);

        if !self.is_plugin_enabled(plugin_id) {
            return Err(PluginError::ExecutionError(format!(
                "Plugin '{}' is disabled",
                plugin_id
            )));
        }

        // Verify the plugin is loaded (preserves the prior error contract)
        // and return `None` — see `dispatch_event` above for why.
        {
            let identity_key = PluginIdentityKey::parse(plugin_id)
                .map_err(|_| PluginError::NotFound(plugin_id.to_string()))?;
            let plugins = self.plugins.read();
            plugins
                .get(&identity_key)
                .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;
        }
        Ok(PluginResponse::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Minimal `ActiveTabBridge` test double for
    /// `process_event_worker_request_fetch`'s own tests: only
    /// `metadata()`/`set_session_metadata` matter here (the tests assert
    /// on what was -- or was not -- written), so every other method is a
    /// harmless stub. A plain `Mutex` cell, not `arclain_signals::Signal`
    /// -- this crate's `ActiveTabBridge` trait no longer exposes a signal
    /// type in its public API, and its test doubles follow suit.
    #[derive(Default)]
    struct TestBridge {
        metadata: Mutex<Option<serde_json::Value>>,
    }

    impl TestBridge {
        fn metadata(&self) -> Option<serde_json::Value> {
            self.metadata.lock().clone()
        }
    }

    impl crate::ActiveTabBridge for TestBridge {
        fn archive_path(&self) -> Option<String> {
            None
        }
        fn current_password(&self) -> Option<String> {
            None
        }
        fn archive_entries(&self) -> Vec<String> {
            Vec::new()
        }
        fn active_archive_session_id(&self) -> Option<u64> {
            None
        }
        fn set_session_metadata(
            &self,
            _archive_session_id: u64,
            metadata: Option<serde_json::Value>,
        ) {
            *self.metadata.lock() = metadata;
        }
        fn set_active_tab_metadata(&self, metadata: Option<serde_json::Value>) {
            *self.metadata.lock() = metadata;
        }
        fn set_archive_path(&self, _path: Option<String>) {}
    }

    fn manager_with_capabilities(
        network: bool,
        archive_metadata_write: bool,
        archive_metadata_read: bool,
    ) -> (tempfile::TempDir, PluginManager) {
        let plugin_id = "ui-demo";
        let root = tempfile::tempdir().expect("create plugin fixture directory");
        let plugins_dir = root.path().join("plugins");
        let plugin_dir = plugins_dir.join(plugin_id);
        std::fs::create_dir_all(&plugin_dir).expect("create plugin manifest directory");

        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins")
            .join(plugin_id);
        std::fs::copy(
            fixture_dir.join(format!("{plugin_id}.wasm")),
            plugin_dir.join(format!("{plugin_id}.wasm")),
        )
        .expect("copy plugin fixture");
        let manifest = std::fs::read_to_string(fixture_dir.join(format!("{plugin_id}.toml")))
            .expect("read plugin manifest")
            .replace("network = false", &format!("network = {network}"))
            .replace(
                "archive_metadata_write = false",
                &format!("archive_metadata_write = {archive_metadata_write}"),
            )
            .replace(
                "archive_metadata_read = false",
                &format!("archive_metadata_read = {archive_metadata_read}"),
            );
        std::fs::write(plugin_dir.join(format!("{plugin_id}.toml")), manifest)
            .expect("write tailored plugin manifest");

        let mut manager = PluginManager::new_with_plugin_log_dir(
            plugins_dir,
            HashMap::new(),
            root.path().join("logs"),
        )
        .expect("create plugin manager");
        manager.init().expect("load plugin fixture");
        (root, manager)
    }

    #[tracing_test::traced_test]
    #[test]
    fn event_trace_redacts_archive_path_and_password() {
        let path_marker = "archive-path-must-not-reach-global-tracing";
        let password_marker = "archive-password-must-not-reach-global-tracing";
        let event = PluginEvent::OnArchiveOpen {
            path: format!("C:/private/{path_marker}.zip"),
            kind: arclain_core::ArchiveKind::Zip,
            password: Some(password_marker.to_string()),
            entries: Arc::new(Vec::new()),
            archive_session_id: 0,
        };

        trace_event_received(&event);

        assert!(!logs_contain(path_marker));
        assert!(!logs_contain(password_marker));
    }

    #[tracing_test::traced_test]
    #[test]
    fn dispatch_failure_traces_redact_guest_context() {
        let archive_marker = "archive-context-must-not-reach-dispatch-errors";
        let key_marker = "guest-key-must-not-reach-native-dispatch-errors";

        trace_event_dispatch_failure(
            "ui-demo",
            &PluginError::ExecutionError(archive_marker.to_string()),
        );
        trace_native_fetch_dispatch_failure(&PluginError::ExecutionError(key_marker.to_string()));

        assert!(!logs_contain(archive_marker));
        assert!(!logs_contain(key_marker));
    }

    #[test]
    fn event_worker_denies_request_fetch_without_both_capabilities() {
        for (network, metadata_write) in [(true, false), (false, true)] {
            let (_root, manager) = manager_with_capabilities(network, metadata_write, false);
            let instance = manager
                .get_plugin_instance("ui-demo")
                .expect("loaded plugin instance");
            let concrete_bridge = Arc::new(TestBridge::default());
            let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
            let fetch_called = AtomicBool::new(false);
            let fallback_called = AtomicBool::new(false);

            let outcome = process_event_worker_request_fetch(
                &instance,
                "ui-demo",
                "dlsite:RJ000001",
                Some(&bridge),
                1,
                true,
                || Ok(()),
                |_, _| {
                    fetch_called.store(true, Ordering::SeqCst);
                    Ok(Some(serde_json::json!({"product_id": "RJ000001"})))
                },
                |_, _| Ok(None),
                |_| fallback_called.store(true, Ordering::SeqCst),
            );

            assert_eq!(outcome, RequestFetchOutcome::Denied);
            assert!(!fetch_called.load(Ordering::SeqCst));
            assert!(!fallback_called.load(Ordering::SeqCst));
            assert!(concrete_bridge.metadata().is_none());
        }
    }

    #[test]
    fn event_worker_allows_request_fetch_with_both_capabilities() {
        let (_root, manager) = manager_with_capabilities(true, true, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let concrete_bridge = Arc::new(TestBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
        let fetch_called = AtomicBool::new(false);
        let fallback_called = AtomicBool::new(false);

        let outcome = process_event_worker_request_fetch(
            &instance,
            "ui-demo",
            "dlsite:RJ000001",
            Some(&bridge),
            1,
            true,
            || Ok(()),
            |source, product_id| {
                assert_eq!(source, "dlsite");
                assert_eq!(product_id, "RJ000001");
                fetch_called.store(true, Ordering::SeqCst);
                Ok(Some(serde_json::json!({"product_id": product_id})))
            },
            |_, _| Ok(None),
            |_| fallback_called.store(true, Ordering::SeqCst),
        );

        assert_eq!(outcome, RequestFetchOutcome::ServerHandled);
        assert!(fetch_called.load(Ordering::SeqCst));
        assert!(!fallback_called.load(Ordering::SeqCst));
        assert_eq!(
            concrete_bridge
                .metadata()
                .and_then(|value| value["product_id"].as_str().map(str::to_owned))
                .as_deref(),
            Some("RJ000001")
        );
    }

    #[test]
    fn event_worker_denies_request_fetch_when_host_service_budget_is_exhausted() {
        let (_root, manager) = manager_with_capabilities(true, true, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = arclain_network::AsyncHttpClient::new(
            runtime.handle().clone(),
            Arc::new(parking_lot::RwLock::new(
                arclain_network::DomainWhitelist::default(),
            )),
            None,
        );
        client.configure_plugin(
            "ui-demo",
            arclain_network::PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 1,
            },
        );
        client
            .try_acquire_plugin_host_service("ui-demo", "gameta")
            .expect("first host-service request is within policy");
        let concrete_bridge = Arc::new(TestBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
        let fetch_called = AtomicBool::new(false);
        let fallback_called = AtomicBool::new(false);

        let outcome = process_event_worker_request_fetch(
            &instance,
            "ui-demo",
            "dlsite:RJ000001",
            Some(&bridge),
            1,
            true,
            || {
                client
                    .try_acquire_plugin_host_service("ui-demo", "gameta")
                    .map_err(|error| error.to_string())
            },
            |_, _| {
                fetch_called.store(true, Ordering::SeqCst);
                Ok(Some(serde_json::json!({"product_id": "RJ000001"})))
            },
            |_, _| Ok(None),
            |_| fallback_called.store(true, Ordering::SeqCst),
        );

        assert_eq!(outcome, RequestFetchOutcome::Denied);
        assert!(!fetch_called.load(Ordering::SeqCst));
        assert!(!fallback_called.load(Ordering::SeqCst));
        assert!(concrete_bridge.metadata().is_none());
    }

    #[test]
    fn event_worker_requires_a_second_permit_before_server_fetch_fallback() {
        let (_root, manager) = manager_with_capabilities(true, true, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = arclain_network::AsyncHttpClient::new(
            runtime.handle().clone(),
            Arc::new(parking_lot::RwLock::new(
                arclain_network::DomainWhitelist::default(),
            )),
            None,
        );
        client.configure_plugin(
            "ui-demo",
            arclain_network::PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 1,
            },
        );
        let concrete_bridge = Arc::new(TestBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
        let get_called = AtomicBool::new(false);
        let fetch_called = AtomicBool::new(false);
        let native_called = AtomicBool::new(false);

        let outcome = process_event_worker_request_fetch(
            &instance,
            "ui-demo",
            "dlsite:RJ000001",
            Some(&bridge),
            1,
            true,
            || {
                client
                    .try_acquire_plugin_host_service("ui-demo", "gameta")
                    .map_err(|error| error.to_string())
            },
            |_, _| {
                get_called.store(true, Ordering::SeqCst);
                Ok(None)
            },
            |_, _| {
                fetch_called.store(true, Ordering::SeqCst);
                Ok(None)
            },
            |_| native_called.store(true, Ordering::SeqCst),
        );

        assert_eq!(outcome, RequestFetchOutcome::Denied);
        assert!(get_called.load(Ordering::SeqCst));
        assert!(!fetch_called.load(Ordering::SeqCst));
        assert!(!native_called.load(Ordering::SeqCst));
        assert!(concrete_bridge.metadata().is_none());
    }

    #[test]
    fn event_worker_rejects_oversized_server_metadata_before_signal_publication() {
        let (_root, manager) = manager_with_capabilities(true, true, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let concrete_bridge = Arc::new(TestBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
        let native_called = AtomicBool::new(false);

        let outcome = process_event_worker_request_fetch(
            &instance,
            "ui-demo",
            "dlsite:RJ000001",
            Some(&bridge),
            1,
            true,
            || Ok(()),
            |_, _| {
                Ok(Some(serde_json::json!({
                    "product_id": "RJ000001",
                    "description": "x".repeat(crate::types::MAX_PLUGIN_METADATA_BYTES),
                })))
            },
            |_, _| Ok(None),
            |_| native_called.store(true, Ordering::SeqCst),
        );

        assert_eq!(outcome, RequestFetchOutcome::Denied);
        assert!(concrete_bridge.metadata().is_none());
        assert!(!native_called.load(Ordering::SeqCst));
    }

    #[test]
    fn event_worker_without_gameta_client_uses_no_permits_and_one_native_fallback() {
        let (_root, manager) = manager_with_capabilities(true, true, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let concrete_bridge = Arc::new(TestBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
        let permits = AtomicUsize::new(0);
        let get_calls = AtomicUsize::new(0);
        let fetch_calls = AtomicUsize::new(0);
        let fallback_calls = AtomicUsize::new(0);

        let outcome = process_event_worker_request_fetch(
            &instance,
            "ui-demo",
            "dlsite:RJ000001",
            Some(&bridge),
            1,
            false,
            || {
                permits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_, _| {
                get_calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            },
            |_, _| {
                fetch_calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            },
            |_| {
                fallback_calls.fetch_add(1, Ordering::SeqCst);
            },
        );

        assert_eq!(outcome, RequestFetchOutcome::NativeFallback);
        assert_eq!(permits.load(Ordering::SeqCst), 0);
        assert_eq!(get_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn event_worker_get_error_uses_one_permit_then_falls_back_without_server_fetch() {
        let (_root, manager) = manager_with_capabilities(true, true, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let concrete_bridge = Arc::new(TestBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = concrete_bridge.clone();
        let permits = AtomicUsize::new(0);
        let fetch_calls = AtomicUsize::new(0);
        let fallback_calls = AtomicUsize::new(0);

        let outcome = process_event_worker_request_fetch(
            &instance,
            "ui-demo",
            "dlsite:RJ000001",
            Some(&bridge),
            1,
            true,
            || {
                permits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_, _| Err("server unavailable".to_string()),
            |_, _| {
                fetch_calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            },
            |_| {
                fallback_calls.fetch_add(1, Ordering::SeqCst);
            },
        );

        assert_eq!(outcome, RequestFetchOutcome::NativeFallback);
        assert_eq!(permits.load(Ordering::SeqCst), 1);
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn event_worker_hides_archive_opened_event_without_read_capability() {
        let (_root, manager) = manager_with_capabilities(false, false, false);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let invoked = AtomicBool::new(false);
        let observed_path = parking_lot::Mutex::new(None);
        let event_ctx = crate::host_functions::EventContext {
            archive_path: "C:/private/library/secret.zip".to_string(),
            password: Some("private-password".to_string()),
            entries: Arc::new(Vec::new()),
            archive_session_id: 0,
        };

        let result =
            dispatch_archive_opened_event(&instance, "ui-demo", &event_ctx, |_, id, value| {
                invoked.store(true, Ordering::SeqCst);
                assert_eq!(id, "event:archive_opened");
                *observed_path.lock() = value;
                Ok(Vec::new())
            });

        assert!(result.is_none(), "unauthorized event was not skipped");
        assert!(!invoked.load(Ordering::SeqCst));
        assert!(
            observed_path.lock().is_none(),
            "unauthorized plugin observed the archive path"
        );
    }

    #[test]
    fn event_worker_dispatches_archive_opened_event_with_read_capability() {
        let (_root, manager) = manager_with_capabilities(false, false, true);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let invoked = AtomicBool::new(false);
        let event_ctx = crate::host_functions::EventContext {
            archive_path: "C:/library/allowed.zip".to_string(),
            password: None,
            entries: Arc::new(Vec::new()),
            archive_session_id: 0,
        };

        let result =
            dispatch_archive_opened_event(&instance, "ui-demo", &event_ctx, |_, id, value| {
                invoked.store(true, Ordering::SeqCst);
                assert_eq!(id, "event:archive_opened");
                assert_eq!(value.as_deref(), Some("C:/library/allowed.zip"));
                Ok(Vec::new())
            });

        assert!(matches!(result, Some(Ok(actions)) if actions.is_empty()));
        assert!(invoked.load(Ordering::SeqCst));
    }

    #[test]
    fn archive_event_context_is_cleared_when_guest_dispatch_panics() {
        let (_root, manager) = manager_with_capabilities(false, false, true);
        let instance = manager
            .get_plugin_instance("ui-demo")
            .expect("loaded plugin instance");
        let event_ctx = crate::host_functions::EventContext {
            archive_path: "C:/library/panic.zip".to_string(),
            password: None,
            entries: Arc::new(Vec::new()),
            archive_session_id: 0,
        };

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = dispatch_archive_opened_event(
                &instance,
                "ui-demo",
                &event_ctx,
                |_, _, _| -> Result<Vec<crate::types::PluginAction>> {
                    panic!("simulated guest dispatch panic")
                },
            );
        }));

        assert!(panic.is_err());
        assert!(!instance.lock().has_event_context_for_test());
    }
}
