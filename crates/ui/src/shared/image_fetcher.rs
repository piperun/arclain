//! Image fetcher — fills a missing cached image from its URL fallback.
//!
//! Originally lived at `features::plugins::presentation::rendering::image::trigger_image_fetch`,
//! but the function isn't plugin-specific: it only depends on services
//! exposed via `SharedState` (the image-asset store and the Tokio
//! runtime). Carousel components in `shared/` need to call it, and
//! `shared/ → features/` is a layering violation. Relocating to
//! `shared/` makes both the carousel and the plugin renderer call into
//! a peer-or-down location instead of crossing the boundary.
//!
//! The HTTP request, the response validation, the per-asset size ceiling
//! and the cache write all live behind `arclain_app`'s image surface, so
//! this frontend holds neither an HTTP client nor a cache handle. What is
//! left here is the frontend's own half: the cross-plugin key guard, the
//! "is anything able to receive this" check, and handing the work to the
//! shared image-asset state machine, which repaints once the decoded
//! pixels are ready. Audit `docs/audits/2026-05-19-dependencies.md` §2
//! (shared/→features/ leak class).

use crate::shared::SharedState;
use eframe::egui;

/// Spawn a background fetch for `url`, filling `key` through the shared
/// image-asset store on success (which then re-runs its decode/upload
/// pipeline for that key).
///
/// The fetch goes through `ImageAssetStore::fetch_into_cache` rather than
/// straight at the application: which namespace a key belongs to is a
/// decision the *read* side already makes (a plugin document's image key
/// is scoped to the owning plugin), and both halves must make it
/// identically. This function therefore does not gate on any cache being
/// wired to this frontend -- the application owns both image namespaces.
/// It asks the store instead ([`ImageAssetStore::can_store`]), and no-ops
/// when nothing could receive the bytes: issuing a request whose result is
/// then discarded, reporting success, and re-triggering every 30 s is
/// worse than not fetching.
///
/// `plugin_id`, when `Some`, is the plugin whose document referenced the
/// image. It routes the request through that plugin's domain-whitelist /
/// rate-limit budget, and is also the host's own statement of which
/// plugin's namespace the bytes may be written into -- see
/// [`ImageAssetStore::fetch_into_cache`]. `None` uses the application's
/// own direct request path.
///
/// Returns whether a fetch was actually dispatched. Callers are
/// fire-and-forget and ignore it; it exists so the two refusal paths
/// below (a key naming another plugin's namespace, and nothing able to
/// store the result) are observable from a test rather than being
/// silent early returns that a regression could delete unnoticed.
///
/// [`ImageAssetStore::can_store`]: crate::shared::image_assets::ImageAssetStore::can_store
/// [`ImageAssetStore::fetch_into_cache`]: crate::shared::image_assets::ImageAssetStore::fetch_into_cache
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

    let image_assets = shared.image_assets.clone();
    shared.services.tokio_runtime.spawn(async move {
        // Awaited, not called: `fetch_into_cache` moves the (blocking)
        // facade round trip onto the blocking pool itself. This task runs
        // *on* the store's own runtime, so blocking inline here panicked
        // with "Cannot start a runtime from within a runtime".
        if let Err(error) = image_assets
            .fetch_into_cache(plugin_id.clone(), key.clone(), url, ctx)
            .await
        {
            // The key's tail is plugin-authored, so only its host-derived
            // owner is logged -- matching the guards above, which were
            // written specifically to keep guest text out of global
            // tracing.
            tracing::warn!(
                owner = crate::shared::image_assets::plugin_scoped_image_key_owner(&key)
                    .unwrap_or("<host>"),
                "failed to fetch an image into the cache: {error}"
            );
        }
    });
    true
}
