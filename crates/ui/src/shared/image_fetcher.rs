//! Image fetcher — generic HTTP-fetch-into-cache for URL-based images.
//!
//! Originally lived at `features::plugins::presentation::rendering::image::trigger_image_fetch`,
//! but the function isn't plugin-specific: it only depends on services
//! exposed via `SharedState` (`async_http_client`, `content_cache`,
//! `tokio_runtime`). Carousel components in `shared/` need to call it,
//! and `shared/ → features/` is a layering violation. Relocating to
//! `shared/` makes both the carousel and the plugin renderer call into
//! a peer-or-down location instead of crossing the boundary.
//!
//! Successful writes notify the shared image-asset state machine, which
//! performs its cache read and decode on the blocking pool and repaints once
//! the decoded pixels are ready. Audit
//! `docs/audits/2026-05-19-dependencies.md` §2 (shared/→features/
//! leak class).

use crate::shared::SharedState;
use arclain_network::{HttpRequest, RequestStatus};
use eframe::egui;

/// Spawn a background fetch for `url`, storing the response bytes at
/// `key` through the shared image-asset store on success (which then
/// re-runs its decode/upload pipeline for that key).
///
/// Storage goes through `ImageAssetStore::store_fetched` rather than
/// `content_cache` directly: which namespace a key belongs to is a
/// decision the *read* side already makes (a plugin document's image key
/// is scoped to the owning plugin), and both halves must make it
/// identically. This function therefore no longer gates on
/// `services.content_cache` being wired directly -- a plugin image is
/// storable through the facade whether or not this frontend has its own
/// host cache. It asks the store instead
/// ([`ImageAssetStore::can_store`]), and no-ops when nothing could
/// receive the bytes: issuing an HTTP request whose result is then
/// discarded, reporting success, and re-triggering every 30 s is worse
/// than not fetching.
///
/// `plugin_id`, when `Some`, routes the HTTP request through the
/// per-plugin rate-limit / domain-whitelist branch of `AsyncHttpClient`,
/// and is also the host's own statement of which plugin's namespace the
/// bytes may be written into -- see [`ImageAssetStore::store_fetched`].
/// `None` uses the host's default request path.
/// Returns whether a fetch was actually dispatched. Callers are
/// fire-and-forget and ignore it; it exists so the two refusal paths
/// below (a key naming another plugin's namespace, and nothing able to
/// store the result) are observable from a test rather than being
/// silent early returns that a regression could delete unnoticed.
pub fn trigger_image_fetch(
    shared: &SharedState,
    plugin_id: Option<String>,
    url: String,
    key: String,
    ctx: egui::Context,
) -> bool {
    // The write choke point for cross-plugin key forgery, mirroring
    // `ImageAssetStore::request`'s read-side check. Every URL-fallback
    // fetch in this frontend goes through here (flat renderer, document
    // renderer, carousel), each declaring which plugin it is rendering, so
    // a key naming a *different* plugin is refused before any request is
    // issued -- and long before the bytes could be written into that
    // plugin's namespace for it to later render as its own.
    if !crate::shared::image_assets::image_key_is_addressable_by(&key, plugin_id.as_deref()) {
        tracing::warn!(
            plugin_id = plugin_id.as_deref().unwrap_or("<none>"),
            "refused an image fetch for a key naming a different plugin's cache namespace"
        );
        return false;
    }
    if !shared.image_assets.can_store(&key) {
        return false;
    }
    let client = &shared.services.async_http_client;
    {
        let client = client.clone();
        let image_assets = shared.image_assets.clone();
        // Use runtime handle
        let runtime = &shared.services.tokio_runtime;

        runtime.spawn(async move {
            let request = HttpRequest::get(&url);

            let id_res = if let Some(pid) = &plugin_id {
                client.request_for_plugin(pid, request)
            } else {
                Ok(client.request(request))
            };

            if let Ok(id) = id_res {
                // Audit P2: wake on completion via `await_complete`
                // instead of a 100ms-tick poll loop. await_complete
                // returns immediately if the request is already done,
                // otherwise notifies-waits until the HTTP task fires
                // notify_waiters.
                let _ = client.await_complete(&id).await;

                if let Some(status) = client.take_response(&id) {
                    if let RequestStatus::Ready(resp) = status {
                        // Validate response before caching
                        let is_valid = resp.status_code == 200
                            && resp.body.len() > 1000  // Real images are >1KB
                            && resp
                                .content_type
                                .as_ref()
                                .is_some_and(|ct| ct.starts_with("image/"));

                        if is_valid {
                            // Stored through the image store rather than
                            // straight into `content_cache`: a plugin
                            // document's image key is namespaced to the
                            // owning plugin, and the read side decodes
                            // that owner out of the key. Writing here
                            // would put the bytes in the host namespace,
                            // where the read can never find them -- a
                            // permanently broken image plus one orphaned
                            // cache entry per 30 s retry. `store_fetched`
                            // is the one place that decision is made, for
                            // both halves.
                            //
                            // Awaited, not called: `store_fetched` moves
                            // the (blocking) write onto the blocking pool
                            // itself. This task runs *on* the store's own
                            // runtime, so doing the blocking write inline
                            // here panicked with "Cannot start a runtime
                            // from within a runtime".
                            if let Err(e) = image_assets
                                .store_fetched(
                                    plugin_id.clone(),
                                    key.clone(),
                                    resp.body,
                                    Some(url.clone()),
                                    ctx.clone(),
                                )
                                .await
                            {
                                // The key's tail is plugin-authored, so
                                // only its host-derived owner is logged --
                                // matching the guards above, which were
                                // written specifically to keep guest text
                                // out of global tracing.
                                tracing::warn!(
                                    owner = crate::shared::image_assets::host_owned_image_key_owner(
                                        &key
                                    )
                                    .unwrap_or("<host>"),
                                    "Failed to cache a fetched image: {e}"
                                );
                            }
                        } else {
                            tracing::warn!(
                                "Invalid image response for {}: status={}, size={}, content_type={:?}",
                                url,
                                resp.status_code,
                                resp.body.len(),
                                resp.content_type
                            );
                        }
                    } else if let RequestStatus::Failed(e) = status {
                        tracing::warn!("Image fetch failed for {}: {}", url, e);
                    }
                } else {
                    client.cancel(&id);
                }
            } else if let Err(e) = id_res {
                tracing::warn!("Failed to start image request {}: {}", url, e);
            }
        });
    }
    true
}
