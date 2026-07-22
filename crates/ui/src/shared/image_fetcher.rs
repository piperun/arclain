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
use arclain_core::CacheType;
use arclain_network::{HttpRequest, RequestStatus};
use eframe::egui;

/// Spawn a background fetch for `url`, caching the response bytes at
/// `key` in the `content_cache` service on success and notifying the shared
/// image-asset store. No-ops if `SharedState` doesn't have
/// a `content_cache` wired (e.g. early-init / test contexts).
///
/// `plugin_id`, when `Some`, routes the HTTP request through the
/// per-plugin rate-limit / domain-whitelist branch of `AsyncHttpClient`.
/// `None` uses the host's default request path.
pub fn trigger_image_fetch(
    shared: &SharedState,
    plugin_id: Option<String>,
    url: String,
    key: String,
    ctx: egui::Context,
) {
    let client = &shared.services.async_http_client;
    if let Some(cache) = &shared.services.content_cache {
        let client = client.clone();
        let cache = cache.clone();
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
                            // Use blocking put with retry - we're in async task so blocking is fine
                            if let Err(e) =
                                cache.put(&key, &resp.body, CacheType::Screenshot, None, Some(&url))
                            {
                                tracing::warn!("Failed to cache image {}: {}", key, e);
                            } else {
                                image_assets.cache_ready(&key, ctx.clone());
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
}
