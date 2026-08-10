//! The `PluginAction::RequestFetch` routing policy, shared by both paths
//! that can produce one.
//!
//! A plugin asks the host to fetch metadata for a key rather than doing it
//! itself, because the host owns the gameta client, the network policy,
//! and the per-plugin rate limit. Two different callers can receive that
//! request:
//!
//! - the event worker, resolving a `RequestFetch` returned from an
//!   `OnArchiveOpen` handler, and
//! - the application facade, resolving one returned from a user
//!   interaction dispatched against an open plugin UI session.
//!
//! They differ only in *where the resulting metadata is written* and *what
//! context the native fallback runs under, so the policy itself --
//! capability gate, rate-limit permit, gameta-then-native ordering, size
//! cap -- lives here once and takes both as parameters. Duplicating it per
//! caller is how the two paths would silently drift on exactly the
//! decisions that matter (whether an uncapabilitied plugin can reach the
//! network, whether an oversized payload is rejected).

use std::sync::Arc;
use tracing::{info, warn};

/// What a resolved [`resolve_request_fetch`] actually did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestFetchOutcome {
    /// Rejected before any network call: missing capabilities, no
    /// rate-limit permit, or a payload over the plugin's data
    /// materialization limit.
    Denied,
    /// The gameta server answered and the metadata was written.
    ServerHandled,
    /// Handed to the plugin's own HTTP path (no gameta server, or it had
    /// nothing for this key).
    NativeFallback,
}

/// Runs the routing policy for one `RequestFetch` key.
///
/// Every effectful step is injected so the policy is testable without a
/// network, and so the two callers can supply their own metadata sink and
/// native-fallback dispatch:
///
/// - `authorized` -- whether the plugin holds both
///   [`crate::types::REQUEST_FETCH_CAPABILITIES`]. Resolved by the caller
///   because each holds the instance differently.
/// - `admit_host_effect` -- binds the complete follow-up action to the exact
///   still-enabled plugin generation. Its guard remains live through network,
///   metadata publication, or native fallback so lifecycle completion is a
///   real effect barrier.
/// - `acquire_host_service_permit` -- the per-plugin network rate limit.
///   Called once before each of the two possible server round-trips, not
///   once for both: a cached hit and a live fetch are separate
///   host-service uses.
/// - `write_metadata` -- where a successful result lands.
/// - `native_fallback` -- how to hand the key back to the plugin.
pub fn resolve_request_fetch<Admit, Guard, Permit, Get, Fetch, Write, Fallback>(
    plugin_id: &str,
    key: &str,
    authorized: bool,
    gameta_available: bool,
    mut admit_host_effect: Admit,
    mut acquire_host_service_permit: Permit,
    get_from_server: Get,
    fetch_from_server: Fetch,
    write_metadata: Write,
    native_fallback: Fallback,
) -> RequestFetchOutcome
where
    Admit: FnMut() -> Option<Guard>,
    Permit: FnMut() -> std::result::Result<(), String>,
    Get: FnOnce(&str, &str) -> std::result::Result<Option<serde_json::Value>, String>,
    Fetch: FnOnce(&str, &str) -> std::result::Result<Option<serde_json::Value>, String>,
    Write: FnOnce(serde_json::Value),
    Fallback: FnOnce(&str),
{
    if !authorized {
        warn!(
            plugin_id,
            "Denied RequestFetch without Network and ArchiveMetadataWrite capabilities"
        );
        return RequestFetchOutcome::Denied;
    }
    let Some(_effect_guard) = admit_host_effect() else {
        warn!(
            plugin_id,
            "Dropped RequestFetch from a stale or disabled plugin"
        );
        return RequestFetchOutcome::Denied;
    };
    if !gameta_available {
        native_fallback(key);
        return RequestFetchOutcome::NativeFallback;
    }
    if let Err(error) = acquire_host_service_permit() {
        warn!(
            plugin_id,
            %error,
            "Denied RequestFetch by plugin host-service network policy"
        );
        return RequestFetchOutcome::Denied;
    }

    let (source, product_id) = key.split_once(':').unwrap_or(("dlsite", key));
    match get_from_server(source, product_id) {
        Ok(Some(metadata)) => {
            if !crate::types::metadata_value_within_limit(&metadata) {
                return RequestFetchOutcome::Denied;
            }
            write_metadata(metadata);
            info!("Set plugin metadata via the gameta server");
            RequestFetchOutcome::ServerHandled
        }
        Ok(None) => {
            if acquire_host_service_permit().is_err() {
                return RequestFetchOutcome::Denied;
            }
            match fetch_from_server(source, product_id) {
                Ok(Some(metadata)) => {
                    if !crate::types::metadata_value_within_limit(&metadata) {
                        return RequestFetchOutcome::Denied;
                    }
                    write_metadata(metadata);
                    RequestFetchOutcome::ServerHandled
                }
                Ok(None) | Err(_) => {
                    native_fallback(key);
                    RequestFetchOutcome::NativeFallback
                }
            }
        }
        Err(_) => {
            native_fallback(key);
            RequestFetchOutcome::NativeFallback
        }
    }
}

