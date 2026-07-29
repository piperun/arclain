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
/// - `acquire_host_service_permit` -- the per-plugin network rate limit.
///   Called once before each of the two possible server round-trips, not
///   once for both: a cached hit and a live fetch are separate
///   host-service uses.
/// - `write_metadata` -- where a successful result lands.
/// - `native_fallback` -- how to hand the key back to the plugin.
pub fn resolve_request_fetch<Permit, Get, Fetch, Write, Fallback>(
    plugin_id: &str,
    key: &str,
    authorized: bool,
    gameta_available: bool,
    mut acquire_host_service_permit: Permit,
    get_from_server: Get,
    fetch_from_server: Fetch,
    write_metadata: Write,
    native_fallback: Fallback,
) -> RequestFetchOutcome
where
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
/// Unlike the event-worker path, there is no event context to pin the
/// write to: the interaction came from a document the user is looking at
/// right now, so both the metadata write and the native fallback resolve
/// through the installed `ActiveTabBridge` -- the same sink a plugin's own
/// `emit_metadata` host function uses outside an event context. A frontend
/// that reports its active archive session (see
/// [`crate::ActiveTabBridge::active_archive_session_id`]) therefore lands
/// the result on the right session without this path needing to know
/// anything about tabs.
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
    instance_arc: &Arc<parking_lot::Mutex<crate::PluginInstance>>,
    plugin_id: &str,
    key: &str,
) -> RequestFetchOutcome {
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
        |metadata| write_active_session_metadata(active_tab.as_ref(), metadata),
        |key| {
            // The plugin's own handler clears its in-progress flag at the
            // end of its native fetch, so no completion notification is
            // sent here -- exactly as the event-worker path does.
            let event = format!("do_native_fetch:{key}");
            if let Err(error) = instance_arc.lock().send_ui_event(&event, None) {
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

/// Writes a resolved payload to whichever archive session the frontend
/// currently reports as active, falling back to its no-session sink when
/// none is (the same two-branch split
/// `crate::host_functions::metadata::emit_metadata` makes outside an event
/// context).
fn write_active_session_metadata(
    active_tab: Option<&Arc<dyn crate::ActiveTabBridge>>,
    metadata: serde_json::Value,
) {
    let Some(bridge) = active_tab else {
        warn!("Dropped a resolved RequestFetch payload: no active-tab bridge is installed");
        return;
    };
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

    #[test]
    fn an_oversized_payload_is_rejected_rather_than_written() {
        let outcome = resolve_request_fetch(
            "demo",
            "dlsite:RJ1",
            true,
            true,
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