/// Resolves one `RequestFetch` a plugin returned from a *user
/// interaction* (a panel button, a list selection) rather than from an
/// archive-open event.
///
/// `pinned_archive_session_id` is the archive session that was active when
/// the *plugin session* this interaction belongs to was opened, and is
/// where the resolved metadata is written. Resolving the destination at
/// completion instead would land the result on whichever archive happens
/// to be active when the fetch returns -- and a gameta round trip plus a
/// native fallback can take seconds, easily long enough for the user to
/// switch archives. The event-worker path pins its own origin for exactly
/// this reason (it carries the originating session in its event context),
/// and the pre-facade UI route captured the origin tab up front too; this
/// keeps all three consistent.
///
/// `None` means the plugin session had no archive open when it opened (a
/// `MainPage` session in the plugin settings page, say). Those fall back
/// to whichever session `ActiveTabBridge::active_archive_session_id`
/// reports at completion, which is the best available answer for a session
/// that never had an origin of its own -- and then to the bridge's
/// no-session sink if there is none.
///
/// Takes the instance rather than the [`PluginManager`] deliberately: the
/// gameta round trip below can take seconds, and a `&PluginManager`
/// receiver would keep the caller's manager lock held for all of it,
/// stalling every unrelated plugin operation. Resolve the instance under a
/// brief manager lock, release it, then call this.
///
/// Blocking: performs the HTTP round trip on the calling thread. Callers
/// on an async runtime must run it on a blocking pool.
pub fn resolve_interactive_request_fetch(
    executor: &crate::InProcessWirtExecutor,
    instance_arc: &Arc<parking_lot::Mutex<crate::PluginInstance>>,
    plugin_id: &str,
    key: &str,
    pinned_archive_session_id: Option<u64>,
) -> RequestFetchOutcome {
    let plugin_id_value = wirt::PluginId::parse(plugin_id.to_string()).ok();
    let (authorized, gameta_available, active_tab) = {
        let instance = instance_arc.lock();
        (
            instance.has_capabilities(&crate::types::REQUEST_FETCH_CAPABILITIES),
            instance.get_gameta_client().is_some(),
            instance.get_active_tab_bridge(),
        )
    };

    resolve_request_fetch(
        plugin_id,
        key,
        authorized,
        gameta_available,
        || {
            plugin_id_value.as_ref().and_then(|plugin_id| {
                executor.admit_host_effect_for_instance(plugin_id, instance_arc)
            })
        },
        || {
            instance_arc
                .lock()
                .try_acquire_network_host_service("gameta")
        },
        |source, product_id| {
            gameta_lookup(instance_arc, |client, limit| {
                client.get_metadata_with_limit(source, product_id, limit)
            })
        },
        |source, product_id| {
            gameta_lookup(instance_arc, |client, limit| {
                client
                    .fetch_metadata_with_limit(source, product_id, false, limit)
                    .map(|response| response.metadata)
            })
        },
        |metadata| {
            write_pinned_session_metadata(active_tab.as_ref(), pinned_archive_session_id, metadata)
        },
        |key| {
            // The plugin's own handler clears its in-progress flag at the
            // end of its native fetch, so no completion notification is
            // sent here -- exactly as the event-worker path does.
            let event = format!("do_native_fetch:{key}");
            let result = plugin_id_value
                .as_ref()
                .ok_or_else(|| crate::types::PluginError::NotFound(plugin_id.to_string()))
                .and_then(|plugin_id| {
                    executor.execute_for_instance(
                        plugin_id,
                        instance_arc,
                        wirt::ExecutorRequest::UiEvent {
                            id: event,
                            value: None,
                        },
                    )
                })
                .and_then(wirt::ExecutorResponse::into_actions);
            if let Err(error) = result {
                warn!(plugin_id, ?error, "Plugin native fetch dispatch failed");
            }
        },
    )
}

/// Runs one gameta lookup against the instance's own client and data
/// materialization limit, re-reading both under the lock each time (the
/// limit is a live setting) and releasing it before the HTTP round trip's
/// result is converted.
fn gameta_lookup<F, T>(
    instance_arc: &Arc<parking_lot::Mutex<crate::PluginInstance>>,
    lookup: F,
) -> std::result::Result<Option<serde_json::Value>, String>
where
    T: serde::Serialize,
    F: FnOnce(
        &arclain_network::features::gameta_client::GametaClient,
        usize,
    ) -> std::result::Result<Option<T>, String>,
{
    let (client, limit) = {
        let instance = instance_arc.lock();
        (
            instance.get_gameta_client(),
            instance.data_materialization_limit(),
        )
    };
    let client = client.ok_or_else(|| "gameta client unavailable".to_string())?;
    lookup(&client, limit)?
        .map(|value| serde_json::to_value(value))
        .transpose()
        .map_err(|error| error.to_string())
}

/// Writes a resolved payload to the archive session this fetch's plugin
/// session was pinned to at open time, falling back to whichever session
/// is active now (and then to the bridge's no-session sink) only when
/// there was no pinned origin -- see
/// [`resolve_interactive_request_fetch`]'s doc comment for why pinning at
/// open is the correct origin and completion time is not.
fn write_pinned_session_metadata(
    active_tab: Option<&Arc<dyn crate::ActiveTabBridge>>,
    pinned_archive_session_id: Option<u64>,
    metadata: serde_json::Value,
) {
    let Some(bridge) = active_tab else {
        warn!("Dropped a resolved RequestFetch payload: no active-tab bridge is installed");
        return;
    };
    if let Some(session_id) = pinned_archive_session_id {
        bridge.set_session_metadata(session_id, Some(metadata));
        return;
    }
    // The same two-branch split `crate::host_functions::metadata::
    // emit_metadata` makes outside an event context.
    match bridge.active_archive_session_id() {
        Some(session_id) => bridge.set_session_metadata(session_id, Some(metadata)),
        None => bridge.set_active_tab_metadata(Some(metadata)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn never_called_server(
        _: &str,
        _: &str,
    ) -> std::result::Result<Option<serde_json::Value>, String> {
        panic!("the server must not be consulted on this path");
    }

    #[test]
    fn an_unauthorized_plugin_never_reaches_the_network_or_the_fallback() {
        let fell_back = Cell::new(false);
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            false,
            true,
            || -> Option<()> { panic!("unauthorized request must not be admitted") },
            || panic!("no permit must be requested"),
            never_called_server,
            never_called_server,
            |_| panic!("no metadata must be written"),
            |_| fell_back.set(true),
        );
        assert_eq!(outcome, RequestFetchOutcome::Denied);
        assert!(!fell_back.get());
    }

    #[test]
    fn no_gameta_server_goes_straight_to_the_plugins_own_fetch() {
        let fell_back = Cell::new(false);
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            true,
            false,
            || Some(()),
            || panic!("no permit must be requested without a server"),
            never_called_server,
            never_called_server,
            |_| panic!("no metadata must be written"),
            |key| {
                assert_eq!(key, "dlsite:RJ1");
                fell_back.set(true);
            },
        );
        assert_eq!(outcome, RequestFetchOutcome::NativeFallback);
        assert!(fell_back.get());
    }

    #[test]
    fn a_denied_permit_stops_before_any_server_call_or_fallback() {
        let fell_back = Cell::new(false);
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            true,
            true,
            || Some(()),
            || Err("rate limited".to_string()),
            never_called_server,
            never_called_server,
            |_| panic!("no metadata must be written"),
            |_| fell_back.set(true),
        );
        assert_eq!(outcome, RequestFetchOutcome::Denied);
        assert!(!fell_back.get());
    }

    #[test]
    fn a_cached_server_hit_writes_metadata_without_a_live_fetch() {
        let written = Cell::new(false);
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            true,
            true,
            || Some(()),
            || Ok(()),
            |source, product_id| {
                assert_eq!((source, product_id), ("dlsite", "RJ1"));
                Ok(Some(serde_json::json!({"title": "x"})))
            },
            never_called_server,
            |metadata| {
                assert_eq!(metadata, serde_json::json!({"title": "x"}));
                written.set(true);
            },
            |_| panic!("no fallback on a server hit"),
        );
        assert_eq!(outcome, RequestFetchOutcome::ServerHandled);
        assert!(written.get());
    }

    /// A key with no `source:` prefix defaults to dlsite -- the same
    /// default both the pre-facade UI route and the event worker used.
    #[test]
    fn a_bare_key_defaults_to_the_dlsite_source() {
        let outcome = resolve_request_fetch(
            "demo",
            "RJ1",
            true,
            true,
            || Some(()),
            || Ok(()),
            |source, product_id| {
                assert_eq!((source, product_id), ("dlsite", "RJ1"));
                Ok(Some(serde_json::json!({})))
            },
            never_called_server,
            |_| {},
            |_| panic!("no fallback on a server hit"),
        );
        assert_eq!(outcome, RequestFetchOutcome::ServerHandled);
    }

    #[test]
    fn a_server_miss_falls_through_to_a_live_fetch_then_to_the_plugin() {
        let permits = Cell::new(0);
        let fell_back = Cell::new(false);
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            true,
            true,
            || Some(()),
            || {
                permits.set(permits.get() + 1);
                Ok(())
            },
            |_, _| Ok(None),
            |_, _| Ok(None),
            |_| panic!("no metadata must be written"),
            |_| fell_back.set(true),
        );
        assert_eq!(outcome, RequestFetchOutcome::NativeFallback);
        assert!(fell_back.get());
        assert_eq!(
            permits.get(),
            2,
            "a cached lookup and a live fetch are separate host-service uses"
        );
    }

    /// Records where a metadata write landed, so the pinning tests can
    /// assert the destination rather than merely that a write happened.
    #[derive(Default)]
    struct RecordingBridge {
        active_session: Option<u64>,
        session_writes: parking_lot::Mutex<Vec<u64>>,
        active_tab_writes: parking_lot::Mutex<usize>,
    }

    impl crate::ActiveTabBridge for RecordingBridge {
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
            self.active_session
        }
        fn set_session_metadata(&self, session_id: u64, _metadata: Option<serde_json::Value>) {
            self.session_writes.lock().push(session_id);
        }
        fn set_active_tab_metadata(&self, _metadata: Option<serde_json::Value>) {
            *self.active_tab_writes.lock() += 1;
        }
        fn set_archive_path(&self, _path: Option<String>) {}
    }

    /// The core of the pinning rule: the destination is decided by where
    /// the plugin session opened, not by what is active when the fetch
    /// finishes. `active_session` here is deliberately a *different*
    /// session, standing in for the user switching archives mid-flight.
    #[test]
    fn a_pinned_fetch_writes_to_its_origin_session_not_whatever_is_active_now() {
        let recording = Arc::new(RecordingBridge {
            active_session: Some(99),
            ..RecordingBridge::default()
        });
        let bridge: Arc<dyn crate::ActiveTabBridge> = recording.clone();

        write_pinned_session_metadata(Some(&bridge), Some(7), serde_json::json!({"t": 1}));

        assert_eq!(
            *recording.session_writes.lock(),
            vec![7],
            "the write must land on the pinned origin session, not the active one"
        );
        assert_eq!(*recording.active_tab_writes.lock(), 0);
    }

    /// A session with no archive open at open time (a settings-page
    /// `MainPage` session) has no origin to pin, so it falls back to the
    /// active session -- and then to the no-session sink.
    #[test]
    fn an_unpinned_fetch_falls_back_to_the_active_session_then_to_the_no_session_sink() {
        let with_active = Arc::new(RecordingBridge {
            active_session: Some(42),
            ..RecordingBridge::default()
        });
        let bridge: Arc<dyn crate::ActiveTabBridge> = with_active.clone();
        write_pinned_session_metadata(Some(&bridge), None, serde_json::json!({}));
        assert_eq!(*with_active.session_writes.lock(), vec![42]);

        let without_active = Arc::new(RecordingBridge::default());
        let bridge: Arc<dyn crate::ActiveTabBridge> = without_active.clone();
        write_pinned_session_metadata(Some(&bridge), None, serde_json::json!({}));
        assert!(without_active.session_writes.lock().is_empty());
        assert_eq!(*without_active.active_tab_writes.lock(), 1);
    }

    #[test]
    fn an_oversized_payload_is_rejected_rather_than_written() {
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            true,
            true,
            || Some(()),
            || Ok(()),
            |_, _| {
                Ok(Some(serde_json::json!({
                    "blob": "x".repeat(crate::types::MAX_PLUGIN_METADATA_BYTES + 1)
                })))
            },
            never_called_server,
            |_| panic!("an oversized payload must never be written"),
            |_| panic!("an oversized payload must not fall back either"),
        );
        assert_eq!(outcome, RequestFetchOutcome::Denied);
    }
}
