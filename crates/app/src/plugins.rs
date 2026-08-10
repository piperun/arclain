//! Renderer-neutral plugin sessions: the application-facade wrapper
//! around `arclain_plugins`'s host-side plugin manager and Wirt's
//! renderer-neutral UI model (`wirt::ui_model`).
//!
//! `wirt::ui_model` defines the node/document *shape*
//! (`PluginUiNodeDto`, `PluginUiNodeKind`, ...) with no awareness of this
//! crate's opaque ids -- it cannot depend on `arclain_app` (the
//! dependency runs the other way). This module supplies the one missing
//! piece, [`PluginUiDocument`], which wraps a normalized root node with
//! the session/plugin/revision identity a caller needs, plus
//! [`PluginSessionStore`]: the session registry, per-plugin action
//! serialization, and the `arclain_plugins::ActiveTabBridge` /
//! `arclain_plugins::types::PluginArchiveAccess` adapters that connect
//! plugin host functions to this application's own archive-session
//! store instead of a UI signal tree.
//!
//! It also owns the plugin *domain-access* read model -- the per-plugin
//! network whitelist ([`DomainWhitelistEntryDto`]) and the URL security
//! analysis a frontend renders beside it ([`analyze_url`],
//! [`DomainAnalysisDto`], [`DomainWarningDto`]). Both live here rather
//! than in a module of their own because both answer one question about
//! one plugin -- which domains may this plugin reach, and do any of them
//! look dangerous -- and both mirror `arclain-network` shapes so that no
//! frontend needs that crate to ask it.
//!
//! ## What this task ports faithfully, and what it does not (yet)
//!
//! Moved into this module, and fully tested here:
//! - Session lifecycle (open/query/close), one normalized document per
//!   session with a retained last-known revision for immediate queries.
//! - Per-plugin action serialization: [`PluginSessionStore::dispatch_action`]
//!   holds a per-`plugin_id` `tokio::sync::Mutex` across the entire
//!   dispatch (node visibility/enabled check, the WASM call, and
//!   re-normalization), so two actions submitted against the same plugin
//!   apply in submission order and never interleave.
//! - `PluginAction::RefreshPanel` handling: resolved inline, by
//!   re-fetching and re-normalizing the session's layout as part of the
//!   very same dispatch that produced the refresh request -- the
//!   *coalescing* egui's old `PluginUiJobs` needed a dedicated queue for
//!   (many render frames independently noticing the same stale layout)
//!   is structural here instead: there is no per-frame poll left to
//!   coalesce, so nothing to build a queue for.
//! - Hidden/disabled action rejection, via
//!   `wirt::ui_model::PluginUiNodeDto::find`.
//! - The plugin-*enabled* gate ([`require_enabled_plugin`]): one check,
//!   applied to every surface that runs a plugin or serves what a plugin
//!   authored, so "a disabled plugin does not run" is a property of this
//!   crate rather than something each renderer has to remember. See
//!   [`PluginSessionStore`]'s own doc comment for what it does to a
//!   session that was already open, and [`is_plugin_disabled_refusal`]
//!   for how a frontend tells that refusal apart from "no such plugin".
//!
//! - `PluginAction::RequestFetch` (the gameta-or-native background
//!   metadata fetch) is resolved through
//!   `arclain_plugins::resolve_interactive_request_fetch` -- the same
//!   routing policy the event-triggered path
//!   (`PluginEvent::OnArchiveOpen` -> `RequestFetch`) runs, sharing one
//!   capability gate, one per-plugin network permit, one
//!   gameta-then-native ordering, and one payload size cap. The two paths
//!   differ only in where the result lands: the event path pins it to the
//!   session the event originated from, while an interaction resolves
//!   through whichever archive session the frontend currently reports as
//!   active, since the user is looking at that document right now.
//!
//! Deliberately **not** ported in this pass, and named here rather than
//! silently dropped:
//! - `arclain_plugins::types::PluginArchiveAccess` (the `PluginArchiveContextId`
//!   -keyed trait [`ArchiveContextAccess`] implements) has no installation
//!   point anywhere in `arclain_plugins` today -- no `PluginManager`/
//!   `HostFunctions`/`PluginInstance` setter exists to receive an
//!   implementation of it, and no host function constructs a
//!   `PluginArchiveContextId` or calls `list_entries`/`read_entry`/
//!   `write_metadata` at all (every archive-facing host function
//!   resolves through `ActiveTabBridge` instead). Wiring it for real
//!   would mean adding that plumbing to `arclain_plugins` itself -- a
//!   new WIT-adjacent surface, not a bridge swap -- and is out of scope
//!   here; [`ArchiveContextAccess`] stays implemented and unit-tested
//!   standalone, `#[allow(dead_code)]`, exactly as before.
//! - [`ArchiveContextBridge`]'s `set_session_metadata`/`set_active_tab_metadata`/
//!   `set_archive_path` are no longer stubs -- they write through a real,
//!   settable [`crate::archive::ArchiveSession`] metadata slot and
//!   source-path override, closing the gap `ActiveTabBridge::
//!   set_active_tab_metadata`'s own doc comment calls out ("a no-op is
//!   an acceptable implementation only if this bridge truly has no
//!   notion of the active tab at all"). Every write that actually lands
//!   also publishes [`crate::event::SessionEvent::MetadataChanged`]
//!   through [`crate::ArclainApp::subscribe_session_events`] -- the push
//!   notification a frontend needs to learn that a session's metadata
//!   (or, after a rename, its `source_path`) changed outside any
//!   operation, without polling `archive_snapshot` on a timer.
//!   [`ProductionActiveTabBridge`] composes this bridge with a
//!   frontend-supplied fallback for the one case archive-session state
//!   alone cannot resolve (`set_active_tab_metadata` with no session
//!   active at all), and [`crate::ArclainApp::active_tab_bridge`] is how
//!   a frontend obtains it to install on `PluginManager`.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex as SyncMutex, RwLock as SyncRwLock};
use tokio::sync::Mutex as AsyncMutex;

use arclain_plugins::types::{PluginArchiveAccess, PluginArchiveContextId};
use arclain_plugins::{ActiveTabBridge, PluginError, PluginManager};
use wirt::ui_model::{self, PluginUiNormalizeError};

// Re-exported so a consumer of this facade (a Flutter/Dart bridge, a CLI,
// this crate's own integration tests) never needs `arclain_plugins` as a
// direct dependency just to name the types `PluginUiDocument`/
// `PluginActionRequest`/`PluginUiUpdate` expose in their own fields --
// every type transitively reachable from those three is re-exported here,
// under `arclain_app::plugins`, alongside the types this module defines
// itself. `PluginUiNormalizeError` is deliberately *not* re-exported: a
// normalization failure is always converted into an `ApplicationError`
// (see `normalize_error`) before it leaves this facade, so no consumer
// ever needs to name the lower-level error type directly.
pub use wirt::ui_model::{
    PluginActionDto, PluginButtonActionDto, PluginExtensionPointDto, PluginHostIntentDto,
    PluginImageDto, PluginKeyValueDto, PluginToastLevelDto, PluginToolbarButtonDto,
    PluginUiNodeDto, PluginUiNodeKind, PluginWarningIconDto,
};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::ids::{ArchiveSessionId, PluginSessionId};

/// Maximum bytes [`crate::ArclainApp::read_plugin_image`] will return for
/// one `cache_key`. Chosen independently of `arclain_plugins::types::
/// MAX_PLUGIN_METADATA_BYTES`/`MAX_PLUGIN_GUEST_DATA_BYTES` (4 MiB each):
/// those bound *structured* data crossing the WASM boundary itself, while
/// this bounds a *rendered image asset* a non-egui frontend materializes
/// directly (a decoded texture upload, not a JSON blob) -- large enough
/// for a typical cover/screenshot, small enough that a single asset can
/// never exhaust a frontend's memory on its own.
pub const MAX_PLUGIN_IMAGE_BYTES: u32 = 16 * 1024 * 1024;

/// Prefix marking a [`PluginUiDocument`] node's `cache_key`/`image_key`
/// field as an *encoded* reference this module rewrote at normalization
/// time -- see [`encode_plugin_image_cache_key`].
const PLUGIN_IMAGE_CACHE_KEY_PREFIX: &str = "plugin-image:";

/// Encodes a plugin's own (unscoped) content-cache key together with the
/// plugin id that owns it, so [`crate::ArclainApp::read_plugin_image`]
/// can resolve it later without any other context. Plugin-authored
/// `Image.cache_key`/`Carousel` image/`ListItem.image_key` values are
/// always resolved through the calling plugin's *own* Data API
/// namespace (`arclain_data::CacheOwner::Plugin` -- see the WIT `host`
/// interface's own doc comment on `fetch-to-cache`: "ContentCache
/// entries are namespaced to the calling plugin id"), so a bare cache_key
/// string is ambiguous on its own once it has left the session that
/// produced it. `plugin_id` never contains `:` (`PluginId::parse` only
/// accepts `[A-Za-z0-9_-]`), so [`decode_plugin_image_cache_key`] can
/// always find the boundary with a single `split_once(':')` even though
/// `raw_key` itself commonly does contain `:` (e.g. `"dlsite:image:
/// RJ123456"`).
fn encode_plugin_image_cache_key(plugin_id: &str, raw_key: &str) -> String {
    format!("{PLUGIN_IMAGE_CACHE_KEY_PREFIX}{plugin_id}:{raw_key}")
}

/// Reverses [`encode_plugin_image_cache_key`], returning `(plugin_id,
/// raw_key)`. `None` for any string this module did not itself produce.
fn decode_plugin_image_cache_key(cache_key: &str) -> Option<(&str, &str)> {
    cache_key
        .strip_prefix(PLUGIN_IMAGE_CACHE_KEY_PREFIX)?
        .split_once(':')
}

/// Whether `cache_key` addresses the **plugin-scoped** image namespace --
/// i.e. whether this facade stamped it with an owning plugin at
/// normalization time.
///
/// This predicate is the single boundary between the two image namespaces,
/// and it is total: every string is either a plugin-scoped key (`true`,
/// handled only by `read_plugin_image`/`write_plugin_image`/
/// `fetch_plugin_image`) or a host-owned one (`false`, handled only by
/// `read_host_image`/`fetch_host_image`/`discard_host_image`). Each family
/// refuses the other's keys, so no key is addressable through both -- and
/// a read and a write of the same key can never disagree about which
/// namespace it lives in.
///
/// Exposed so a frontend routes by the *facade's* own predicate instead of
/// re-deriving the prefix from a copy of the literal, which is exactly how
/// the two halves would drift apart again.
pub fn is_plugin_image_key(cache_key: &str) -> bool {
    cache_key.starts_with(PLUGIN_IMAGE_CACHE_KEY_PREFIX)
}

/// The plugin a plugin-scoped image key names as its owner, or `None` for
/// a host-owned key.
///
/// The key encodes its own owner, so this is a *claim*, never an
/// authorization: every write path independently requires the caller to
/// state which plugin it is acting for and refuses a mismatch. Frontends
/// use it to check a key against the surface that is rendering it before
/// the facade is even called.
pub fn plugin_image_key_owner(cache_key: &str) -> Option<&str> {
    decode_plugin_image_cache_key(cache_key).map(|(plugin_id, _)| plugin_id)
}

/// Rewrites every image-bearing node's cache key reference to the
/// encoded form [`encode_plugin_image_cache_key`] produces, recursing
/// into every container kind. Applied once, right after
/// `wirt::ui_model::normalize_layout` succeeds -- see
/// [`PluginSessionStore::fetch_and_normalize`].
fn rewrite_cache_keys(mut node: PluginUiNodeDto, plugin_id: &str) -> PluginUiNodeDto {
    node.kind = match node.kind {
        PluginUiNodeKind::Image {
            cache_key,
            url,
            max_height,
        } => PluginUiNodeKind::Image {
            cache_key: cache_key.map(|key| encode_plugin_image_cache_key(plugin_id, &key)),
            url,
            max_height,
        },
        PluginUiNodeKind::ListItem {
            title,
            subtitle,
            badge,
            image_key,
            image_url,
            selected,
            warning_icon,
        } => PluginUiNodeKind::ListItem {
            title,
            subtitle,
            badge,
            image_key: image_key.map(|key| encode_plugin_image_cache_key(plugin_id, &key)),
            image_url,
            selected,
            warning_icon,
        },
        PluginUiNodeKind::Carousel {
            images,
            current_index,
            max_height,
            thumbnail_height,
            enable_lightbox,
        } => PluginUiNodeKind::Carousel {
            images: images
                .into_iter()
                .map(|image| PluginImageDto {
                    cache_key: encode_plugin_image_cache_key(plugin_id, &image.cache_key),
                    url: image.url,
                })
                .collect(),
            current_index,
            max_height,
            thumbnail_height,
            enable_lightbox,
        },
        PluginUiNodeKind::Single { children } => PluginUiNodeKind::Single {
            children: rewrite_children(children, plugin_id),
        },
        PluginUiNodeKind::Split {
            sidebar,
            content,
            sidebar_width,
        } => PluginUiNodeKind::Split {
            sidebar: rewrite_children(sidebar, plugin_id),
            content: rewrite_children(content, plugin_id),
            sidebar_width,
        },
        PluginUiNodeKind::ListContainer {
            children,
            max_height,
            empty_message,
        } => PluginUiNodeKind::ListContainer {
            children: rewrite_children(children, plugin_id),
            max_height,
            empty_message,
        },
        PluginUiNodeKind::Group {
            title,
            description,
            children,
        } => PluginUiNodeKind::Group {
            title,
            description,
            children: rewrite_children(children, plugin_id),
        },
        other => other,
    };
    node
}

fn rewrite_children(children: Vec<PluginUiNodeDto>, plugin_id: &str) -> Vec<PluginUiNodeDto> {
    children
        .into_iter()
        .map(|child| rewrite_cache_keys(child, plugin_id))
        .collect()
}

/// Resolves a [`PluginUiDocument`] node's encoded `cache_key`/`image_key`
/// value into the bytes `arclain_plugins::host_functions` cached under
/// it, honoring [`MAX_PLUGIN_IMAGE_BYTES`]. Backs
/// `ArclainApp::read_plugin_image`.
///
/// Rejects (`NotFound`) a `cache_key` this module never encoded (an
/// unknown/malformed key), a key naming a plugin/entry the cache does
/// not have, and a cached entry whose size or actual byte count exceeds
/// the cap -- `ContentCache::get_with_limit_for_owner` itself enforces
/// the cap during the read (bailing out before an oversized entry is
/// ever assembled into one `Vec<u8>`), so this never returns a
/// partially-read or truncated buffer for an oversized asset.
pub(crate) fn read_plugin_image(
    content_cache: &arclain_data::ContentCache,
    cache_key: &str,
) -> Result<Vec<u8>, ApplicationError> {
    let (plugin_id, raw_key) = decode_plugin_image_cache_key(cache_key).ok_or_else(|| {
        ApplicationError::new(
            ApplicationErrorKind::NotFound,
            "unknown plugin image cache key",
        )
        .with_recoverability(Recoverability::Fatal)
    })?;
    content_cache
        .get_with_limit_for_owner(
            &arclain_data::CacheOwner::plugin(plugin_id),
            raw_key,
            MAX_PLUGIN_IMAGE_BYTES as usize,
        )
        .map_err(|error| {
            ApplicationError::new(
                ApplicationErrorKind::Internal,
                "failed to read plugin image",
            )
            .with_diagnostic(error.to_string())
            .with_recoverability(Recoverability::Fatal)
        })?
        .ok_or_else(|| {
            ApplicationError::new(ApplicationErrorKind::NotFound, "plugin image not found")
                .with_recoverability(Recoverability::Fatal)
        })
}

/// Caches `bytes` under the plugin namespace `cache_key` names -- the
/// write counterpart of [`read_plugin_image`], and the only supported way
/// for a frontend to populate a plugin document's image reference.
///
/// A renderer that resolves an `Image`/`ListItem`/`Carousel` node's
/// `url` fallback (because the plugin has not cached that asset yet) has
/// fetched bytes that belong in the *plugin's* cache namespace, not the
/// host's: [`read_plugin_image`] decodes the owner out of the key and
/// reads from there, so a host-owner write is structurally unreadable by
/// the very path that asked for it -- an entry that can never be found
/// again and a recovery loop that can never succeed. Routing the write
/// through here keeps both halves on one namespace by construction.
///
/// Enforces [`MAX_PLUGIN_IMAGE_BYTES`] on the way in, so an oversized
/// asset is rejected at write time rather than cached and then
/// permanently rejected on every read. Rejects (`NotFound`) any key this
/// module did not itself encode, exactly as the read does.
pub(crate) fn write_plugin_image(
    content_cache: &arclain_data::ContentCache,
    expected_plugin_id: &str,
    cache_key: &str,
    bytes: &[u8],
    source_url: Option<&str>,
) -> Result<(), ApplicationError> {
    let raw_key = authorize_plugin_image_write(expected_plugin_id, cache_key)?;
    if bytes.len() > MAX_PLUGIN_IMAGE_BYTES as usize {
        return Err(oversized_image_error(
            "plugin image exceeds the maximum size",
            bytes.len(),
            MAX_PLUGIN_IMAGE_BYTES as usize,
        ));
    }
    content_cache
        .put_for_owner(
            &arclain_data::CacheOwner::plugin(expected_plugin_id),
            raw_key,
            bytes,
            // Re-exported by `arclain_core` from `arclain_db`; the same
            // type `ContentCache::put_for_owner` takes, reached through the
            // dependency this crate already has.
            arclain_core::CacheType::Screenshot,
            None,
            source_url,
        )
        .map(|_| ())
        .map_err(|error| {
            ApplicationError::new(
                ApplicationErrorKind::Internal,
                "failed to cache plugin image",
            )
            .with_diagnostic(error.to_string())
            .with_recoverability(Recoverability::Retry)
        })
}

/// Resolves the plugin-namespace raw key `cache_key` addresses, after
/// proving `expected_plugin_id` is entitled to write it.
///
/// The single authorization point for every plugin-namespace image write
/// ([`write_plugin_image`] and [`fetch_plugin_image`]), so the two cannot
/// drift into disagreeing about who may write where. Three refusals, in
/// order:
///
/// 1. A key this module never encoded is not a plugin key at all
///    (`NotFound`, matching the read).
/// 2. The key names its own owner, so on its own it is a bearer token for
///    a cache namespace: anyone holding the string `plugin-image:victim:k`
///    could write bytes that `victim` would later render as its own. The
///    caller must independently state which plugin it is acting for, and
///    the two must agree (`PermissionDenied`).
/// 3. A syntactically decodable but structurally impossible owner is
///    refused before it can mint a cache namespace (with its own quota
///    accounting) that no installed plugin could ever read back
///    (`InvalidInput`).
fn authorize_plugin_image_write<'key>(
    expected_plugin_id: &str,
    cache_key: &'key str,
) -> Result<&'key str, ApplicationError> {
    let (plugin_id, raw_key) = decode_plugin_image_cache_key(cache_key).ok_or_else(|| {
        ApplicationError::new(
            ApplicationErrorKind::NotFound,
            "unknown plugin image cache key",
        )
        .with_recoverability(Recoverability::Fatal)
    })?;
    if plugin_id != expected_plugin_id {
        return Err(ApplicationError::new(
            ApplicationErrorKind::PermissionDenied,
            "plugin image cache key belongs to a different plugin",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("cache_key"));
    }
    if arclain_plugins::types::PluginId::parse(plugin_id).is_err() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "plugin image cache key names a malformed plugin id",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("cache_key"));
    }
    Ok(raw_key)
}

// ============================================================================
// Display images: the host-owned namespace, and the fetch that fills either
// ============================================================================
// A frontend renders two kinds of image reference and must hold neither a
// content-cache handle nor an HTTP client to resolve them:
//
// - plugin-scoped keys, stamped by `rewrite_cache_keys` and served by
//   `read_plugin_image` / `write_plugin_image` / `fetch_plugin_image`;
// - host-owned keys (every other string -- legacy plugin renderers, the
//   carousel, anything the host itself cached) served by
//   `read_host_image` / `fetch_host_image` / `discard_host_image`.
//
// `is_plugin_image_key` is the total, single boundary between the two, and
// each family refuses the other's keys outright. That is what makes
// namespace separation structural rather than conventional: there is no
// key both families accept, so no caller can choose where a given key's
// bytes land, and a read can never resolve a namespace a write did not.

/// Maximum bytes [`crate::ArclainApp::read_host_image`] returns, and
/// [`crate::ArclainApp::fetch_host_image`] stores, for one host-owned
/// `cache_key`.
///
/// **Three times [`MAX_PLUGIN_IMAGE_BYTES`], on purpose.** The two
/// namespaces are different trust boundaries, not two spellings of one
/// rule:
///
/// - A plugin-scoped asset is named by plugin-authored content, so its
///   ceiling is a containment budget against a guest. 16 MiB is generous
///   for a cover or screenshot and deliberately tight against anything
///   else.
/// - A host-owned asset is one the host itself resolved. Nothing guest-
///   authored decides it exists, so the ceiling's job here is bounding a
///   single buffered response, not fencing an untrusted party.
///
/// The value is exactly the ceiling host image reads have *always* had:
/// before this surface existed the frontend read them through
/// `ContentCache::get`, whose default is
/// `arclain_data::DEFAULT_MAX_RESOURCE_SIZE_BYTES`. Matching it is the
/// point -- introducing a bound must not retire images that render today.
/// A host entry between the plugin cap and this one is not hypothetical
/// (see `a_host_image_larger_than_the_plugin_cap_still_round_trips`), and
/// narrowing to 16 MiB would have broken exactly those permanently: the
/// read refuses them, and so would every URL-fallback refetch that tried
/// to heal the gap.
///
/// The assertion below keeps that equality honest -- if the underlying
/// default ever moves, this fails to compile instead of silently
/// narrowing what a user can already see.
///
/// What this bound does *not* claim to do is cap decoded size: 50 MiB of
/// PNG can expand to far more RGBA, and bounding that is the frontend's
/// job at texture-upload time, not this constant's.
///
/// Held as a separate constant rather than an alias of the plugin cap so
/// the two can move independently, which is the whole reason they can
/// legitimately differ.
pub const MAX_HOST_IMAGE_BYTES: u32 = 50 * 1024 * 1024;

const _: () = assert!(
    MAX_HOST_IMAGE_BYTES as usize == arclain_data::DEFAULT_MAX_RESOURCE_SIZE_BYTES,
    "MAX_HOST_IMAGE_BYTES must stay equal to the content cache's default \
     materialized-read ceiling: it exists to preserve what host image reads \
     already accept, and drifting below it silently stops serving cached \
     images that render today",
);

/// Smallest response body accepted as an image by
/// [`fetch_display_image`].
///
/// Carried over verbatim from the pre-facade frontend fetch, which
/// rejected anything at or below this size on the grounds that "real
/// images are >1KB". It is a heuristic, not a format check, and its real
/// job is refusing a tiny placeholder or error document that happens to
/// carry an `image/*` content type.
const MIN_FETCHED_IMAGE_BYTES: usize = 1000;

/// Bytes for one image reference, plus whether serving them needed the
/// network -- the result of [`crate::ArclainApp::fetch_host_image`] and
/// [`crate::ArclainApp::fetch_plugin_image`].
///
/// `served_from_cache` is what makes "fetch once, then serve from cache"
/// observable to a caller (and testable) without exposing the cache
/// itself.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ImageBytesDto {
    pub bytes: Vec<u8>,
    pub served_from_cache: bool,
}

/// Resolves the host-namespace raw key `cache_key` addresses, refusing
/// anything that could name a row outside the host namespace.
///
/// The refusal is the host half of the namespace boundary, and it is a
/// security property rather than tidiness: without it, every host image
/// method would be a second, *unauthorized* door into a plugin's cache
/// namespace. It has to close **two** doors, not one, because a key can
/// name a plugin's row in two different vocabularies:
///
/// 1. **This module's encoding** (`plugin-image:{owner}:{key}`), which
///    `read_host_image` would otherwise resolve straight into the owner's
///    namespace.
/// 2. **The storage layer's own scoped encoding**
///    (`CacheOwner::scoped_key`). This one is not obvious and was a real
///    leak: `ContentCache`'s *host* read, remove, and has all fall back to
///    the unscoped keyspace for rows written before owner scoping existed
///    (`cache.rs`'s `matches!(owner, CacheOwner::Host) && self.service.has(key)`
///    branch) -- and every plugin row is indexed under exactly that scoped
///    string. So a host key that *is* a plugin row's scoped string reached
///    it: verified returning another plugin's bytes verbatim, and
///    `discard_host_image` verified destroying that plugin's entry.
///
/// Both checks below are therefore load-bearing, and they are deliberately
/// belt-and-braces: [`arclain_data::CacheOwner::from_scoped_key`] is the
/// storage layer's own parser (so this stays correct if the encoding
/// changes shape), while the sentinel check catches a *malformed* key
/// wearing the same marker byte, which the parser rejects but a future
/// fallback might not.
fn host_image_key(cache_key: &str) -> Result<&str, ApplicationError> {
    let refuse = |summary: &str| {
        Err(
            ApplicationError::new(ApplicationErrorKind::PermissionDenied, summary)
                .with_recoverability(Recoverability::Fatal)
                .with_field("cache_key"),
        )
    };
    if is_plugin_image_key(cache_key) {
        return refuse("cache key belongs to a plugin image namespace, not the host");
    }
    if arclain_data::CacheOwner::from_scoped_key(cache_key).is_some()
        || cache_key.starts_with(CACHE_SCOPED_KEY_SENTINEL)
    {
        return refuse("cache key names a storage-scoped cache row, not a host image");
    }
    Ok(cache_key)
}

/// First byte of [`arclain_data::CacheOwner::scoped_key`]'s output.
///
/// A control character precisely so it cannot occur in a key any caller
/// legitimately authors; a key carrying it is addressing the storage
/// layer's internal keyspace.
///
/// `scoped_key` allocates, so this cannot be a compile-time assertion --
/// `the_scoped_key_sentinel_matches_the_storage_encoding` checks it
/// against the real encoding instead, so a change there turns red rather
/// than quietly reopening the hole [`host_image_key`] closes.
const CACHE_SCOPED_KEY_SENTINEL: char = '\u{1}';

fn oversized_image_error(summary: &str, actual: usize, limit: usize) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, summary)
        .with_diagnostic(format!("{actual} bytes exceeds the {limit}-byte limit"))
        .with_recoverability(Recoverability::Fatal)
}

/// Reads a host-owned image out of the content cache, honoring
/// [`MAX_HOST_IMAGE_BYTES`]. Backs `ArclainApp::read_host_image`.
///
/// `NotFound` for a key the cache does not hold; `PermissionDenied` for a
/// plugin-scoped key (see [`host_image_key`]); `Internal` for an entry
/// whose size exceeds the cap or any other cache read failure --
/// `ContentCache::get_with_limit` enforces the cap during the read, so an
/// oversized entry never assembles into one `Vec<u8>` first.
pub(crate) fn read_host_image(
    content_cache: &arclain_data::ContentCache,
    cache_key: &str,
) -> Result<Vec<u8>, ApplicationError> {
    read_cached_host_image(content_cache, host_image_key(cache_key)?)?.ok_or_else(|| {
        ApplicationError::new(ApplicationErrorKind::NotFound, "host image not found")
            .with_recoverability(Recoverability::Fatal)
    })
}

/// The cache probe behind both [`read_host_image`] and
/// [`fetch_host_image`]: `Ok(None)` for a plain miss, so the fetch path
/// can tell "not cached yet" from "the cache is broken".
fn read_cached_host_image(
    content_cache: &arclain_data::ContentCache,
    raw_key: &str,
) -> Result<Option<Vec<u8>>, ApplicationError> {
    content_cache
        .get_with_limit(raw_key, MAX_HOST_IMAGE_BYTES as usize)
        .map_err(|error| {
            ApplicationError::new(ApplicationErrorKind::Internal, "failed to read host image")
                .with_diagnostic(error.to_string())
                .with_recoverability(Recoverability::Fatal)
        })
}

/// Drops a host-owned cached image, returning whether anything was
/// removed. Backs `ArclainApp::discard_host_image`.
///
/// Exists for exactly one caller shape: a frontend that read an entry,
/// failed to decode it, and must stop that permanently-corrupt entry from
/// being re-served forever. It refuses plugin-scoped keys like every other
/// host method -- evicting a plugin's cache entry is not a frontend's
/// decision to make.
pub(crate) fn discard_host_image(
    content_cache: &arclain_data::ContentCache,
    cache_key: &str,
) -> Result<bool, ApplicationError> {
    content_cache
        .remove(host_image_key(cache_key)?)
        .map_err(|error| {
            ApplicationError::new(
                ApplicationErrorKind::Internal,
                "failed to discard host image",
            )
            .with_diagnostic(error.to_string())
            .with_recoverability(Recoverability::Retry)
        })
}

/// Serves `cache_key` from the host image namespace, fetching `url` into
/// it first if it is not cached yet. Backs
/// `ArclainApp::fetch_host_image`.
///
/// `on_behalf_of_plugin` names whose *network policy* gates the request --
/// a plugin document's URL fallback must spend that plugin's domain
/// whitelist and rate-limit budget even when the key it fills is a legacy
/// host-owned one. It never selects a namespace: this function writes the
/// host namespace and nothing else, and refuses a plugin-scoped key
/// outright.
///
/// **Blocking.** Both the cache access and the fetch block; call it from a
/// blocking context (`spawn_blocking`), never from a runtime task.
pub(crate) fn fetch_host_image(
    content_cache: &arclain_data::ContentCache,
    http: &arclain_network::AsyncHttpClient,
    cache_key: &str,
    url: &str,
    on_behalf_of_plugin: Option<&str>,
) -> Result<ImageBytesDto, ApplicationError> {
    let raw_key = host_image_key(cache_key)?;
    if let Some(bytes) = read_cached_host_image(content_cache, raw_key)? {
        return Ok(ImageBytesDto {
            bytes,
            served_from_cache: true,
        });
    }
    let bytes = fetch_display_image(
        http,
        on_behalf_of_plugin,
        url,
        MAX_HOST_IMAGE_BYTES as usize,
    )?;
    content_cache
        .put(
            raw_key,
            &bytes,
            arclain_core::CacheType::Screenshot,
            None,
            Some(url),
        )
        .map_err(|error| {
            ApplicationError::new(ApplicationErrorKind::Internal, "failed to cache host image")
                .with_diagnostic(error.to_string())
                .with_recoverability(Recoverability::Retry)
        })?;
    Ok(ImageBytesDto {
        bytes,
        served_from_cache: false,
    })
}

/// Serves `cache_key` from the plugin namespace it encodes, fetching `url`
/// into it first if it is not cached yet. Backs
/// `ArclainApp::fetch_plugin_image`.
///
/// The fetch spends `plugin_id`'s own network budget and lands in
/// `plugin_id`'s own cache namespace -- the same namespace
/// [`read_plugin_image`] resolves the key from, because both go through
/// one authorization ([`authorize_plugin_image_write`]) and one owner
/// encoding. `plugin_id` must match the owner the key names.
///
/// **Blocking.** Same contract as [`fetch_host_image`].
pub(crate) fn fetch_plugin_image(
    content_cache: &arclain_data::ContentCache,
    http: &arclain_network::AsyncHttpClient,
    plugin_id: &str,
    cache_key: &str,
    url: &str,
) -> Result<ImageBytesDto, ApplicationError> {
    let raw_key = authorize_plugin_image_write(plugin_id, cache_key)?;
    let cached = content_cache
        .get_with_limit_for_owner(
            &arclain_data::CacheOwner::plugin(plugin_id),
            raw_key,
            MAX_PLUGIN_IMAGE_BYTES as usize,
        )
        .map_err(|error| {
            ApplicationError::new(
                ApplicationErrorKind::Internal,
                "failed to read plugin image",
            )
            .with_diagnostic(error.to_string())
            .with_recoverability(Recoverability::Fatal)
        })?;
    if let Some(bytes) = cached {
        return Ok(ImageBytesDto {
            bytes,
            served_from_cache: true,
        });
    }
    let bytes = fetch_display_image(http, Some(plugin_id), url, MAX_PLUGIN_IMAGE_BYTES as usize)?;
    write_plugin_image(content_cache, plugin_id, cache_key, &bytes, Some(url))?;
    Ok(ImageBytesDto {
        bytes,
        served_from_cache: false,
    })
}

/// Fetches `url` as a displayable image, bounded to `max_bytes`.
///
/// Bounded *while reading*, not after: the network layer refuses a
/// declared `Content-Length` over the ceiling and stops streaming the
/// moment the body crosses it, so a hostile URL can never make the
/// application buffer an unbounded body before the cap is checked. That is
/// the half a frontend could not own -- it only ever saw a fully buffered
/// body.
///
/// Because the ceiling is enforced during the read, an oversized body
/// never reaches the checks below -- it comes back as
/// `HttpError::ResponseTooLarge`, which [`image_fetch_error`] classifies
/// as a *permanent* refusal. A post-hoc `body.len() > max_bytes` check
/// here would be unreachable.
///
/// The three response checks are the pre-facade frontend's, moved here
/// intact: a 200 status, a body over [`MIN_FETCHED_IMAGE_BYTES`], and an
/// `image/*` content type. Anything else is `InvalidInput`, so a cached
/// HTML error page can never be served back as an image.
///
/// **Blocking**: the network calls below use `block_on` internally and
/// must run on a blocking thread.
fn fetch_display_image(
    http: &arclain_network::AsyncHttpClient,
    on_behalf_of_plugin: Option<&str>,
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, ApplicationError> {
    let fetched = match on_behalf_of_plugin {
        Some(plugin_id) => {
            http.blocking_get_response_for_plugin_with_limit(plugin_id, url, max_bytes)
        }
        None => http.blocking_get_response_with_limit(url, false, max_bytes),
    };
    let response = fetched.map_err(image_fetch_error)?;

    if response.status_code != 200 {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch returned an unexpected status",
        )
        .with_diagnostic(format!("HTTP status {}", response.status_code))
        .with_recoverability(Recoverability::Retry));
    }
    if !response
        .content_type
        .as_deref()
        .is_some_and(|content_type| content_type.starts_with("image/"))
    {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch returned a non-image content type",
        )
        .with_recoverability(Recoverability::Fatal));
    }
    if response.body.len() <= MIN_FETCHED_IMAGE_BYTES {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch returned too few bytes to be an image",
        )
        .with_diagnostic(format!(
            "{} bytes is at or below the {MIN_FETCHED_IMAGE_BYTES}-byte floor",
            response.body.len()
        ))
        .with_recoverability(Recoverability::Retry));
    }
    Ok(response.body)
}

/// Maps a network failure onto the envelope, keeping "too large" apart
/// from "the network misbehaved".
///
/// The distinction is the difference between a broken image and a hot
/// loop: an oversized asset is exactly as oversized on the next attempt,
/// while an unreachable host may well not be. Size refusals are therefore
/// `InvalidInput` with `Recoverability::Fatal`, everything else stays
/// `Backend` with `Recoverability::Retry`.
///
/// This classification is only half the fix -- a caller has to *act* on
/// it. `arclain_ui`'s image store keeps the recoverability across its own
/// error boundary and refuses to re-arm the renderer's 30 s retry for a
/// key refused as `Fatal`; see `ImageFetchError` there. Classifying
/// without that was the earlier bug: correct at this boundary, discarded
/// one call before the only code that could use it.
fn image_fetch_error(error: arclain_network::HttpError) -> ApplicationError {
    match error {
        arclain_network::HttpError::ResponseTooLarge { limit } => ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "image fetch exceeded the maximum size",
        )
        .with_diagnostic(format!("response body exceeds the {limit}-byte limit"))
        .with_recoverability(Recoverability::Fatal),
        other => ApplicationError::new(ApplicationErrorKind::Backend, "image fetch failed")
            .with_diagnostic(other.to_string())
            .with_recoverability(Recoverability::Retry)
            // `with_recoverability` does not touch `retryable`, which
            // defaults to false -- leaving the two halves of the envelope
            // contradicting each other for exactly the case where a caller
            // most wants to trust them.
            .with_retryable(true),
    }
}

// ============================================================================
// Facade-level DTOs (wrap `wirt::ui_model` shapes with this
// crate's own opaque ids).
// ============================================================================

/// One capability a plugin's manifest declares, as carried by
/// [`PluginSummary::capabilities`]. Mirrors
/// `arclain_plugins::types::PluginCapability` variant for variant.
///
/// A mirrored enum rather than the `Vec<String>` of `{:?}`-formatted
/// variant names the pre-facade plugins page built for itself: a `Debug`
/// spelling is not a contract, so a non-Rust frontend receiving one could
/// neither match on it nor translate it without re-deriving Rust's own
/// formatting. [`Self::label`] keeps that rendering available to whoever
/// wants it, from one place, instead of every caller re-deriving it.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapabilityDto {
    FileRead,
    FileWrite,
    Network,
    ArchiveMetadataRead,
    ArchiveMetadataWrite,
    ArchiveModify,
}

impl From<arclain_plugins::types::PluginCapability> for PluginCapabilityDto {
    fn from(capability: arclain_plugins::types::PluginCapability) -> Self {
        use arclain_plugins::types::PluginCapability as Source;
        // No wildcard arm: a capability added upstream fails to compile
        // here until this mirror carries it too.
        match capability {
            Source::FileRead => Self::FileRead,
            Source::FileWrite => Self::FileWrite,
            Source::Network => Self::Network,
            Source::ArchiveMetadataRead => Self::ArchiveMetadataRead,
            Source::ArchiveMetadataWrite => Self::ArchiveMetadataWrite,
            Source::ArchiveModify => Self::ArchiveModify,
        }
    }
}

impl PluginCapabilityDto {
    /// The permission label a plugin detail view renders for this
    /// capability, byte-identical to the `{:?}` spelling the pre-facade
    /// plugins page produced (pinned by
    /// `capability_labels_match_the_pre_facade_debug_spelling`), so a
    /// frontend that adopts this DTO renders exactly what it rendered
    /// before.
    pub fn label(self) -> &'static str {
        match self {
            Self::FileRead => "FileRead",
            Self::FileWrite => "FileWrite",
            Self::Network => "Network",
            Self::ArchiveMetadataRead => "ArchiveMetadataRead",
            Self::ArchiveMetadataWrite => "ArchiveMetadataWrite",
            Self::ArchiveModify => "ArchiveModify",
        }
    }
}

/// Renderer-neutral metadata returned before a package install is approved.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginInstallPreviewDto {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub abi: String,
    pub capabilities: Vec<PluginCapabilityDto>,
    pub network_domains: Vec<String>,
    pub fingerprint: String,
}

impl From<arclain_plugins::PluginInstallPreview> for PluginInstallPreviewDto {
    fn from(preview: arclain_plugins::PluginInstallPreview) -> Self {
        let manifest = preview.manifest;
        Self {
            plugin_id: manifest.plugin.id,
            name: manifest.plugin.name,
            version: manifest.plugin.version,
            author: manifest.plugin.author,
            abi: manifest.wirt.abi,
            capabilities: manifest
                .capabilities
                .to_capabilities()
                .into_iter()
                .map(PluginCapabilityDto::from)
                .collect(),
            network_domains: manifest.capabilities.network_domains,
            fingerprint: preview.fingerprint.to_string(),
        }
    }
}

/// One plugin as reported by [`crate::ArclainApp::plugins`]: enough to
/// render a plugin list/settings row without a caller needing to reach
/// into `arclain_plugins::PluginListItem`/`PluginManifest` directly.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PluginQuarantineState {
    #[default]
    Clear,
    Retryable {
        failed_retries: u8,
    },
    PersistentlyDisabled {
        failed_retries: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The manifest's `[plugin] author`, verbatim. A plain `String`, not
    /// an `Option`, because the manifest field itself is required and
    /// carries no absent state -- an author nobody filled in is the empty
    /// string at the source, and inventing a `None` here would claim a
    /// distinction the manifest cannot express.
    pub author: String,
    /// The manifest's `[plugin] description`, verbatim. A `String` for
    /// the same reason [`Self::author`] is one.
    pub description: String,
    /// Every capability the manifest grants this plugin, in
    /// `arclain_plugins::types::CapabilitiesConfig::to_capabilities`'s own
    /// order (which follows the source enum's declaration order, not the
    /// manifest's key order), deduplicated by construction because each
    /// capability has exactly one manifest flag.
    pub capabilities: Vec<PluginCapabilityDto>,
    /// Per-surface visibility overrides persisted for this plugin.
    ///
    /// Keys are frontend-neutral capability slots such as `toolbar` and
    /// `info_panel`. Missing keys retain that surface's default behavior.
    pub visibility: BTreeMap<String, bool>,
    pub enabled: bool,
    /// `Some(reason)` if this plugin was discovered on disk but failed to
    /// load -- see `arclain_plugins::manager::FailedPlugin`. A plugin
    /// reported this way has no running instance and `arclain_plugins`
    /// records only its id and failure reason, not its manifest: `enabled`
    /// is always `false`, `name`/`version`/`author`/`description` are
    /// always empty strings rather than whatever the manifest claimed, and
    /// `capabilities` is always empty -- *not* "this plugin declared no
    /// capabilities", but "no manifest survived to say".
    pub load_error: Option<String>,
    pub quarantine_state: PluginQuarantineState,
    pub last_reason: Option<String>,
}

/// A renderer-neutral plugin UI document: [`wirt::ui_model::PluginUiNodeDto`]'s
/// normalized tree, plus the session/plugin/revision identity
/// `wirt::ui_model` itself cannot carry (it has no
/// dependency on this crate's opaque ids).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginUiDocument {
    pub session_id: PluginSessionId,
    pub plugin_id: String,
    /// A stable, human-readable slug for this document's extension point
    /// (see `PluginExtensionPointDto::region_slug`). Unrelated to
    /// `arclain_core`'s `UiRegion` (the native chrome-layout
    /// customization enum) -- same English word, different concept: this
    /// is a plugin-*document* slot, not a customizable toolbar/panel
    /// region.
    pub region_id: String,
    pub extension_point: PluginExtensionPointDto,
    pub revision: u64,
    pub root: PluginUiNodeDto,
}

/// The bounded, typed side effects and updated document one plugin
/// interaction produces -- the payload of
/// `OperationResult::PluginUiUpdated`.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginUiUpdate {
    pub document: PluginUiDocument,
    pub intents: Vec<PluginHostIntentDto>,
}

/// A renderer's request to interact with one node of an open plugin
/// session -- the argument to `ArclainApp::start_plugin_action`.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginActionRequest {
    pub session_id: PluginSessionId,
    pub node_id: String,
    pub action: PluginActionDto,
}

/// What `ArclainApp::open_plugin_session` returns: the freshly minted
/// session id and its first document.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginSessionSnapshot {
    pub session_id: PluginSessionId,
    pub document: PluginUiDocument,
}

// ============================================================================
// Domain access: per-plugin whitelist + URL security analysis
// ============================================================================
// The two network-owned shapes a frontend needs to render a plugin's
// "Domain Access" section, mirrored into this crate so no frontend has to
// depend on `arclain-network` for them. Kept in its own delimited section
// for the same reason as the task sections in `crate::runtime`: a
// concurrent worktree may be touching this same shared file.

/// One entry of a plugin's domain whitelist, as reported by
/// [`crate::ArclainApp::plugin_domain_whitelist`]. Mirrors
/// `arclain_network::features::whitelist::WhitelistEntry` field for
/// field.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DomainWhitelistEntryDto {
    /// The plugin that requested this domain.
    pub plugin_id: String,
    /// The domain itself, normalized to lowercase by the whitelist.
    pub domain: String,
    /// Whether this domain is currently granted (by explicit user
    /// approval or by the plugin's loaded manifest). `false` means the
    /// plugin asked for it and it is still pending.
    pub approved: bool,
}

impl From<arclain_network::features::whitelist::WhitelistEntry> for DomainWhitelistEntryDto {
    fn from(entry: arclain_network::features::whitelist::WhitelistEntry) -> Self {
        Self {
            plugin_id: entry.plugin_id,
            domain: entry.domain,
            approved: entry.approved,
        }
    }
}

/// Backs [`crate::ArclainApp::plugin_domain_whitelist`]: every entry
/// `whitelist` holds for `plugin_id`, in a deterministic order.
///
/// Sorted by domain because the underlying store keeps its domains in
/// `HashSet`s -- so `get_all_entries` returns them in an order that can
/// differ between two calls in the same process, which would make a
/// frontend's list of domains reshuffle itself between frames. Sorting
/// here means every caller sees one stable order without needing to know
/// that.
///
/// A blank `plugin_id` is rejected rather than answered with an empty
/// list: no plugin id is empty, so the only way to ask for one is a
/// caller bug, and reporting it as such beats silently claiming the
/// (nonexistent) plugin requested no domains.
pub(crate) fn plugin_domain_whitelist(
    whitelist: &arclain_network::features::whitelist::DomainWhitelist,
    plugin_id: &str,
) -> Result<Vec<DomainWhitelistEntryDto>, ApplicationError> {
    if plugin_id.trim().is_empty() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "plugin id must not be empty",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("plugin_id"));
    }
    let mut entries: Vec<DomainWhitelistEntryDto> = whitelist
        .get_all_entries()
        .into_iter()
        .filter(|entry| entry.plugin_id == plugin_id)
        .map(DomainWhitelistEntryDto::from)
        .collect();
    entries.sort_by(|left, right| left.domain.cmp(&right.domain));
    Ok(entries)
}

/// The result of [`analyze_url`]: what a URL's host actually resolves to,
/// plus every security warning the analysis raised. Mirrors
/// `arclain_network::features::security::DomainInfo`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DomainAnalysisDto {
    /// The URL exactly as analyzed.
    pub full_url: String,
    /// The registrable domain (`dlsite.com`, not `api.dlsite.com`).
    pub effective_domain: String,
    /// The full host, subdomains included.
    pub host: String,
    /// The top-level domain (`com`, `co.jp`).
    pub tld: String,
    /// Every warning raised, in detection order. Empty means nothing
    /// suspicious was found.
    pub warnings: Vec<DomainWarningDto>,
}

impl From<arclain_network::features::security::DomainInfo> for DomainAnalysisDto {
    fn from(info: arclain_network::features::security::DomainInfo) -> Self {
        Self {
            full_url: info.full_url,
            effective_domain: info.effective_domain,
            host: info.host,
            tld: info.tld,
            warnings: info
                .warnings
                .into_iter()
                .map(DomainWarningDto::from)
                .collect(),
        }
    }
}

/// One security warning about a URL, mirroring
/// `arclain_network::features::security::DomainWarning` variant for
/// variant.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainWarningDto {
    /// A lookalike character from another alphabet (homograph attack).
    HomographDetected {
        suspicious_char: char,
        position: usize,
        looks_like: char,
    },
    /// A subdomain shaped to look like a different, trusted domain.
    SuspiciousSubdomain {
        subdomain: String,
        looks_like: String,
    },
    /// A top-level domain rarely used for legitimate traffic.
    UnusualTld { tld: String },
    /// The URL names a bare IP address instead of a domain.
    IpAddress { ip: String },
    /// The URL points at localhost or a private network range.
    LocalhostOrPrivate,
    /// The URL contains encoded characters that may hide its real
    /// destination.
    SuspiciousEncoding,
    /// The URL nests an unusual number of subdomain levels.
    ExcessiveSubdomains { count: usize },
    /// The domain contains keywords commonly used in phishing.
    SuspiciousKeywords { keywords: Vec<String> },
}

impl From<arclain_network::features::security::DomainWarning> for DomainWarningDto {
    fn from(warning: arclain_network::features::security::DomainWarning) -> Self {
        use arclain_network::features::security::DomainWarning as Source;
        match warning {
            Source::HomographDetected {
                suspicious_char,
                position,
                looks_like,
            } => Self::HomographDetected {
                suspicious_char,
                position,
                looks_like,
            },
            Source::SuspiciousSubdomain {
                subdomain,
                looks_like,
            } => Self::SuspiciousSubdomain {
                subdomain,
                looks_like,
            },
            Source::UnusualTld { tld } => Self::UnusualTld { tld },
            Source::IpAddress { ip } => Self::IpAddress { ip },
            Source::LocalhostOrPrivate => Self::LocalhostOrPrivate,
            Source::SuspiciousEncoding => Self::SuspiciousEncoding,
            Source::ExcessiveSubdomains { count } => Self::ExcessiveSubdomains { count },
            Source::SuspiciousKeywords { keywords } => Self::SuspiciousKeywords { keywords },
        }
    }
}

impl DomainWarningDto {
    /// Rebuilds the network crate's own warning value, so
    /// [`Self::description`] and [`Self::is_critical`] can delegate to it.
    ///
    /// A private method rather than a `From` impl on purpose: a trait impl
    /// cannot be `pub(crate)`, so `impl From<&DomainWarningDto> for
    /// arclain_network::…::DomainWarning` would put an `arclain-network`
    /// type back on this crate's *public* API -- exactly the coupling the
    /// mirrored DTO exists to keep away from frontends.
    fn to_source(&self) -> arclain_network::features::security::DomainWarning {
        use arclain_network::features::security::DomainWarning as Target;
        match self {
            Self::HomographDetected {
                suspicious_char,
                position,
                looks_like,
            } => Target::HomographDetected {
                suspicious_char: *suspicious_char,
                position: *position,
                looks_like: *looks_like,
            },
            Self::SuspiciousSubdomain {
                subdomain,
                looks_like,
            } => Target::SuspiciousSubdomain {
                subdomain: subdomain.clone(),
                looks_like: looks_like.clone(),
            },
            Self::UnusualTld { tld } => Target::UnusualTld { tld: tld.clone() },
            Self::IpAddress { ip } => Target::IpAddress { ip: ip.clone() },
            Self::LocalhostOrPrivate => Target::LocalhostOrPrivate,
            Self::SuspiciousEncoding => Target::SuspiciousEncoding,
            Self::ExcessiveSubdomains { count } => Target::ExcessiveSubdomains { count: *count },
            Self::SuspiciousKeywords { keywords } => Target::SuspiciousKeywords {
                keywords: keywords.clone(),
            },
        }
    }

    /// A human-readable sentence describing this warning, suitable for
    /// rendering next to the domain it was raised for.
    ///
    /// Delegates to the network crate's own wording rather than
    /// re-spelling it here: two independent copies of the same eight
    /// sentences would drift the first time one side is reworded, and a
    /// mirrored DTO whose text silently disagrees with the analysis it
    /// mirrors is worse than no mirror at all.
    pub fn description(&self) -> String {
        self.to_source().description()
    }

    /// Whether this warning is severe enough that a request to the domain
    /// should be blocked rather than merely flagged. Delegates for the
    /// same single-source-of-truth reason [`Self::description`] does.
    pub fn is_critical(&self) -> bool {
        self.to_source().is_critical()
    }
}

/// Analyzes `url` for domain-level security problems: homograph
/// characters, lookalike subdomains, unusual TLDs, bare IP hosts, and
/// suspicious patterns.
///
/// Pure and total: no application handle, no runtime, no I/O, no shared
/// state -- which is exactly why it is a free function rather than an
/// [`crate::ArclainApp`] method. A frontend calls it directly to decide
/// whether to warn about a domain it is about to display.
///
/// `InvalidInput` if `url` cannot be parsed into a host at all.
pub fn analyze_url(url: &str) -> Result<DomainAnalysisDto, ApplicationError> {
    arclain_network::features::security::analyze_url(url)
        .map(DomainAnalysisDto::from)
        .map_err(|message| {
            ApplicationError::new(
                ApplicationErrorKind::InvalidInput,
                "URL could not be analyzed",
            )
            .with_diagnostic(message)
            .with_recoverability(Recoverability::UserAction)
            .with_field("url")
        })
}

// ============================================================================
// Plugin chrome: the status counts and top-tab strip an application frame
// renders, plus the aggregated plugin network log
// ============================================================================
// Kept in its own delimited section for the same reason as the domain-access
// section above: a concurrent worktree may be touching this same shared file.
//
// Two *separate* read models, deliberately not one call. A frontend polls
// them on different cadences -- the top-tab strip's badges are chrome that
// redraws with the window, the network log is a diagnostics page that only
// matters while it is open -- and folding them together would force the
// tighter of the two cadences onto both, re-entering every enabled plugin's
// WASM for a log nobody is looking at.

/// Counts-only plugin status, as reported by
/// [`PluginChromeSnapshot::summary`]. Mirrors
/// `arclain_plugins::manager::PluginStatusSummary` field for field.
///
/// `u64` rather than the source's `usize`: these cross a frontend
/// boundary (a Dart bridge today, whatever comes next later), and a
/// pointer-width integer is not a stable wire type. Widening is lossless
/// on every platform this application builds for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginStatusSummaryDto {
    /// Every plugin that loaded successfully, enabled or not. Failed
    /// plugins are *not* counted -- they never reached the manager's
    /// plugin map (see [`PluginSummary::load_error`]).
    pub total: u64,
    /// How many of [`Self::total`] are currently enabled.
    pub enabled: u64,
}

impl From<arclain_plugins::manager::PluginStatusSummary> for PluginStatusSummaryDto {
    fn from(summary: arclain_plugins::manager::PluginStatusSummary) -> Self {
        // Destructured rather than field-accessed, for the reason
        // `crate::layout`'s mirrors are: a field added upstream fails to
        // compile here until this mirror carries it too.
        let arclain_plugins::manager::PluginStatusSummary { total, enabled } = summary;
        Self {
            total: total as u64,
            enabled: enabled as u64,
        }
    }
}

/// The badge a plugin's top tab carries, if any. Mirrors
/// `arclain_plugins::types::BadgeConfig` field for field.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginBadgeDto {
    /// A numeric count to render. `None`, or `Some(0)`, means the badge
    /// carries no number -- the pre-facade renderer draws nothing for a
    /// zero count, and this DTO preserves the distinction rather than
    /// normalizing it, because only the renderer knows what it wants to
    /// do with a zero.
    pub count: Option<u32>,
    /// Render a plain dot instead of a number.
    pub dot: bool,
    /// The plugin's semantic color name (`"red"`, `"green"`, `"blue"`,
    /// `"orange"`, or anything else). Plugin-authored and therefore
    /// untrusted: a renderer maps known names onto its own theme and
    /// falls back for the rest, and must never treat this as a color
    /// literal.
    pub color: String,
}

impl From<arclain_plugins::types::BadgeConfig> for PluginBadgeDto {
    fn from(badge: arclain_plugins::types::BadgeConfig) -> Self {
        let arclain_plugins::types::BadgeConfig { count, dot, color } = badge;
        Self { count, dot, color }
    }
}

/// One top-level tab an enabled plugin registers, as reported by
/// [`PluginChromeSnapshot::top_tabs`]. Mirrors
/// `arclain_plugins::types::TopTabConfig` field for field, plus the
/// owning [`Self::plugin_id`] that the manager reports alongside each
/// config rather than inside it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginTopTabDto {
    /// The plugin that registered this tab. Host-validated
    /// (`arclain_plugins::types::PluginId`: at most 64 bytes of
    /// `[A-Za-z0-9_-]`), never plugin-authored free text.
    pub plugin_id: String,
    /// The tab's own id, unique only within its plugin. Plugin-authored,
    /// and passed through verbatim: it is identity, not display text, and
    /// two absurd ids truncated to the same prefix would select each
    /// other's tab.
    pub id: String,
    /// Display text, truncated to [`MAX_PLUGIN_TOP_TAB_TEXT_BYTES`].
    pub label: String,
    /// An icon *name* (`"GLOBE"`, `"DATABASE"`, ...) or a glyph, likewise
    /// truncated to [`MAX_PLUGIN_TOP_TAB_TEXT_BYTES`]. Which names are
    /// recognized is the renderer's business, not this facade's.
    pub icon: String,
    pub badge: Option<PluginBadgeDto>,
    /// Lower sorts earlier. Already applied: [`PluginChromeSnapshot::
    /// top_tabs`] arrives sorted.
    pub priority: u32,
}

/// The longest a plugin-authored *display* field of a [`PluginTopTabDto`]
/// may be, matching [`crate::layout::MAX_UI_ITEM_TEXT_BYTES`] (the bound
/// `runtime::bootstrap`'s `sync_plugin_top_tab_items` already applies to
/// this very data on its way into the layout database).
///
/// A tab label is tens of bytes; a plugin is untrusted WASM whose whole
/// `get-top-tabs` result is bounded only by the runtime's ~1 MiB quota.
/// Without this, one plugin could hand a frontend a near-megabyte string
/// to lay out in its tab strip on every frame.
pub const MAX_PLUGIN_TOP_TAB_TEXT_BYTES: usize = crate::layout::MAX_UI_ITEM_TEXT_BYTES;

/// Truncates plugin-authored display text at the largest char boundary at
/// or under [`MAX_PLUGIN_TOP_TAB_TEXT_BYTES`]. Truncating rather than
/// dropping the tab, for the same reason `sync_plugin_top_tab_items`
/// truncates rather than skipping on a long *label*: an over-long caption
/// must not cost the user a whole tab.
fn clamp_top_tab_text(mut value: String) -> String {
    if value.len() <= MAX_PLUGIN_TOP_TAB_TEXT_BYTES {
        return value;
    }
    let mut end = MAX_PLUGIN_TOP_TAB_TEXT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

impl From<(String, arclain_plugins::types::TopTabConfig)> for PluginTopTabDto {
    fn from(entry: (String, arclain_plugins::types::TopTabConfig)) -> Self {
        let (plugin_id, config) = entry;
        // Destructured for the drift guard `crate::layout`'s mirrors use:
        // a field added to `TopTabConfig` fails to compile here until this
        // mirror carries it too. `plugin_id` is the one field with no
        // counterpart in the source struct -- the manager reports it
        // beside each config, in the tuple this impl takes.
        let arclain_plugins::types::TopTabConfig {
            id,
            label,
            icon,
            badge,
            priority,
        } = config;
        Self {
            plugin_id,
            id,
            label: clamp_top_tab_text(label),
            icon: clamp_top_tab_text(icon),
            badge: badge.map(PluginBadgeDto::from),
            priority,
        }
    }
}

/// What [`crate::ArclainApp::plugin_chrome`] reports: everything an
/// application frame needs to draw its plugin-owned chrome in one read.
///
/// [`Default`] is what an application composed without a plugin runtime
/// reports -- zero counts, no tabs -- so a frontend renders "no plugin
/// chrome" through the same path it renders "no plugins enabled".
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginChromeSnapshot {
    pub summary: PluginStatusSummaryDto,
    /// Every enabled plugin's top tabs, already sorted by
    /// [`PluginTopTabDto::priority`] (ascending). A plugin whose
    /// `get-top-tabs` call fails contributes nothing and does not fail the
    /// read -- matching `EnabledPluginSnapshot::get_all_top_tabs`, which
    /// logs and skips.
    pub top_tabs: Vec<PluginTopTabDto>,
}

/// One line of the aggregated plugin network log, as reported by
/// [`crate::ArclainApp::plugin_network_log`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginNetworkLogEntryDto {
    /// When the plugin logged this line, as Unix milliseconds -- the same
    /// wire shape [`crate::archive::ArchiveEntryDto::modified_at_unix_ms`]
    /// uses, rather than a `SystemTime` no bridge can carry. Negative for
    /// the (pathological) case of a clock set before 1970.
    pub logged_at_unix_ms: i64,
    /// The plugin-authored message. Already bounded at its source: each
    /// line is truncated to 4 KiB, and each plugin retains at most 256
    /// lines / 256 KiB, oldest evicted first.
    pub message: String,
}

/// Converts a `SystemTime` to Unix milliseconds, saturating rather than
/// panicking on a time so far from the epoch that it does not fit an
/// `i64` (roughly ±292 million years -- reachable only from a corrupted
/// clock, never from a real log line).
fn unix_millis(time: std::time::SystemTime) -> i64 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(before_epoch) => i64::try_from(before_epoch.duration().as_millis())
            .map(|millis| -millis)
            .unwrap_or(i64::MIN),
    }
}

impl From<(std::time::SystemTime, String)> for PluginNetworkLogEntryDto {
    fn from(entry: (std::time::SystemTime, String)) -> Self {
        let (logged_at, message) = entry;
        Self {
            logged_at_unix_ms: unix_millis(logged_at),
            message,
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

fn plugin_manager_unavailable() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "plugin runtime is unavailable",
    )
    .with_recoverability(Recoverability::Fatal)
}

/// Resolves `AppRuntime::plugin_manager()`'s `Option`, for every facade
/// method in `runtime::mod` that needs a live plugin manager. Kept here
/// (not duplicated per call site) so every "no plugin runtime" error
/// looks identical.
pub(crate) fn require_manager(
    manager: Option<Arc<SyncMutex<PluginManager>>>,
) -> Result<Arc<SyncMutex<PluginManager>>, ApplicationError> {
    manager.ok_or_else(plugin_manager_unavailable)
}

fn plugin_not_found(plugin_id: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "plugin not found")
        .with_diagnostic(format!("plugin id: {plugin_id}"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("plugin_id")
}

/// The exact `summary` every disabled-plugin refusal carries. Exported so
/// a frontend recognizes the refusal by comparing against *this* symbol
/// rather than spelling the sentence itself: if the wording ever changes,
/// both ends move together. See [`is_plugin_disabled_refusal`], which is
/// what callers should actually use.
pub const PLUGIN_DISABLED_SUMMARY: &str = "plugin is disabled";

/// The refusal every plugin surface produces for a plugin that exists but
/// is currently disabled -- see [`require_enabled_plugin`].
///
/// `PermissionDenied`, deliberately *not* `NotFound`: a renderer that
/// asked for a disabled plugin's layout should draw nothing, while one
/// that asked for a plugin that does not exist has a stale item id it
/// needs to drop. Those are different repairs, so they cannot share an
/// error kind (that ambiguity is exactly what
/// [`is_plugin_disabled_refusal`] exists to remove).
///
/// No `field` is set, unlike [`plugin_not_found`]'s `plugin_id`: nothing
/// about the caller's *request* is wrong. The same request succeeds
/// unchanged once the plugin is enabled again, so pointing at a request
/// field would misdirect a frontend into highlighting an input the user
/// cannot fix there. The plugin the refusal is about is named in the
/// diagnostic instead.
fn plugin_disabled(plugin_id: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::PermissionDenied,
        PLUGIN_DISABLED_SUMMARY,
    )
    .with_diagnostic(format!("plugin id: {plugin_id}"))
    .with_recoverability(Recoverability::UserAction)
}

/// True for exactly the refusal a disabled plugin's surface produces --
/// `open_plugin_session`, `open_plugin_session_for_archive`,
/// `plugin_ui_document`, and a `start_plugin_action` operation's
/// `Failed` state.
///
/// The distinction a frontend needs: a slot whose plugin was turned off
/// should quietly stop drawing (and may start drawing again if the user
/// turns it back on), while a slot naming a plugin that does not exist is
/// a stale reference to discard, and a genuine failure deserves the error
/// the user sees. Only the middle case is `NotFound`, so
/// `kind`-matching alone would collapse the first two.
pub fn is_plugin_disabled_refusal(error: &ApplicationError) -> bool {
    error.kind == ApplicationErrorKind::PermissionDenied && error.summary == PLUGIN_DISABLED_SUMMARY
}

/// The one enabled-flag gate every plugin surface in this crate goes
/// through, so "a disabled plugin does not run" holds no matter which
/// frontend asks and no matter which extension point it asks for.
///
/// Three outcomes, and the distinction between the last two is the point:
/// - enabled -- `Ok(())`;
/// - known, but currently disabled -- [`plugin_disabled`]
///   (`PermissionDenied`);
/// - not a plugin this manager knows at all -- [`plugin_not_found`]
///   (`NotFound`).
///
/// Both reads happen under a single `PluginManager` lock acquisition, and
/// the enabled flag they consult is the same
/// `PluginManager::enabled_plugins` map that [`PluginSessionStore::plugins`]
/// reports through `PluginSummary::enabled` and that
/// [`PluginSessionStore::set_plugin_enabled`] writes. There is therefore
/// no instant at which this gate and `ArclainApp::plugins` disagree about
/// a plugin: a frontend's *cached copy* of the last `plugins()` answer can
/// of course be stale, which is precisely why this gate is re-evaluated at
/// each use rather than trusted from the caller.
pub(crate) fn require_enabled_plugin(
    manager: &SyncMutex<PluginManager>,
    plugin_id: &str,
) -> Result<(), ApplicationError> {
    let manager = manager.lock();
    if manager.is_plugin_enabled(plugin_id) {
        return Ok(());
    }
    // Only reached on refusal, so the metadata clone never costs the
    // happy path anything.
    if manager.get_plugin_metadata(plugin_id).is_none() {
        return Err(plugin_not_found(plugin_id));
    }
    Err(plugin_disabled(plugin_id))
}

fn unknown_plugin_session(session_id: PluginSessionId) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such plugin session")
        .with_diagnostic(format!("session id: {}", session_id.into_raw()))
        .with_recoverability(Recoverability::Fatal)
}

fn normalize_error(error: PluginUiNormalizeError) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Plugin,
        "plugin UI layout is malformed",
    )
    .with_diagnostic(error.to_string())
    .with_recoverability(Recoverability::Fatal)
}

fn plugin_execution_error(error: PluginError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Plugin, "plugin execution failed")
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::Retry)
}

/// The longest caller-selected plugin package path the facade accepts.
/// A path this long cannot name a real file on any supported platform, so
/// rejecting it costs nothing and keeps an absurd caller-supplied value
/// out of the manager, the error envelope, and the log.
pub const MAX_PLUGIN_INSTALL_PATH_BYTES: usize = 32 * 1024;

#[cfg(test)]
fn invalid_install_path(reason: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "plugin file cannot be installed",
    )
    .with_diagnostic(reason.to_string())
    .with_recoverability(Recoverability::UserAction)
    .with_field("wasm_path")
}

/// Attaches the caller's own `wasm_path` to an install error, so a user
/// who picked the wrong file is told which one.
///
/// Sound because this path is by definition user-chosen -- the exact case
/// [`ApplicationError::with_path`] exists for -- and it is the only way
/// the file reaches the user at all: `with_diagnostic` redacts path-like
/// tokens from free text, so the manager's own "File does not exist"
/// wording arrives with the filename already scrubbed out.
///
/// Skipped for a path over [`MAX_PLUGIN_INSTALL_PATH_BYTES`]: echoing
/// tens of kilobytes back into an error envelope helps nobody, and the
/// diagnostic already says what was wrong with it.
#[cfg(test)]
fn with_install_path(error: ApplicationError, wasm_path: &std::path::Path) -> ApplicationError {
    if wasm_path.as_os_str().as_encoded_bytes().len() > MAX_PLUGIN_INSTALL_PATH_BYTES {
        return error;
    }
    error.with_path(wasm_path)
}

/// Structural validation of an install request, before any file is opened.
///
/// Deliberately *not* an existence check: whether the file is there is
/// the filesystem's answer at open time, and asking here would only add a
/// check the install then races (the file can vanish, or appear, in
/// between). What this rejects are the things that are wrong about the
/// *request* regardless of what is on disk -- an empty path, an absurdly
/// long one, and one that does not name a `.wasm` file at all. The last
/// duplicates a check `PluginManager::install_plugin` also makes, on
/// purpose: the manager reports it as an untyped `LoadError` string among
/// a dozen others, and matching on that string to recover the distinction
/// would be far more fragile than re-deriving it from the path.
#[cfg(test)]
pub(crate) fn validate_install_path(wasm_path: &std::path::Path) -> Result<(), ApplicationError> {
    if wasm_path.as_os_str().is_empty() {
        return Err(invalid_install_path("a plugin path must not be empty"));
    }
    if wasm_path.as_os_str().as_encoded_bytes().len() > MAX_PLUGIN_INSTALL_PATH_BYTES {
        return Err(invalid_install_path(&format!(
            "a plugin path must not exceed {MAX_PLUGIN_INSTALL_PATH_BYTES} bytes"
        )));
    }
    if !wasm_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wasm"))
    {
        return Err(with_install_path(
            invalid_install_path("a plugin must be installed from a .wasm file"),
            wasm_path,
        ));
    }
    Ok(())
}

pub(crate) fn validate_package_path(
    package_path: &std::path::Path,
) -> Result<(), ApplicationError> {
    if package_path.as_os_str().is_empty() {
        return Err(invalid_package_path(
            "a plugin package path must not be empty",
        ));
    }
    if package_path.as_os_str().as_encoded_bytes().len() > MAX_PLUGIN_INSTALL_PATH_BYTES {
        return Err(invalid_package_path(&format!(
            "a plugin package path must not exceed {MAX_PLUGIN_INSTALL_PATH_BYTES} bytes"
        )));
    }
    if !package_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wirt"))
    {
        return Err(with_package_path(
            invalid_package_path("a plugin must be installed from a .wirt package"),
            package_path,
        ));
    }
    Ok(())
}

fn invalid_package_path(reason: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "plugin package cannot be installed",
    )
    .with_diagnostic(reason.to_string())
    .with_recoverability(Recoverability::UserAction)
    .with_field("package_path")
}

fn with_package_path(error: ApplicationError, package_path: &std::path::Path) -> ApplicationError {
    if package_path.as_os_str().as_encoded_bytes().len() > MAX_PLUGIN_INSTALL_PATH_BYTES {
        error
    } else {
        error.with_path(package_path)
    }
}

fn plugin_package_error(error: PluginError) -> ApplicationError {
    let kind = match &error {
        PluginError::InvalidManifest(_) | PluginError::InvalidPackage(_) => {
            ApplicationErrorKind::InvalidInput
        }
        PluginError::Unsupported(_) => ApplicationErrorKind::Unsupported,
        PluginError::Conflict(_) => ApplicationErrorKind::Conflict,
        PluginError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            ApplicationErrorKind::PermissionDenied
        }
        PluginError::Io(_) => ApplicationErrorKind::Backend,
        _ => ApplicationErrorKind::Plugin,
    };
    ApplicationError::new(kind, "plugin package could not be processed")
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::UserAction)
        .with_field("package_path")
}

/// Maps a failed `PluginManager::install_plugin` onto the error envelope.
///
/// Classified by `PluginError` *variant* rather than by its message, so a
/// frontend can branch on invalid packages, unsupported ABIs, collisions,
/// permission failures, and backend I/O without parsing diagnostics.
///
/// The caller's own path is attached separately (see
/// [`with_install_path`]), because the diagnostic's redaction removes it.
#[cfg(test)]
fn plugin_install_error(error: PluginError) -> ApplicationError {
    let kind = match &error {
        // A manifest this plugin's own metadata export produced is
        // malformed -- the plugin file is the invalid input.
        PluginError::InvalidManifest(_) | PluginError::InvalidPackage(_) => {
            ApplicationErrorKind::InvalidInput
        }
        PluginError::Unsupported(_) => ApplicationErrorKind::Unsupported,
        PluginError::Conflict(_) => ApplicationErrorKind::Conflict,
        PluginError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            ApplicationErrorKind::PermissionDenied
        }
        PluginError::Io(_) => ApplicationErrorKind::Backend,
        PluginError::LoadError(_)
        | PluginError::InitError(_)
        | PluginError::ExecutionError(_)
        | PluginError::Unavailable(_)
        | PluginError::ResourceLimit { .. }
        | PluginError::CapabilityDenied(_)
        | PluginError::NotFound(_)
        | PluginError::WasmError(_)
        | PluginError::Serialization(_)
        | PluginError::TomlError(_) => ApplicationErrorKind::Plugin,
    };
    ApplicationError::new(kind, "plugin could not be installed")
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::UserAction)
        .with_field("wasm_path")
}

/// Maximum byte length of a `Dialog`/`Page` extension point's id string.
/// Matches the bound the pre-facade egui `PluginUiJobs` queue enforced on
/// the analogous `page_id`/`plugin_id` request fields before rejecting a
/// request outright, rather than letting an unbounded guest-adjacent
/// string reach a `HashMap` key or a WASM host-function call.
const MAX_EXTENSION_POINT_ID_BYTES: usize = 512;

fn invalid_extension_point(reason: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "invalid plugin extension point",
    )
    .with_diagnostic(reason.to_string())
    .with_recoverability(Recoverability::UserAction)
    .with_field("extension_point")
}

/// Structural validation for the extension point `open_plugin_session`
/// was asked to open. `MainPage`/`PluginButton`/`Panel` carry no
/// caller-supplied data and are always valid. `Dialog`/`Page` carry an
/// open-ended id a plugin defines; this facade cannot know in advance
/// which ids a given plugin actually recognizes (that's determined by
/// whatever `get-ui-layout` returns for it, empty or not), but it *can*
/// reject a structurally malformed one -- empty, or absurdly long --
/// before ever reaching the plugin manager or its WASM guest.
fn validate_extension_point(
    extension_point: &PluginExtensionPointDto,
) -> Result<(), ApplicationError> {
    let id = match extension_point {
        PluginExtensionPointDto::Dialog(id) | PluginExtensionPointDto::Page(id) => id,
        PluginExtensionPointDto::MainPage
        | PluginExtensionPointDto::PluginButton
        | PluginExtensionPointDto::Panel => return Ok(()),
    };
    if id.is_empty() {
        return Err(invalid_extension_point(
            "a dialog/page extension point id must not be empty",
        ));
    }
    if id.len() > MAX_EXTENSION_POINT_ID_BYTES {
        return Err(invalid_extension_point(&format!(
            "a dialog/page extension point id must not exceed {MAX_EXTENSION_POINT_ID_BYTES} bytes"
        )));
    }
    Ok(())
}

fn action_rejected(node_id: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "target node is hidden or disabled",
    )
    .with_diagnostic(format!("node id: {node_id}"))
    .with_recoverability(Recoverability::UserAction)
    .with_field("node_id")
}

// ============================================================================
// Session store
// ============================================================================

struct SessionRecord {
    plugin_id: String,
    extension_point: PluginExtensionPointDto,
    region_id: String,
    revision: u64,
    root: PluginUiNodeDto,
    /// Which archive session was active when this plugin session opened,
    /// if any -- see [`PluginSessionStore::open`]'s doc comment.
    pinned_archive_session: Option<ArchiveSessionId>,
}

impl SessionRecord {
    fn document(&self, session_id: PluginSessionId) -> PluginUiDocument {
        PluginUiDocument {
            session_id,
            plugin_id: self.plugin_id.clone(),
            region_id: self.region_id.clone(),
            extension_point: self.extension_point.clone(),
            revision: self.revision,
            root: self.root.clone(),
        }
    }
}

/// The registry every open plugin session lives in, plus the per-plugin
/// serialization lock every action dispatch acquires. Mints
/// [`PluginSessionId`]s from its own counter (matching `ArchiveSessionStore`'s
/// per-instance-counter pattern, not a process-wide `static`).
///
/// # What a disable does to a session that is already open
///
/// **The session record survives; every plugin-driven operation on it
/// refuses for as long as the plugin stays disabled; re-enabling resumes
/// it exactly where it was.** [`Self::open`], [`Self::document`] and
/// [`Self::dispatch_action`] each enforce their half through
/// [`require_enabled_plugin`]; this is the one place that states the
/// policy whole.
///
/// Why the session is *not* destroyed at disable time:
/// - The plugin runtime does not destroy anything either. `PluginManager::
///   disable_plugin` flips a flag; the guest instance, its settings and its
///   network log all survive. A facade that reaped sessions would impose a
///   shorter lifetime than the runtime it wraps, and only on the host's
///   half of the state -- the guest would go on remembering what the host
///   had thrown away.
/// - A frontend closes its own slots. [`Self::close`] on a session the
///   facade had silently reaped would start reporting `NotFound` for
///   routine teardown, turning correct cleanup into an error path.
/// - A reaped session is indistinguishable from a bogus session id, so a
///   panel could not tell "your plugin was turned off" (draw nothing, and
///   maybe draw again later) from "your session id is garbage" (a bug
///   worth surfacing). Keeping the record is what makes
///   [`is_plugin_disabled_refusal`] answerable at all.
///
/// What that implies for in-flight state, each pinned by a test:
/// - The retained document is *withheld, not discarded*: no new revision is
///   minted while disabled, and the same revision is served again on
///   re-enable.
/// - A guest call already executing when the disable lands cannot be
///   preempted -- a WASM call is not cancellable. It runs to completion,
///   but the flag is re-checked at every boundary after it, so **once
///   `set_plugin_enabled(.., false)` has returned, no further guest call
///   is made for that plugin and nothing is published on its behalf**: no
///   host intents, no `RequestFetch`, no `RefreshPanel` re-entry, no
///   document. A dispatch caught this way ends `Failed` carrying the
///   refusal; an open caught this way registers no session. The boundaries
///   are enumerated on [`Self::dispatch_action`], and there is more than
///   one on purpose -- a single post-guest check leaves the seconds of
///   network I/O that follow it unguarded.
///   The residual, stated rather than hidden: whatever that one
///   already-issued guest call did *inside* the guest (a setting it wrote)
///   stands.
/// - Nothing re-fetches on re-enable. Enabling a plugin makes no plugin
///   call, so the session resumes on its last document; the next dispatch
///   refreshes it if the plugin asks.
pub(crate) struct PluginSessionStore {
    sessions: SyncRwLock<HashMap<PluginSessionId, SessionRecord>>,
    next_id: AtomicU64,
    per_plugin_locks: SyncMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl PluginSessionStore {
    pub(crate) fn new() -> Self {
        Self {
            sessions: SyncRwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            per_plugin_locks: SyncMutex::new(HashMap::new()),
        }
    }

    fn lock_for_plugin(&self, plugin_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self.per_plugin_locks.lock();
        locks
            .entry(plugin_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Every plugin `PluginManager` knows about, successfully loaded or
    /// not (see `arclain_plugins::manager::FailedPlugin`).
    pub(crate) fn plugins(
        manager: &SyncMutex<PluginManager>,
        persisted_visibility: Option<&str>,
    ) -> Vec<PluginSummary> {
        let visibility: HashMap<String, BTreeMap<String, bool>> =
            serde_json::from_str(persisted_visibility.unwrap_or("{}")).unwrap_or_default();
        let manager = manager.lock();
        let mut summaries: Vec<PluginSummary> = manager
            .list_plugins()
            .into_iter()
            .map(|item| {
                let (quarantine_state, last_reason) = match item.quarantine_state {
                    arclain_plugins::QuarantineState::Clear => (PluginQuarantineState::Clear, None),
                    arclain_plugins::QuarantineState::Retryable(record) => (
                        PluginQuarantineState::Retryable {
                            failed_retries: record.failed_retries,
                        },
                        Some(record.last_reason),
                    ),
                    arclain_plugins::QuarantineState::PersistentlyDisabled(record) => (
                        PluginQuarantineState::PersistentlyDisabled {
                            failed_retries: record.failed_retries,
                        },
                        Some(record.last_reason),
                    ),
                };
                let plugin_visibility = visibility.get(&item.id).cloned().unwrap_or_default();
                let capabilities = item
                    .manifest
                    .capabilities
                    .to_capabilities()
                    .into_iter()
                    .map(PluginCapabilityDto::from)
                    .collect();
                PluginSummary {
                    id: item.id,
                    name: item.manifest.plugin.name,
                    version: item.manifest.plugin.version,
                    author: item.manifest.plugin.author,
                    description: item.manifest.plugin.description,
                    capabilities,
                    visibility: plugin_visibility,
                    enabled: item.enabled,
                    load_error: None,
                    quarantine_state,
                    last_reason,
                }
            })
            .collect();
        summaries.extend(manager.failed_plugins().into_iter().map(|failed| {
            let plugin_visibility = visibility
                .get(&failed.original_id)
                .cloned()
                .unwrap_or_default();
            PluginSummary {
                id: failed.original_id,
                name: String::new(),
                version: String::new(),
                author: String::new(),
                description: String::new(),
                capabilities: Vec::new(),
                visibility: plugin_visibility,
                enabled: false,
                load_error: Some(failed.error),
                quarantine_state: PluginQuarantineState::Clear,
                last_reason: None,
            }
        }));
        summaries
    }

    pub(crate) fn set_plugin_enabled(
        manager: &SyncMutex<PluginManager>,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<(), ApplicationError> {
        let manager = manager.lock();
        let result = if enabled {
            manager.enable_plugin(plugin_id)
        } else {
            manager.disable_plugin(plugin_id)
        };
        result.map_err(|error| match error {
            PluginError::NotFound(_) => plugin_not_found(plugin_id),
            error => plugin_execution_error(error),
        })
    }

    pub(crate) fn retry_plugin(
        manager: &SyncMutex<PluginManager>,
        plugin_id: &str,
    ) -> Result<(), ApplicationError> {
        manager
            .lock()
            .retry_plugin(plugin_id)
            .map_err(plugin_execution_error)
    }

    pub(crate) fn reset_plugin_quarantine(
        manager: &SyncMutex<PluginManager>,
        plugin_id: &str,
    ) -> Result<(), ApplicationError> {
        manager
            .lock()
            .reset_plugin_quarantine(plugin_id)
            .map_err(plugin_execution_error)
    }

    pub(crate) fn inspect_plugin_package(
        manager: &SyncMutex<PluginManager>,
        package_path: &std::path::Path,
    ) -> Result<PluginInstallPreviewDto, ApplicationError> {
        manager
            .lock()
            .inspect_plugin_package(package_path)
            .map(PluginInstallPreviewDto::from)
            .map_err(|error| with_package_path(plugin_package_error(error), package_path))
    }

    pub(crate) fn install_plugin_package(
        manager: &SyncMutex<PluginManager>,
        package_path: &std::path::Path,
        expected: &wirt::PackageFingerprint,
    ) -> Result<String, ApplicationError> {
        manager
            .lock()
            .install_plugin_package(package_path, expected)
            .map_err(|error| with_package_path(plugin_package_error(error), package_path))
    }

    /// Backs [`crate::ArclainApp::plugin_chrome`]. Blocking: reading the
    /// top tabs calls into every enabled plugin's WASM and waits on each
    /// instance's lock, so every caller must already be on a
    /// blocking-pool thread.
    ///
    /// Takes the counts and the instance snapshot under one manager lock,
    /// then *drops* that lock before calling into any plugin -- the
    /// pattern `EnabledPluginSnapshot`'s own doc comment prescribes, and
    /// the reason this does not simply call `PluginManager::
    /// get_all_top_tabs` (which would hold the manager lock across every
    /// plugin call, stalling every other plugin operation behind one slow
    /// guest).
    ///
    /// Bypassing `PluginManager::get_all_top_tabs` also bypasses its
    /// memoized result, and that is deliberate: that cache is only
    /// invalidated on enable/disable/load, so a badge count -- live data a
    /// plugin updates as work completes -- would freeze at whatever it
    /// was when the last plugin was toggled. This read model exists to
    /// serve the *live* strip. The cost is one WASM call per enabled
    /// plugin per call, so the caller owns the cadence.
    pub(crate) fn plugin_chrome(manager: &SyncMutex<PluginManager>) -> PluginChromeSnapshot {
        let (summary, instances) = {
            let manager = manager.lock();
            (manager.status_summary(), manager.enabled_plugin_snapshot())
        };
        PluginChromeSnapshot {
            summary: summary.into(),
            top_tabs: instances
                .get_all_top_tabs()
                .into_iter()
                .map(PluginTopTabDto::from)
                .collect(),
        }
    }

    /// Backs [`crate::ArclainApp::plugin_network_log`]. Blocking for the
    /// same reason [`Self::plugin_chrome`] is -- it waits on every enabled
    /// plugin's instance lock -- and drops the manager lock before doing
    /// so for the same reason.
    pub(crate) fn plugin_network_log(
        manager: &SyncMutex<PluginManager>,
    ) -> Vec<PluginNetworkLogEntryDto> {
        let instances = { manager.lock().enabled_plugin_snapshot() };
        instances
            .get_network_log()
            .into_iter()
            .map(PluginNetworkLogEntryDto::from)
            .collect()
    }

    /// Opens a fresh session for `plugin_id`'s requested `extension_point`
    /// -- `MainPage`/`Panel`/`PluginButton`/`Dialog(id)`/`Page(id)`, any
    /// of the five current WIT extension points. Rejects a structurally
    /// invalid `Dialog`/`Page` id (see [`validate_extension_point`])
    /// before ever reaching the plugin manager. Runs the WASM
    /// `get-ui-layout` call on a blocking-pool thread via `handle`, never
    /// on the caller's async task.
    ///
    /// A structurally valid id the plugin does not implement still opens
    /// successfully with an empty document -- see
    /// `ArclainApp::open_plugin_session`'s own doc comment for why that
    /// is indistinguishable from a real empty layout, and not a bug.
    ///
    /// Refuses a disabled plugin (see [`require_enabled_plugin`]) both
    /// before and after the WASM call: opening a session *is* running the
    /// plugin, because `get-ui-layout` executes in the guest, and a
    /// disable landing while that call runs must not leave a live session
    /// behind. The first check sits after [`validate_extension_point`]
    /// because a malformed request is malformed whatever the plugin's
    /// state is.
    ///
    /// `pinned_archive_session` is whichever archive session was active at
    /// the moment this session opened, and is where a background metadata
    /// fetch this session later requests writes its result. A plugin UI
    /// slot is opened *for* the archive the user is looking at (a panel is
    /// declared by that archive's browser), so open time is when the
    /// origin is unambiguous; completion time is not, because a fetch can
    /// take seconds and the user may switch archives while it runs. The
    /// event-triggered fetch path pins its own origin for exactly the same
    /// reason (see `arclain_plugins::manager::dispatch`'s event context).
    /// `None` -- no archive open at the time, as for a `MainPage` session
    /// in the plugin settings page -- falls back to whichever session is
    /// active at completion, which is the best available answer when the
    /// session never had an origin of its own.
    pub(crate) async fn open(
        &self,
        manager: Arc<SyncMutex<PluginManager>>,
        plugin_id: String,
        extension_point: PluginExtensionPointDto,
        pinned_archive_session: Option<ArchiveSessionId>,
        handle: &tokio::runtime::Handle,
    ) -> Result<PluginSessionSnapshot, ApplicationError> {
        validate_extension_point(&extension_point)?;
        require_enabled_plugin(&manager, &plugin_id)?;
        let root = self
            .fetch_and_normalize(&manager, &plugin_id, &extension_point, handle)
            .await?;
        // The same containment [`Self::dispatch_action`] applies after its
        // own guest call: `get-ui-layout` is not preemptible, so a disable
        // landing while it ran cannot stop it -- but the session it would
        // have produced is never registered and its layout is never
        // returned. Without this, disabling a plugin during the frame that
        // opens its panel leaves a live session behind.
        require_enabled_plugin(&manager, &plugin_id)?;

        let id = PluginSessionId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed));
        let record = SessionRecord {
            plugin_id: plugin_id.clone(),
            region_id: extension_point.region_slug(),
            extension_point,
            revision: 1,
            root,
            pinned_archive_session,
        };
        let document = record.document(id);
        self.sessions.write().insert(id, record);
        Ok(PluginSessionSnapshot {
            session_id: id,
            document,
        })
    }

    /// Immediate query of the last document revision retained for
    /// `session_id` -- no plugin call.
    ///
    /// Gated on the enabled flag even though nothing executes here: the
    /// retained document is the disabled plugin's own authored content,
    /// and serving it would leave that plugin's panel on screen after the
    /// user turned it off -- the visible half of "a disabled plugin still
    /// runs". The record itself is kept, not dropped (see
    /// [`PluginSessionStore`]'s own doc comment), so the document comes
    /// back verbatim, at the same revision, if the plugin is re-enabled.
    ///
    /// Order is forced rather than chosen: the session has to resolve
    /// before there is a plugin id to check, so an unknown session id is
    /// `NotFound` regardless of any plugin's state.
    pub(crate) fn document(
        &self,
        manager: &SyncMutex<PluginManager>,
        session_id: PluginSessionId,
    ) -> Result<PluginUiDocument, ApplicationError> {
        let (plugin_id, document) = {
            let sessions = self.sessions.read();
            let record = sessions
                .get(&session_id)
                .ok_or_else(|| unknown_plugin_session(session_id))?;
            (record.plugin_id.clone(), record.document(session_id))
        };
        require_enabled_plugin(manager, &plugin_id)?;
        Ok(document)
    }

    /// Which plugin owns `session_id`, or `None` if the session is
    /// unknown or already closed.
    ///
    /// Read *before* a dispatch rather than after, by the caller that has
    /// to persist that plugin's settings once the dispatch returns: a
    /// dispatch can end with the session gone (closed from another task
    /// while the guest ran), and a guest that wrote a setting on its way
    /// to that outcome still wrote it.
    pub(crate) fn session_plugin_id(&self, session_id: PluginSessionId) -> Option<String> {
        self.sessions
            .read()
            .get(&session_id)
            .map(|record| record.plugin_id.clone())
    }

    /// The archive session this plugin session's background metadata
    /// writes are pinned to -- see [`Self::open`].
    ///
    /// Exposed so a caller can verify the origin it named was actually
    /// recorded. Nothing in the rendering path needs it: the pin is only
    /// ever read internally, by `dispatch_action`.
    pub(crate) fn pinned_archive_session(
        &self,
        session_id: PluginSessionId,
    ) -> Result<Option<ArchiveSessionId>, ApplicationError> {
        self.sessions
            .read()
            .get(&session_id)
            .map(|record| record.pinned_archive_session)
            .ok_or_else(|| unknown_plugin_session(session_id))
    }

    pub(crate) fn close(&self, session_id: PluginSessionId) -> Result<(), ApplicationError> {
        self.sessions
            .write()
            .remove(&session_id)
            .map(|_| ())
            .ok_or_else(|| unknown_plugin_session(session_id))
    }

    /// Dispatches one [`PluginActionRequest`] against its session's
    /// plugin, serialized per plugin id (see the module doc comment),
    /// and returns the resulting [`PluginUiUpdate`]: an updated document
    /// (re-normalized, and -- if the plugin requested it via
    /// `RefreshPanel` -- re-fetched) plus every bounded host intent the
    /// plugin's response produced.
    ///
    /// Rejects (without ever reaching the WASM guest) an action whose
    /// `node_id` names a node in the *current* document that is
    /// `!visible || !enabled`. A `node_id` that does not name any node in
    /// the tree (a toolbar button's own id, or an internal lifecycle
    /// event) is dispatched normally -- see
    /// `wirt::ui_model::PluginUiNodeDto::find`'s doc comment.
    ///
    /// The enabled flag is re-evaluated at every boundary where this
    /// dispatch is about to act on the plugin's behalf again: before the
    /// per-plugin queue, after it, the moment the guest call returns,
    /// before the refresh re-enters the guest, and before the resulting
    /// document is published. They are not redundant -- each guards a
    /// different window, and every window between them is one a user has
    /// time to click a toggle in (a queue wait behind another action's
    /// guest call; the guest call itself; seconds of `RequestFetch`
    /// network I/O; a second guest call to re-fetch the layout).
    ///
    /// What that buys, exactly: **once `set_plugin_enabled(.., false)` has
    /// returned, this dispatch makes no further call into that plugin's
    /// guest and publishes nothing on its behalf** -- no host intents, no
    /// metadata fetch, no document revision. The one thing no check can
    /// undo is a guest call already executing when the disable landed; it
    /// runs to completion inside the guest, and only its results are
    /// discarded.
    pub(crate) async fn dispatch_action(
        &self,
        manager: Arc<SyncMutex<PluginManager>>,
        request: PluginActionRequest,
        handle: &tokio::runtime::Handle,
    ) -> Result<PluginUiUpdate, ApplicationError> {
        let (plugin_id, extension_point, pinned_archive_session, force_page_init_refresh) = {
            let sessions = self.sessions.read();
            let record = sessions
                .get(&request.session_id)
                .ok_or_else(|| unknown_plugin_session(request.session_id))?;
            if let Some(target) = record.root.find(&request.node_id) {
                if !target.visible || !target.enabled {
                    return Err(action_rejected(&request.node_id));
                }
            }
            // A page session necessarily opens by reading its layout once,
            // before the frontend can dispatch the page's internal
            // `__page_init` lifecycle event. That event may mutate guest
            // state without returning `RefreshPanel`; publishing the
            // already-read root would therefore let the frontend cache a
            // pre-init document indefinitely. A matching page init always
            // receives one fresh read after the guest call, so the action's
            // returned document is the first one safe to draw.
            let force_page_init_refresh = matches!(
                (&record.extension_point, &request.action),
                (
                    PluginExtensionPointDto::Page(page_id),
                    PluginActionDto::SetValue {
                        value: Some(init_page_id),
                    },
                ) if request.node_id == "__page_init" && init_page_id == page_id
            );
            (
                record.plugin_id.clone(),
                record.extension_point.clone(),
                record.pinned_archive_session,
                force_page_init_refresh,
            )
        };
        // Re-evaluated at every boundary below where this dispatch is
        // about to act on the plugin's behalf again. Named once so the
        // repetition reads as the deliberate pattern it is rather than as
        // a check someone forgot to hoist: each call site guards a
        // *different* window, and the windows are wide -- a queue wait, a
        // guest call, seconds of network I/O.
        let still_enabled = || require_enabled_plugin(&manager, &plugin_id);

        still_enabled()?;

        let plugin_lock = self.lock_for_plugin(&plugin_id);
        let _guard = plugin_lock.lock().await;
        // The queue wait: the per-plugin lock can be held for as long as
        // another action's guest call takes, and the plugin can be
        // disabled while this dispatch waits its turn behind it.
        still_enabled()?;

        let event_id = request.node_id.clone();
        let value = match request.action {
            PluginActionDto::Activate => None,
            PluginActionDto::SetValue { value } => value,
        };
        let dispatch_manager = manager.clone();
        let dispatch_plugin_id = plugin_id.clone();
        let actions = handle
            .spawn_blocking(move || {
                let executor = dispatch_manager.lock().wirt_executor();
                let plugin_id = wirt::PluginId::parse(dispatch_plugin_id)?;
                wirt::WirtExecutor::execute(
                    executor.as_ref(),
                    &plugin_id,
                    wirt::ExecutorRequest::UiEvent {
                        id: event_id,
                        value,
                    },
                )
                .and_then(wirt::ExecutorResponse::into_actions)
            })
            .await
            .map_err(|join_error| {
                ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "plugin action worker failed",
                )
                .with_diagnostic(join_error.to_string())
            })?;
        let actions = match actions {
            Ok(actions) => actions,
            Err(error) => {
                still_enabled()?;
                return Err(plugin_execution_error(error));
            }
        };

        // The guest call could not be taken back; what it produced can be,
        // and is. Nothing below may happen for a plugin disabled while it
        // ran: no metadata fetch on its network budget, no re-entry into
        // its guest, no intents handed to a frontend, no new revision.
        still_enabled()?;

        let bounded = wirt::action_policy::bound_plugin_actions(actions);
        let mut intents = Vec::with_capacity(bounded.len());
        let mut outcome = BoundedActionOutcome::default();
        for action in bounded {
            apply_bounded_action(action, &plugin_id, &mut intents, &mut outcome);
        }

        // Resolved before the refresh below, so a fetch that lands
        // synchronously (a gameta cache hit) is already reflected in the
        // layout a `RefreshPanel` in the same response re-fetches.
        if !outcome.fetch_keys.is_empty() {
            resolve_request_fetches(
                &manager,
                &plugin_id,
                outcome.fetch_keys,
                pinned_archive_session,
                handle,
            )
            .await;
        }

        // The widest window of the whole dispatch: a `RequestFetch` is a
        // network round trip that can take seconds, and the refresh below
        // re-enters the guest. A check before the guest call is worth
        // nothing if the seconds spent between them are unguarded.
        still_enabled()?;

        let refreshed_root = if outcome.needs_refresh || force_page_init_refresh {
            Some(
                self.fetch_and_normalize(&manager, &plugin_id, &extension_point, handle)
                    .await?,
            )
        } else {
            None
        };

        // Last boundary: publishing. The refresh above is a guest call of
        // its own, so a disable landing during it gets the same treatment
        // the first one does -- its layout is dropped rather than
        // committed, and the session's revision does not move.
        still_enabled()?;

        let document = {
            let mut sessions = self.sessions.write();
            let record = sessions
                .get_mut(&request.session_id)
                .ok_or_else(|| unknown_plugin_session(request.session_id))?;
            if let Some(root) = refreshed_root {
                record.root = root;
            }
            record.revision += 1;
            record.document(request.session_id)
        };

        Ok(PluginUiUpdate { document, intents })
    }

    async fn fetch_and_normalize(
        &self,
        manager: &Arc<SyncMutex<PluginManager>>,
        plugin_id: &str,
        extension_point: &PluginExtensionPointDto,
        handle: &tokio::runtime::Handle,
    ) -> Result<PluginUiNodeDto, ApplicationError> {
        let worker_manager = manager.clone();
        let worker_plugin_id = plugin_id.to_string();
        let host_extension_point = to_host_extension_point(extension_point);
        let layout = handle
            .spawn_blocking(move || {
                let executor = worker_manager.lock().wirt_executor();
                let plugin_id = wirt::PluginId::parse(worker_plugin_id)?;
                wirt::WirtExecutor::execute(
                    executor.as_ref(),
                    &plugin_id,
                    wirt::ExecutorRequest::UiLayout {
                        extension_point: host_extension_point,
                    },
                )
                .and_then(wirt::ExecutorResponse::into_layout)
            })
            .await
            .map_err(|join_error| {
                ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "plugin layout worker failed",
                )
                .with_diagnostic(join_error.to_string())
            })?;
        let layout = match layout {
            Ok(layout) => layout,
            Err(error) => {
                require_enabled_plugin(manager, plugin_id)?;
                return Err(plugin_execution_error(error));
            }
        };
        let root = ui_model::normalize_layout(&layout).map_err(normalize_error)?;
        Ok(rewrite_cache_keys(root, plugin_id))
    }
}

fn to_host_extension_point(
    extension_point: &PluginExtensionPointDto,
) -> wirt::PluginExtensionPoint {
    use wirt::PluginExtensionPoint as HostExtensionPoint;
    match extension_point {
        PluginExtensionPointDto::MainPage => HostExtensionPoint::MainPage,
        PluginExtensionPointDto::PluginButton => HostExtensionPoint::PluginButton,
        PluginExtensionPointDto::Panel => HostExtensionPoint::Panel,
        PluginExtensionPointDto::Dialog(id) => HostExtensionPoint::Dialog(id.clone()),
        PluginExtensionPointDto::Page(id) => HostExtensionPoint::Page(id.clone()),
    }
}

/// What one dispatch's batch of bounded actions asked this layer to do,
/// beyond the [`PluginHostIntentDto`]s it produced for the renderer.
#[derive(Default)]
struct BoundedActionOutcome {
    /// Set by any number of `RefreshPanel` actions; the caller re-fetches
    /// the layout exactly once regardless of how many arrived.
    needs_refresh: bool,
    /// Every `RequestFetch` key, in the order the plugin emitted them.
    /// `action_policy` already caps how many survive one response.
    fetch_keys: Vec<String>,
}

/// Runs each `RequestFetch` key through
/// `arclain_plugins::resolve_interactive_request_fetch`, the same routing
/// policy the event-triggered path uses (capability gate, per-plugin
/// network permit, gameta-then-native ordering, payload size cap).
///
/// Each key runs on a blocking-pool thread: the gameta round trip is
/// synchronous and can take seconds. The plugin manager's lock is taken
/// only long enough to resolve the instance, never held across the
/// network call -- otherwise one plugin's slow fetch would stall every
/// other plugin operation in the application.
///
/// Failures are logged rather than failing the dispatch: a metadata fetch
/// that could not be satisfied is not a reason to discard the plugin's
/// updated document and the intents that came with it.
async fn resolve_request_fetches(
    manager: &Arc<SyncMutex<PluginManager>>,
    plugin_id: &str,
    keys: Vec<String>,
    pinned_archive_session: Option<ArchiveSessionId>,
    handle: &tokio::runtime::Handle,
) {
    let (instance, executor) = {
        let manager = manager.lock();
        let Some(instance) = manager.get_plugin_instance(plugin_id) else {
            tracing::warn!(plugin_id, "dropped a RequestFetch for an unknown plugin");
            return;
        };
        (instance, manager.wirt_executor())
    };
    let pinned = pinned_archive_session.map(ArchiveSessionId::into_raw);
    for key in keys {
        let instance = instance.clone();
        let executor = executor.clone();
        let worker_plugin_id = plugin_id.to_string();
        let outcome = handle
            .spawn_blocking(move || {
                arclain_plugins::resolve_interactive_request_fetch(
                    &executor,
                    &instance,
                    &worker_plugin_id,
                    &key,
                    pinned,
                )
            })
            .await;
        match outcome {
            Ok(outcome) => tracing::debug!(plugin_id, ?outcome, "resolved a plugin RequestFetch"),
            Err(error) => tracing::warn!(
                plugin_id,
                %error,
                "a plugin RequestFetch worker failed"
            ),
        }
    }
}

/// Applies one bounded (`wirt::action_policy`-passed)
/// `PluginAction` to this dispatch's outcome: either a
/// [`PluginHostIntentDto`] a renderer should react to, or a host-internal
/// signal this layer resolves itself.
///
/// `RefreshPanel` sets `needs_refresh` so the caller re-fetches the
/// layout once, after every bounded action in the batch has been
/// examined -- not once per `RefreshPanel` action, since a plugin
/// returning several in one response should still trigger exactly one
/// re-fetch.
///
/// `RequestFetch` is collected rather than resolved inline: resolving it
/// needs the plugin manager and a blocking thread, neither of which this
/// pure function has, and the caller must run every key *before* the
/// refresh so a synchronously-satisfied fetch is already visible in the
/// re-fetched layout.
fn apply_bounded_action(
    action: wirt::PluginAction,
    plugin_id: &str,
    intents: &mut Vec<PluginHostIntentDto>,
    outcome: &mut BoundedActionOutcome,
) {
    use wirt::PluginAction;

    match action {
        PluginAction::None => {}
        PluginAction::ShowToast { message, level } => {
            intents.push(PluginHostIntentDto::ShowToast {
                message,
                level: ui_model::convert_toast_level(level),
            });
        }
        PluginAction::RefreshPanel { .. } => {
            outcome.needs_refresh = true;
        }
        PluginAction::CloseDialog => intents.push(PluginHostIntentDto::CloseDialog),
        PluginAction::CopyToClipboard { text } => {
            intents.push(PluginHostIntentDto::CopyToClipboard { text });
        }
        PluginAction::OpenLightbox {
            images,
            start_index,
            title,
        } => {
            intents.push(PluginHostIntentDto::OpenLightbox {
                // Encoded the same way `rewrite_cache_keys` encodes every
                // other image-bearing node (Image/Carousel/ListItem) at
                // normalization time: a lightbox image reference is just
                // as plugin-namespaced as those, and `read_plugin_image`
                // only ever accepts the encoded form (see
                // `decode_plugin_image_cache_key`). Left raw, every
                // lightbox image would fail to resolve with `NotFound`.
                images: images
                    .into_iter()
                    .map(|(cache_key, url)| PluginImageDto {
                        cache_key: encode_plugin_image_cache_key(plugin_id, &cache_key),
                        url,
                    })
                    .collect(),
                start_index: start_index as u64,
                title,
            });
        }
        PluginAction::SetPageDisplayName { name } => {
            intents.push(PluginHostIntentDto::SetPageDisplayName { name });
        }
        PluginAction::RequestFetch { key } => {
            outcome.fetch_keys.push(key);
        }
    }
}

// ============================================================================
// Active-session tracking + `ActiveTabBridge` / `PluginArchiveAccess`
// adapters.
// ============================================================================

/// Shared "which archive session is currently active" state:
/// [`crate::ArclainApp::set_active_archive_session`] writes it, and
/// [`ArchiveContextBridge`]/[`ArchiveContextAccess`] read it. A plain
/// `RwLock<Option<ArchiveSessionId>>` behind an `Arc` -- cheap to read
/// from a WASM host-function call (synchronous, briefly-held lock), and
/// cheap to clone into every adapter that needs it.
#[derive(Clone, Default)]
pub(crate) struct ActiveArchiveSession(Arc<SyncRwLock<Option<ArchiveSessionId>>>);

impl ActiveArchiveSession {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set(&self, session_id: Option<ArchiveSessionId>) {
        *self.0.write() = session_id;
    }

    /// Read by [`ArchiveContextBridge`] on every `ActiveTabBridge` call --
    /// see that type's own doc comment.
    pub(crate) fn get(&self) -> Option<ArchiveSessionId> {
        *self.0.read()
    }
}

/// This crate's own `arclain_plugins::ActiveTabBridge` implementation,
/// backed by [`ActiveArchiveSession`] and `crate::archive::ArchiveSessionStore`
/// instead of a UI signal tree. See the module doc comment for how this
/// composes with [`ProductionActiveTabBridge`] and
/// [`crate::ArclainApp::active_tab_bridge`] into the bridge a frontend
/// installs on `PluginManager`.
pub(crate) struct ArchiveContextBridge {
    active_session: ActiveArchiveSession,
    sessions: Arc<crate::archive::ArchiveSessionStore>,
}

impl ArchiveContextBridge {
    pub(crate) fn new(
        active_session: ActiveArchiveSession,
        sessions: Arc<crate::archive::ArchiveSessionStore>,
    ) -> Self {
        Self {
            active_session,
            sessions,
        }
    }

    /// Blocking (short) read of the active session's snapshot, or `None`
    /// if no session is active or it no longer exists. Every
    /// `ActiveTabBridge` method is synchronous (called from a WASM host-
    /// function context), and `crate::archive::ArchiveSessionStore::
    /// get_sync` is genuinely synchronous too -- a `parking_lot::RwLock`
    /// lookup, not an async one -- so this needs no runtime, no `Handle`,
    /// and no `block_in_place`/`block_on` dance. It works identically from
    /// every one of this bridge's actual production callers:
    /// `PluginSessionStore`'s `spawn_blocking` calls (panel/button-
    /// triggered plugin actions) and `arclain_plugins::PluginManager`'s
    /// event-dispatch worker (a bare `std::thread::spawn` thread with no
    /// Tokio context at all, which calls a plugin's `OnArchiveOpen`
    /// handler -- and, through it, both of the primary metadata-write
    /// sinks: `arclain_plugins::manager::dispatch::set_event_session_metadata`
    /// and the event-context branch of `crate::host_functions::metadata`'s
    /// `emit_metadata`). An earlier version of this method bridged to the
    /// store's since-removed `tokio::sync::RwLock` via exactly that
    /// `Handle`/`block_in_place` dance, which silently no-op'd every write
    /// from the event-dispatch worker specifically -- see
    /// [`tests::archive_context_bridge_writes_land_from_a_bare_thread_with_no_tokio_context`]
    /// for the regression test that now proves the opposite.
    fn with_active_session<T>(
        &self,
        read: impl FnOnce(&crate::archive::ArchiveSession) -> T,
    ) -> Option<T> {
        let session_id = self.active_session.get()?;
        self.with_session(session_id, read)
    }

    /// Same bridging as [`Self::with_active_session`], but resolving an
    /// explicit `session_id` rather than "whichever is active" -- used by
    /// `set_session_metadata`, which must write to the tab that
    /// originated an event even if the user has since switched tabs.
    fn with_session<T>(
        &self,
        session_id: ArchiveSessionId,
        read: impl FnOnce(&crate::archive::ArchiveSession) -> T,
    ) -> Option<T> {
        let session = self.sessions.get_sync(session_id).ok()?;
        Some(read(&session))
    }

    /// Publishes `SessionEvent::MetadataChanged` for `session_id`, but
    /// only when `committed` is `Some` -- i.e. only when the write this
    /// call announces actually landed in the session store. `with_session`/
    /// `with_active_session` return `None` for an unresolvable session
    /// (an unknown or already-closed session id, or no active session at
    /// all), and neither is "something changed" -- publishing anyway would
    /// tell a subscriber to go re-fetch a snapshot that never actually
    /// changed, for no reason. Resolution no longer depends on an ambient
    /// runtime: the session store's lock is genuinely synchronous, so
    /// every caller thread resolves identically (see
    /// [`ArchiveContextBridge::with_session`]).
    fn publish_if_committed<T>(&self, session_id: ArchiveSessionId, committed: Option<T>) {
        if committed.is_some() {
            self.sessions.publish_metadata_changed(session_id);
        }
    }
}

impl ActiveTabBridge for ArchiveContextBridge {
    fn archive_path(&self) -> Option<String> {
        self.with_active_session(|session| session.source_path().to_string_lossy().into_owned())
    }

    fn current_password(&self) -> Option<String> {
        // No production caller reads this: every `host_functions` call
        // site resolves password-dependent behavior some other way
        // (`ArchiveSession` does not expose the archive's password
        // directly), so this stays `None` rather than reaching into
        // `arclain_core::Archive` internals for a method nothing calls.
        None
    }

    fn archive_entries(&self) -> Vec<String> {
        self.with_active_session(|session| session.all_file_paths())
            .unwrap_or_default()
    }

    /// Overridden per `ActiveTabBridge::archive_entry_count`'s own doc
    /// comment ("without cloning their paths"): `ArchiveSession::
    /// file_count` is `O(1)`, unlike the trait's default (`self.
    /// archive_entries().len()`), which would materialize and clone every
    /// path in the archive just to count them.
    fn archive_entry_count(&self) -> usize {
        self.with_active_session(|session| session.file_count())
            .unwrap_or(0)
    }

    /// Overridden per `ActiveTabBridge::archive_entries_page`'s own doc
    /// comment: `ArchiveSession::file_paths_page` clones only the
    /// requested page, unlike the trait's default (`self.archive_entries()
    /// .skip(offset).take(limit)`), which would materialize and clone
    /// every path in the archive on every single page -- `O(n)` per call,
    /// `O(n^2)` for a full paged walk.
    fn archive_entries_page(&self, offset: usize, limit: usize) -> Vec<String> {
        self.with_active_session(|session| session.file_paths_page(offset, limit))
            .unwrap_or_default()
    }

    fn active_archive_session_id(&self) -> Option<u64> {
        self.active_session.get().map(ArchiveSessionId::into_raw)
    }

    fn set_session_metadata(&self, archive_session_id: u64, metadata: Option<serde_json::Value>) {
        let session_id = ArchiveSessionId::from_raw(archive_session_id);
        // No match (`None`) can mean the tab's session was already
        // closed, or this is a stale/fabricated id -- dropping the write
        // silently is correct, matching `ActiveTabBridge::
        // set_session_metadata`'s own doc comment. Only a write that
        // actually landed publishes -- see `Self::publish_if_committed`.
        self.publish_if_committed(
            session_id,
            self.with_session(session_id, |session| session.set_metadata(metadata)),
        );
    }

    fn set_active_tab_metadata(&self, metadata: Option<serde_json::Value>) {
        let Some(session_id) = self.active_session.get() else {
            // No archive session is active at all -- this bridge has no
            // notion of "the active tab" to fall back to (see the module
            // doc comment on where that fallback now lives instead).
            return;
        };
        self.publish_if_committed(
            session_id,
            self.with_session(session_id, |session| session.set_metadata(metadata)),
        );
    }

    fn set_archive_path(&self, path: Option<String>) {
        let Some(path) = path else {
            return;
        };
        let Some(session_id) = self.active_session.get() else {
            return;
        };
        self.publish_if_committed(
            session_id,
            self.with_session(session_id, |session| {
                session.set_source_path(std::path::PathBuf::from(path))
            }),
        );
    }
}

/// This crate's `arclain_plugins::types::PluginArchiveAccess`
/// implementation, backed by `crate::archive::ArchiveSessionStore`.
/// [`PluginArchiveContextId`] round-trips the same raw `u64` as this
/// crate's own `ArchiveSessionId` -- see that type's own doc comment for
/// why `arclain_plugins` cannot name `ArchiveSessionId` directly.
/// `#[allow(dead_code)]` for the same reason as `ArchiveContextBridge`.
#[allow(dead_code)]
pub(crate) struct ArchiveContextAccess {
    sessions: Arc<crate::archive::ArchiveSessionStore>,
}

#[allow(dead_code)]
impl ArchiveContextAccess {
    pub(crate) fn new(sessions: Arc<crate::archive::ArchiveSessionStore>) -> Self {
        Self { sessions }
    }

    fn resolve(
        &self,
        context_id: PluginArchiveContextId,
    ) -> Result<Arc<crate::archive::ArchiveSession>, PluginError> {
        let session_id = ArchiveSessionId::from_raw(context_id.into_raw());
        // Genuinely synchronous -- see `ArchiveContextBridge::with_session`'s
        // own doc comment for why this no longer needs a `Handle`/
        // `block_in_place` dance (and why that dance was a real bug, not
        // just unnecessary ceremony).
        self.sessions
            .get_sync(session_id)
            .map_err(|error| PluginError::NotFound(error.summary))
    }
}

impl PluginArchiveAccess for ArchiveContextAccess {
    fn list_entries(
        &self,
        context_id: PluginArchiveContextId,
    ) -> arclain_plugins::types::Result<Vec<arclain_core::ArchiveEntry>> {
        let session = self.resolve(context_id)?;
        let paths = session.all_file_paths();
        Ok(paths
            .into_iter()
            .map(|path| arclain_core::ArchiveEntry {
                path,
                size: 0,
                packed_size: 0,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            })
            .collect())
    }

    fn read_entry(
        &self,
        _context_id: PluginArchiveContextId,
        _path: &str,
    ) -> arclain_plugins::types::Result<Vec<u8>> {
        // `ArchiveSession` retains only its indexed metadata, not a
        // read-entry-bytes operation -- extracting a single entry's
        // content goes through `crate::operations::extract` today, not
        // through this trait. Reserved for a future task that wires a
        // real single-entry read.
        Err(PluginError::Unavailable(
            "reading a single archive entry is not yet implemented".to_string(),
        ))
    }

    fn write_metadata(
        &self,
        _context_id: PluginArchiveContextId,
        _value: serde_json::Value,
    ) -> arclain_plugins::types::Result<()> {
        // See `ArchiveContextBridge::set_session_metadata`'s doc comment:
        // `ArchiveSession` has no metadata slot to persist into yet.
        Err(PluginError::Unavailable(
            "writing archive metadata through this adapter is not yet implemented".to_string(),
        ))
    }
}

/// The `ActiveTabBridge` this application installs on `PluginManager` in
/// production: [`ArchiveContextBridge`], composed with a caller-supplied
/// `fallback` for the one case archive-session state alone cannot
/// resolve.
///
/// `ActiveTabBridge::set_active_tab_metadata`'s own doc comment mandates
/// that a production bridge must not leave that method a no-op: "a
/// no-op is an acceptable implementation only if this bridge truly has
/// no notion of the active tab at all". `ArchiveContextBridge` alone
/// *is* exactly that case whenever no archive session is active -- at
/// the application layer there is no "active tab" independent of a
/// session, only whichever session `set_active_archive_session` last
/// reported. A frontend's own notion of "the active tab" (an egui tab,
/// a Flutter route) is precisely the one thing this crate must never
/// know about directly (see the crate doc comment's toolkit-independence
/// rule) -- so `fallback` is how a frontend supplies exactly that one
/// piece, and nothing more: it is called with the metadata payload only
/// when [`ActiveTabBridge::active_archive_session_id`] is `None` at the
/// time of the call. Every other method, including `set_active_tab_
/// metadata` whenever a session *is* active, resolves entirely through
/// `inner`, independent of `fallback`.
pub(crate) struct ProductionActiveTabBridge {
    inner: ArchiveContextBridge,
    fallback: Box<dyn Fn(Option<serde_json::Value>) + Send + Sync>,
}

impl ProductionActiveTabBridge {
    pub(crate) fn new(
        inner: ArchiveContextBridge,
        fallback: impl Fn(Option<serde_json::Value>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner,
            fallback: Box::new(fallback),
        }
    }
}

impl ActiveTabBridge for ProductionActiveTabBridge {
    fn archive_path(&self) -> Option<String> {
        self.inner.archive_path()
    }

    fn current_password(&self) -> Option<String> {
        self.inner.current_password()
    }

    fn archive_entries(&self) -> Vec<String> {
        self.inner.archive_entries()
    }

    /// Forwarded explicitly, not left to the trait default: the default
    /// would call `self.archive_entries()` above (this composite's own
    /// override of *that*, which just forwards too) and `.len()` it,
    /// undoing `ArchiveContextBridge`'s own `O(1)` override one layer up.
    fn archive_entry_count(&self) -> usize {
        self.inner.archive_entry_count()
    }

    /// Forwarded explicitly for the same reason as
    /// [`Self::archive_entry_count`] -- see `ArchiveContextBridge::
    /// archive_entries_page`'s own doc comment for the `O(n)`-per-page
    /// cost this avoids.
    fn archive_entries_page(&self, offset: usize, limit: usize) -> Vec<String> {
        self.inner.archive_entries_page(offset, limit)
    }

    fn active_archive_session_id(&self) -> Option<u64> {
        self.inner.active_archive_session_id()
    }

    fn set_session_metadata(&self, archive_session_id: u64, metadata: Option<serde_json::Value>) {
        self.inner
            .set_session_metadata(archive_session_id, metadata);
    }

    fn set_active_tab_metadata(&self, metadata: Option<serde_json::Value>) {
        // Resolved exactly once: `active_archive_session_id()` and
        // `inner.set_active_tab_metadata` each independently re-read the
        // same underlying `ActiveArchiveSession` state, so checking here
        // and then calling the latter would be a TOCTOU -- the session
        // could change (or clear) between the two reads, and either the
        // fallback or the session write could silently see a different
        // answer than the one this method decided on. Using the single
        // resolved id to call `set_session_metadata` directly (rather
        // than `inner.set_active_tab_metadata`, which would re-resolve)
        // closes that gap.
        match self.inner.active_archive_session_id() {
            Some(session_id) => self.inner.set_session_metadata(session_id, metadata),
            None => (self.fallback)(metadata),
        }
    }

    fn set_archive_path(&self, path: Option<String>) {
        self.inner.set_archive_path(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;
    use std::sync::Mutex as StdMutex;
    use wirt::ui_model::PluginToastLevelDto;
    use wirt::{PluginLayout, PluginUiElement};

    /// Minimal in-memory `arclain_data::CacheIndex` -- enough to exercise
    /// `ContentCache::put`/`get_with_limit_for_owner` without a real
    /// SQLite-backed index.
    #[derive(Default)]
    struct InMemoryCacheIndex {
        entries: StdMutex<StdHashMap<String, arclain_db::CacheEntry>>,
    }

    impl arclain_data::CacheIndex for InMemoryCacheIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: arclain_db::CacheType,
            size_bytes: Option<i64>,
        ) -> anyhow::Result<i64> {
            let mut entries = self.entries.lock().unwrap();
            let id = entries.len() as i64 + 1;
            entries.insert(
                key.to_string(),
                arclain_db::CacheEntry {
                    id,
                    key: key.to_string(),
                    product_id: product_id.map(str::to_string),
                    content_hash: content_hash.to_string(),
                    source_url: source_url.map(str::to_string),
                    cache_type,
                    created_at: String::new(),
                    last_accessed: None,
                    size_bytes,
                },
            );
            Ok(id)
        }

        fn get(&self, key: &str) -> anyhow::Result<Option<arclain_db::CacheEntry>> {
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        fn has(&self, key: &str) -> anyhow::Result<bool> {
            Ok(self.entries.lock().unwrap().contains_key(key))
        }

        fn delete(&self, key: &str) -> anyhow::Result<bool> {
            Ok(self.entries.lock().unwrap().remove(key).is_some())
        }

        fn delete_by_pattern(&self, _pattern: &str) -> anyhow::Result<usize> {
            Ok(0)
        }

        fn update_last_accessed(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// A cache whose disk-containment limits are the defaults except for
    /// the free-space floor, which is zeroed.
    ///
    /// Every cache write reserves before committing, and the reservation
    /// refuses to proceed unless the filesystem has `min_free_space_bytes`
    /// (2 GiB by default) of headroom to spare. Temp directories here sit
    /// on whatever the machine calls TEMP -- a small RAM disk on at least
    /// one development machine -- so leaving the default in place makes
    /// every write test fail or pass on free space rather than on the
    /// behaviour it is asserting.
    fn test_content_cache() -> (tempfile::TempDir, arclain_data::ContentCache) {
        let root = tempfile::tempdir().unwrap();
        let cache = arclain_data::ContentCache::new_with_limits(
            root.path().join("cache"),
            Arc::new(InMemoryCacheIndex::default()),
            arclain_data::CacheLimits {
                min_free_space_bytes: 0,
                ..Default::default()
            },
        )
        .unwrap();
        (root, cache)
    }

    fn simple_layout() -> PluginLayout {
        PluginLayout::Single {
            elements: vec![PluginUiElement::Button {
                id: "go".to_string(),
                label: "Go".to_string(),
                action: None,
            }],
        }
    }

    #[test]
    fn plugin_ui_document_round_trips_through_serde() {
        let root = ui_model::normalize_layout(&simple_layout()).unwrap();
        let document = PluginUiDocument {
            session_id: PluginSessionId::from_raw(7),
            plugin_id: "demo".to_string(),
            region_id: PluginExtensionPointDto::MainPage.region_slug(),
            extension_point: PluginExtensionPointDto::MainPage,
            revision: 1,
            root,
        };
        fn accepts_wirt_root(_: &wirt::ui_model::PluginUiNodeDto) {}
        accepts_wirt_root(&document.root);

        let json = serde_json::to_string(&document).unwrap();
        let restored: PluginUiDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, document);
    }

    /// Minimal `ArchiveBackend` double, just enough to open a real
    /// `ArchiveSession` through `ArchiveSessionStore::open` for
    /// `ArchiveContextBridge`/`ArchiveContextAccess`'s own tests below.
    struct NoopBackend;
    impl arclain_core::ArchiveBackend for NoopBackend {
        fn name(&self) -> &str {
            "noop"
        }
        fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
            arclain_core::archive::BackendCapabilities::read_only()
        }
        fn identify(
            &self,
            _path: &std::path::Path,
        ) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
            Ok(arclain_core::archive::ArchiveKind::Zip)
        }
        fn list(
            &self,
            _path: &std::path::Path,
            _password: Option<&str>,
        ) -> anyhow::Result<arclain_core::ArchiveInfo> {
            unimplemented!("not exercised by these tests")
        }
        fn extract_all(
            &self,
            _p: &std::path::Path,
            _d: &std::path::Path,
            _pw: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_files(
            &self,
            _p: &std::path::Path,
            _d: &std::path::Path,
            _f: &[String],
            _pw: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_directory(
            &self,
            _p: &std::path::Path,
            _d: &std::path::Path,
            _dp: &str,
            _pw: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn recompress_7z(&self, _s: &std::path::Path, _d: &std::path::Path) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_files(&self, _a: &std::path::Path, _f: &[std::path::PathBuf]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn create_archive(
            &self,
            _d: &std::path::Path,
            _f: &[std::path::PathBuf],
            _fmt: &str,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn read_text_file(
            &self,
            _a: &std::path::Path,
            _p: &str,
            _pw: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
        fn delete_files(&self, _a: &std::path::Path, _f: &[String]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_or_update_file_from_str(
            &self,
            _a: &std::path::Path,
            _p: &str,
            _c: &str,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn convert_to_7z(
            &self,
            _s: &arclain_core::Archive,
            _d: &std::path::Path,
            _t: &std::path::Path,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn crc32_of_entry(
            &self,
            _a: &std::path::Path,
            _p: &str,
            _pw: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
    }

    async fn open_test_session(
        store: &crate::archive::ArchiveSessionStore,
        entries: Vec<arclain_core::ArchiveEntry>,
    ) -> ArchiveSessionId {
        let archive = arclain_core::Archive::new(Arc::new(NoopBackend), "fixture.zip");
        let session = store
            .open(
                std::path::PathBuf::from("fixture.zip"),
                "zip".to_string(),
                archive,
                Arc::new(entries),
                crate::archive::SessionEncryption::default(),
                &tokio::runtime::Handle::current(),
            )
            .await
            .unwrap();
        session.id()
    }

    /// Regression test for the actual production requirement: every write
    /// through this bridge must land from ANY calling thread, not only a
    /// Tokio multi-thread runtime worker. `arclain_plugins::PluginManager`'s
    /// event-dispatch worker -- which calls a plugin's `OnArchiveOpen`
    /// handler, and is where BOTH of the primary metadata-write sinks
    /// (`arclain_plugins::manager::dispatch::set_event_session_metadata`
    /// and the event-context branch of `crate::host_functions::metadata`'s
    /// `emit_metadata`) actually call `set_session_metadata` in production
    /// -- runs on a bare `std::thread::spawn` thread with no Tokio context
    /// entered at all.
    ///
    /// This test replaces one that asserted the *opposite*: that a write
    /// from exactly this kind of thread silently no-ops. That was the bug,
    /// not a safety margin -- "safely" dropping the primary flow's every
    /// metadata write is not a passing behavior. `with_session` no longer
    /// depends on `Handle::try_current()`/`block_in_place` at all (see its
    /// own doc comment), so this now genuinely lands the write and
    /// publishes the `SessionEvent` from any thread.
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_writes_land_from_a_bare_thread_with_no_tokio_context() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        // A bare OS thread, exactly like `PluginManager`'s own event
        // worker (`std::thread::spawn`, no runtime entered) -- not a tokio
        // task, not routed through `spawn_blocking`.
        let handle = std::thread::spawn(move || {
            bridge.set_session_metadata(session_id.into_raw(), Some(serde_json::json!({"a": 1})));
        });
        handle.join().expect("worker thread must not panic");

        let session = sessions.get(session_id).await.unwrap();
        assert_eq!(
            session.snapshot().metadata,
            Some(serde_json::json!({"a": 1})),
            "a write from a bare thread with no Tokio context must still land in the session store"
        );
        assert_eq!(
            events
                .try_recv()
                .expect("a SessionEvent must have been published"),
            crate::event::SessionEvent::MetadataChanged { session_id }
        );
    }

    /// Same requirement, for a read: `archive_path`/`archive_entries` must
    /// resolve the real active session from a bare thread too, not just
    /// avoid panicking.
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_reads_resolve_from_a_bare_thread_with_no_tokio_context() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(
            &sessions,
            vec![arclain_core::ArchiveEntry {
                path: "readme.txt".to_string(),
                size: 10,
                packed_size: 10,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
        )
        .await;
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions);

        let handle = std::thread::spawn(move || (bridge.archive_path(), bridge.archive_entries()));
        let (path, entries) = handle.join().expect("worker thread must not panic");

        assert!(
            path.unwrap().contains("fixture.zip"),
            "archive_path must resolve the real active session from a bare thread"
        );
        assert_eq!(entries, vec!["readme.txt".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_resolves_the_active_session_and_none_when_unset() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(
            &sessions,
            vec![arclain_core::ArchiveEntry {
                path: "readme.txt".to_string(),
                size: 10,
                packed_size: 10,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
        )
        .await;
        let tracker = ActiveArchiveSession::new();
        let bridge = ArchiveContextBridge::new(tracker.clone(), sessions);

        assert!(bridge.archive_path().is_none(), "no active session yet");
        assert!(bridge.archive_entries().is_empty());
        assert_eq!(bridge.archive_entry_count(), 0);
        assert!(bridge.archive_entries_page(0, 10).is_empty());
        assert_eq!(bridge.active_archive_session_id(), None);

        tracker.set(Some(session_id));

        assert!(bridge.archive_path().unwrap().contains("fixture.zip"));
        assert_eq!(bridge.archive_entries(), vec!["readme.txt".to_string()]);
        assert_eq!(bridge.archive_entry_count(), 1);
        assert_eq!(
            bridge.archive_entries_page(0, 10),
            vec!["readme.txt".to_string()]
        );
        assert_eq!(
            bridge.active_archive_session_id(),
            Some(session_id.into_raw())
        );
    }

    /// Regression test for the fix making `set_active_tab_metadata` a real
    /// write instead of the no-op it was before this task -- see
    /// `ActiveTabBridge::set_active_tab_metadata`'s own doc comment
    /// mandating exactly this for "every production implementation".
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_set_active_tab_metadata_writes_the_active_session() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        bridge.set_active_tab_metadata(Some(serde_json::json!({"title": "demo"})));

        let session = sessions.get(session_id).await.unwrap();
        assert_eq!(
            session.snapshot().metadata,
            Some(serde_json::json!({"title": "demo"}))
        );
    }

    /// `set_session_metadata` resolves by the given session id, not by
    /// whichever session happens to be active right now -- proving the
    /// distinction `with_session`/`with_active_session` exist to draw.
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_set_session_metadata_targets_the_given_session_not_the_active_one(
    ) {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let target_session_id = open_test_session(&sessions, vec![]).await;
        let other_session_id = open_test_session(&sessions, vec![]).await;
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(other_session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        bridge.set_session_metadata(
            target_session_id.into_raw(),
            Some(serde_json::json!({"title": "targeted"})),
        );

        let target = sessions.get(target_session_id).await.unwrap();
        let other = sessions.get(other_session_id).await.unwrap();
        assert_eq!(
            target.snapshot().metadata,
            Some(serde_json::json!({"title": "targeted"}))
        );
        assert_eq!(
            other.snapshot().metadata,
            None,
            "the active-but-not-targeted session must be untouched"
        );
    }

    /// A stale or fabricated session id must never panic -- matches
    /// `ActiveTabBridge::set_session_metadata`'s own doc comment: "the
    /// write is simply lost".
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_set_session_metadata_on_an_unknown_session_is_a_silent_no_op() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let tracker = ActiveArchiveSession::new();
        let bridge = ArchiveContextBridge::new(tracker, sessions);

        bridge.set_session_metadata(999_999, Some(serde_json::json!({"ignored": true})));
    }

    /// Regression test for the fix making `set_archive_path` a real write
    /// instead of the no-op it was before this task -- the `rename_archive`
    /// host function's only write sink.
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_bridge_set_archive_path_renames_the_active_session() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        bridge.set_archive_path(Some("renamed.zip".to_string()));

        assert!(bridge.archive_path().unwrap().contains("renamed.zip"));
        let session = sessions.get(session_id).await.unwrap();
        assert!(session
            .snapshot()
            .source_path
            .to_string_lossy()
            .contains("renamed.zip"));
    }

    // -- SessionEvent emission -------------------------------------------

    /// A metadata write that actually lands (a real, still-open session)
    /// publishes `SessionEvent::MetadataChanged` for that session id,
    /// after the write is already visible through `archive_snapshot`
    /// (via `session.snapshot()` here, the same read path).
    #[tokio::test(flavor = "multi_thread")]
    async fn set_session_metadata_publishes_metadata_changed_after_the_write_lands() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        bridge.set_session_metadata(session_id.into_raw(), Some(serde_json::json!({"a": 1})));

        let event = events
            .try_recv()
            .expect("an event must have been published");
        assert_eq!(
            event,
            crate::event::SessionEvent::MetadataChanged { session_id }
        );
        // The event only carries the id -- confirm the write it announces
        // is already committed and observable by the time it arrives.
        let session = sessions.get(session_id).await.unwrap();
        assert_eq!(
            session.snapshot().metadata,
            Some(serde_json::json!({"a": 1}))
        );
    }

    /// A write that never lands (unknown/stale session id) must not
    /// publish -- there is nothing for a subscriber to usefully
    /// reconcile.
    #[tokio::test(flavor = "multi_thread")]
    async fn set_session_metadata_on_an_unknown_session_does_not_publish() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        let bridge = ArchiveContextBridge::new(tracker, sessions);

        bridge.set_session_metadata(999_999, Some(serde_json::json!({"ignored": true})));

        assert!(
            events.try_recv().is_err(),
            "no event must be published for a write that never landed"
        );
    }

    /// The active-tab metadata write publishes exactly like the
    /// explicit-session-id path, when a session is active.
    #[tokio::test(flavor = "multi_thread")]
    async fn set_active_tab_metadata_publishes_metadata_changed_when_a_session_is_active() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        bridge.set_active_tab_metadata(Some(serde_json::json!({"title": "demo"})));

        assert_eq!(
            events.try_recv().unwrap(),
            crate::event::SessionEvent::MetadataChanged { session_id }
        );
    }

    /// With no active session at all, `set_active_tab_metadata` is a
    /// no-op at this layer (see the module doc comment on where the
    /// fallback lives instead) -- and correctly publishes nothing, since
    /// nothing in the session store changed.
    #[tokio::test(flavor = "multi_thread")]
    async fn set_active_tab_metadata_does_not_publish_without_an_active_session() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        let bridge = ArchiveContextBridge::new(tracker, sessions);

        bridge.set_active_tab_metadata(Some(serde_json::json!({"title": "demo"})));

        assert!(events.try_recv().is_err());
    }

    /// A rename mutates `source_path`, which is session-visible state
    /// through `archive_snapshot` exactly like `metadata` -- it publishes
    /// the same event so a subscriber reconciling via `archive_snapshot`
    /// picks up the new path too.
    #[tokio::test(flavor = "multi_thread")]
    async fn set_archive_path_publishes_metadata_changed_after_the_rename_lands() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions.clone());

        bridge.set_archive_path(Some("renamed.zip".to_string()));

        assert_eq!(
            events.try_recv().unwrap(),
            crate::event::SessionEvent::MetadataChanged { session_id }
        );
    }

    /// `set_archive_path(None)` is a defensive early return with no
    /// session lookup at all (see the method's own early `let Some(path)
    /// = path else { return }`) -- confirm it does not publish either.
    #[tokio::test(flavor = "multi_thread")]
    async fn set_archive_path_with_no_path_does_not_publish() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let mut events = sessions.subscribe_session_events();
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let bridge = ArchiveContextBridge::new(tracker, sessions);

        bridge.set_archive_path(None);

        assert!(events.try_recv().is_err());
    }

    // -- ProductionActiveTabBridge ----------------------------------------

    /// With a session active, the composite bridge behaves exactly like
    /// `ArchiveContextBridge` alone -- the fallback must never run.
    #[tokio::test(flavor = "multi_thread")]
    async fn production_bridge_delegates_to_inner_and_skips_the_fallback_when_a_session_is_active()
    {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(&sessions, vec![]).await;
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let inner = ArchiveContextBridge::new(tracker, sessions.clone());
        let fallback_calls: Arc<StdMutex<Vec<Option<serde_json::Value>>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let fallback_calls_for_closure = fallback_calls.clone();
        let bridge = ProductionActiveTabBridge::new(inner, move |metadata| {
            fallback_calls_for_closure.lock().unwrap().push(metadata);
        });

        bridge.set_active_tab_metadata(Some(serde_json::json!({"title": "demo"})));

        let session = sessions.get(session_id).await.unwrap();
        assert_eq!(
            session.snapshot().metadata,
            Some(serde_json::json!({"title": "demo"}))
        );
        assert!(
            fallback_calls.lock().unwrap().is_empty(),
            "the fallback must not run when a session is active"
        );
    }

    /// With no session active at all, the composite bridge must not
    /// silently drop the write (matching `ActiveTabBridge::
    /// set_active_tab_metadata`'s own "every production implementation
    /// must actually write somewhere" mandate) -- it hands the payload to
    /// the fallback instead, unmodified.
    #[tokio::test(flavor = "multi_thread")]
    async fn production_bridge_invokes_the_fallback_when_no_session_is_active() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let tracker = ActiveArchiveSession::new();
        let inner = ArchiveContextBridge::new(tracker, sessions);
        let fallback_calls: Arc<StdMutex<Vec<Option<serde_json::Value>>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let fallback_calls_for_closure = fallback_calls.clone();
        let bridge = ProductionActiveTabBridge::new(inner, move |metadata| {
            fallback_calls_for_closure.lock().unwrap().push(metadata);
        });

        bridge.set_active_tab_metadata(Some(serde_json::json!({"title": "no session"})));

        assert_eq!(
            fallback_calls.lock().unwrap().as_slice(),
            [Some(serde_json::json!({"title": "no session"}))]
        );
    }

    /// Every other `ActiveTabBridge` method is pure delegation to `inner`,
    /// with no fallback involvement at all -- proven by driving each one
    /// against a real session and checking it observes exactly what
    /// `ArchiveContextBridge` alone would have produced.
    #[tokio::test(flavor = "multi_thread")]
    async fn production_bridge_delegates_every_other_method_to_inner() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(
            &sessions,
            vec![arclain_core::ArchiveEntry {
                path: "a.txt".to_string(),
                size: 1,
                packed_size: 1,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
        )
        .await;
        let tracker = ActiveArchiveSession::new();
        tracker.set(Some(session_id));
        let inner = ArchiveContextBridge::new(tracker, sessions.clone());
        let bridge = ProductionActiveTabBridge::new(inner, |_| {
            panic!("fallback must never run for this test")
        });

        assert!(bridge.archive_path().unwrap().contains("fixture.zip"));
        assert_eq!(bridge.archive_entries(), vec!["a.txt".to_string()]);
        assert_eq!(bridge.archive_entry_count(), 1);
        assert_eq!(
            bridge.archive_entries_page(0, 10),
            vec!["a.txt".to_string()]
        );
        assert_eq!(
            bridge.active_archive_session_id(),
            Some(session_id.into_raw())
        );
        assert_eq!(bridge.current_password(), None);

        bridge.set_session_metadata(session_id.into_raw(), Some(serde_json::json!({"n": 1})));
        assert_eq!(
            sessions.get(session_id).await.unwrap().snapshot().metadata,
            Some(serde_json::json!({"n": 1}))
        );

        bridge.set_archive_path(Some("renamed.zip".to_string()));
        assert!(bridge.archive_path().unwrap().contains("renamed.zip"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn archive_context_access_lists_entries_for_a_known_context_and_rejects_an_unknown_one() {
        let sessions = Arc::new(crate::archive::ArchiveSessionStore::new());
        let session_id = open_test_session(
            &sessions,
            vec![arclain_core::ArchiveEntry {
                path: "game/data.bin".to_string(),
                size: 5,
                packed_size: 5,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
        )
        .await;
        let access = ArchiveContextAccess::new(sessions);

        let entries = access
            .list_entries(PluginArchiveContextId::from_raw(session_id.into_raw()))
            .expect("known context must resolve");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "game/data.bin");

        let unknown = PluginArchiveContextId::from_raw(999_999);
        assert!(access.list_entries(unknown).is_err());
    }

    #[test]
    fn active_archive_session_defaults_to_none_and_round_trips_a_set_value() {
        let tracker = ActiveArchiveSession::new();
        assert_eq!(tracker.get(), None);
        tracker.set(Some(ArchiveSessionId::from_raw(9)));
        assert_eq!(tracker.get(), Some(ArchiveSessionId::from_raw(9)));
        tracker.set(None);
        assert_eq!(tracker.get(), None);
    }

    /// The refusal's own shape, in isolation from any running plugin: the
    /// behavior tests live in `crates/app/tests/plugin_sessions.rs` and
    /// drive a real guest, but *which* envelope they assert on is decided
    /// here.
    #[test]
    fn the_disabled_refusal_is_permission_denied_and_names_its_plugin() {
        let error = plugin_disabled("ui-demo");

        assert_eq!(error.kind, ApplicationErrorKind::PermissionDenied);
        assert_eq!(error.summary, PLUGIN_DISABLED_SUMMARY);
        assert_eq!(error.diagnostic.as_deref(), Some("plugin id: ui-demo"));
        assert_eq!(error.recoverability, Recoverability::UserAction);
        assert!(!error.retryable, "retrying alone cannot enable a plugin");
        // Nothing about the caller's request is wrong -- the identical
        // request succeeds once the plugin is enabled -- so no request
        // field is blamed.
        assert_eq!(error.field, None);
    }

    /// The distinction the gate exists to make legible. Both "not found"
    /// errors a plugin surface can produce must fall *outside* the
    /// predicate, or a renderer would treat a stale plugin reference as a
    /// temporarily switched-off one and keep it forever.
    #[test]
    fn only_the_disabled_refusal_answers_the_disabled_predicate() {
        assert!(is_plugin_disabled_refusal(&plugin_disabled("ui-demo")));
        assert!(!is_plugin_disabled_refusal(&plugin_not_found("ui-demo")));
        assert!(!is_plugin_disabled_refusal(&unknown_plugin_session(
            PluginSessionId::from_raw(7)
        )));
        assert!(!is_plugin_disabled_refusal(&plugin_manager_unavailable()));
        // A `PermissionDenied` from a *different* refusal on this same
        // surface must not be mistaken for it either.
        assert!(!is_plugin_disabled_refusal(
            &authorize_plugin_image_write("ui-demo", "plugin-image:other:key").unwrap_err()
        ));
    }

    #[test]
    fn refresh_panel_sets_the_refresh_flag_and_is_not_surfaced_as_an_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::RefreshPanel {
                extension_point: "MainPage".to_string(),
            },
            "demo",
            &mut intents,
            &mut outcome,
        );
        assert!(outcome.needs_refresh);
        assert!(intents.is_empty());
    }

    /// `RequestFetch` is host-internal: it never reaches a renderer as an
    /// intent, and it does not itself ask for a refresh. It is collected
    /// so `dispatch_action` can resolve it against the plugin manager on a
    /// blocking thread -- see `resolve_request_fetches`.
    #[test]
    fn request_fetch_is_collected_for_the_host_rather_than_surfaced_as_an_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::RequestFetch {
                key: "dlsite:RJ000001".to_string(),
            },
            "demo",
            &mut intents,
            &mut outcome,
        );
        assert!(!outcome.needs_refresh);
        assert!(intents.is_empty());
        assert_eq!(outcome.fetch_keys, vec!["dlsite:RJ000001".to_string()]);
    }

    /// Several keys in one response are all retained, in emission order:
    /// unlike `RefreshPanel` (which coalesces to a single re-fetch), two
    /// fetches name two different products and both must run.
    #[test]
    fn several_request_fetch_keys_in_one_response_are_all_retained_in_order() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        for key in ["dlsite:RJ1", "fanza:VJ2"] {
            apply_bounded_action(
                wirt::PluginAction::RequestFetch {
                    key: key.to_string(),
                },
                "demo",
                &mut intents,
                &mut outcome,
            );
        }
        assert_eq!(
            outcome.fetch_keys,
            vec!["dlsite:RJ1".to_string(), "fanza:VJ2".to_string()]
        );
        assert!(intents.is_empty());
    }

    #[test]
    fn cache_key_codec_round_trips_a_raw_key_containing_colons() {
        let encoded = encode_plugin_image_cache_key("dlsite-metadata", "dlsite:image:RJ000001");
        let (plugin_id, raw_key) = decode_plugin_image_cache_key(&encoded).unwrap();
        assert_eq!(plugin_id, "dlsite-metadata");
        assert_eq!(raw_key, "dlsite:image:RJ000001");
    }

    #[test]
    fn decode_rejects_a_key_this_module_never_encoded() {
        assert!(decode_plugin_image_cache_key("dlsite:image:RJ000001").is_none());
        assert!(decode_plugin_image_cache_key("plugin-image:no-colon-separator").is_none());
    }

    #[test]
    fn rewrite_cache_keys_encodes_every_image_bearing_node_recursively() {
        let layout = PluginLayout::Single {
            elements: vec![
                PluginUiElement::Image {
                    cache_key: Some("cover:1".to_string()),
                    url: None,
                    max_height: None,
                },
                PluginUiElement::ListContainer {
                    id: "list".to_string(),
                    items: vec![PluginUiElement::ListItem {
                        id: "row".to_string(),
                        title: "Row".to_string(),
                        subtitle: None,
                        badge: None,
                        image_key: Some("thumb:1".to_string()),
                        image_url: None,
                        selected: false,
                        warning_icon: None,
                    }],
                    max_height: None,
                    empty_message: None,
                },
            ],
        };
        let root = ui_model::normalize_layout(&layout).unwrap();
        let rewritten = rewrite_cache_keys(root, "demo-plugin");

        let PluginUiNodeKind::Single { children } = &rewritten.kind else {
            panic!("expected Single root");
        };
        let PluginUiNodeKind::Image { cache_key, .. } = &children[0].kind else {
            panic!("expected Image node");
        };
        assert_eq!(
            cache_key.as_deref(),
            Some("plugin-image:demo-plugin:cover:1")
        );

        let PluginUiNodeKind::ListContainer { children, .. } = &children[1].kind else {
            panic!("expected ListContainer node");
        };
        let PluginUiNodeKind::ListItem { image_key, .. } = &children[0].kind else {
            panic!("expected ListItem node");
        };
        assert_eq!(
            image_key.as_deref(),
            Some("plugin-image:demo-plugin:thumb:1")
        );
    }

    #[test]
    fn read_plugin_image_rejects_an_unknown_cache_key() {
        let (_root, cache) = test_content_cache();
        let error = read_plugin_image(&cache, "not-an-encoded-key").unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    }

    #[test]
    fn read_plugin_image_rejects_a_key_the_cache_never_stored() {
        let (_root, cache) = test_content_cache();
        let encoded = encode_plugin_image_cache_key("demo-plugin", "missing-key");
        let error = read_plugin_image(&cache, &encoded).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    }

    #[test]
    fn read_plugin_image_returns_the_cached_bytes_byte_for_byte() {
        let (_root, cache) = test_content_cache();
        let bytes = vec![0xAB_u8; 4096];
        cache
            .put_for_owner(
                &arclain_data::CacheOwner::plugin("demo-plugin"),
                "cover:1",
                &bytes,
                arclain_db::CacheType::Cover,
                None,
                None,
            )
            .unwrap();
        let encoded = encode_plugin_image_cache_key("demo-plugin", "cover:1");

        let read = read_plugin_image(&cache, &encoded).unwrap();

        assert_eq!(read, bytes);
    }

    /// The namespace predicate is total and the two vocabularies are
    /// disjoint: no string is addressable as both.
    #[test]
    fn every_cache_key_belongs_to_exactly_one_image_namespace() {
        let plugin_key = encode_plugin_image_cache_key("demo-plugin", "cover:1");
        assert!(is_plugin_image_key(&plugin_key));
        assert_eq!(plugin_image_key_owner(&plugin_key), Some("demo-plugin"));
        assert!(host_image_key(&plugin_key).is_err());

        for host_key in ["dlsite:image:RJ1", "", "plugin-image", "plugin-imag:x:y"] {
            assert!(!is_plugin_image_key(host_key), "{host_key:?}");
            assert_eq!(plugin_image_key_owner(host_key), None, "{host_key:?}");
            assert_eq!(host_image_key(host_key).unwrap(), host_key);
        }
    }

    /// Crafted key, host door: a plugin-scoped key must not be readable,
    /// writable, or evictable through the host surface -- otherwise every
    /// host method is a second, unauthorized way into a plugin's namespace.
    #[test]
    fn the_host_image_surface_refuses_a_key_naming_a_plugin_namespace() {
        let (_root, cache) = test_content_cache();
        let victim = vec![0x11_u8; MIN_FETCHED_IMAGE_BYTES + 1];
        let victim_key = encode_plugin_image_cache_key("victim-plugin", "secret");
        write_plugin_image(&cache, "victim-plugin", &victim_key, &victim, None).unwrap();

        for error in [
            read_host_image(&cache, &victim_key).unwrap_err(),
            discard_host_image(&cache, &victim_key).unwrap_err(),
        ] {
            assert_eq!(error.kind, ApplicationErrorKind::PermissionDenied);
            assert_eq!(error.field.as_deref(), Some("cache_key"));
        }

        assert_eq!(
            read_plugin_image(&cache, &victim_key).unwrap(),
            victim,
            "the victim's own entry must be untouched by the refused attempts"
        );
    }

    /// Crafted key, storage vocabulary: a host key that *is* a plugin row's
    /// storage-scoped string must neither read nor delete that row.
    ///
    /// This is the second, non-obvious namespace door. `ContentCache`'s
    /// host read/remove/has fall back to the unscoped keyspace for
    /// pre-scoping rows, and every plugin row is indexed under its scoped
    /// string -- so before `host_image_key` learned to refuse that
    /// vocabulary, this exact key returned the victim's bytes verbatim and
    /// then destroyed the victim's entry. Reachable in production: the flat
    /// legacy renderer still hands plugin-authored, unstamped `cache_key`
    /// strings to the host path.
    #[test]
    fn the_host_image_surface_refuses_a_storage_scoped_key_for_a_plugin_row() {
        let (_root, cache) = test_content_cache();
        let victim = vec![0x33_u8; 1024];
        let victim_key = encode_plugin_image_cache_key("victim-plugin", "secret");
        write_plugin_image(&cache, "victim-plugin", &victim_key, &victim, None).unwrap();
        // The exact string the storage layer indexes the victim's row under.
        let storage_key = arclain_data::CacheOwner::plugin("victim-plugin").scoped_key("secret");

        let read_error = read_host_image(&cache, &storage_key).unwrap_err();
        let discard_error = discard_host_image(&cache, &storage_key).unwrap_err();

        for error in [&read_error, &discard_error] {
            assert_eq!(error.kind, ApplicationErrorKind::PermissionDenied);
            assert_eq!(error.field.as_deref(), Some("cache_key"));
        }
        assert_eq!(
            read_plugin_image(&cache, &victim_key).unwrap(),
            victim,
            "the victim's row must survive both attempts intact"
        );
    }

    /// The same door in the other direction and in its other shapes: a
    /// host-scoped string, and a malformed key merely wearing the sentinel.
    #[test]
    fn the_host_image_surface_refuses_every_storage_scoped_shape() {
        let (_root, cache) = test_content_cache();
        let host_scoped = arclain_data::CacheOwner::host().scoped_key("dlsite:image:RJ1");
        let malformed = format!("{CACHE_SCOPED_KEY_SENTINEL}arclain-cache:v9:nonsense");

        for key in [host_scoped.as_str(), malformed.as_str()] {
            assert_eq!(
                read_host_image(&cache, key).unwrap_err().kind,
                ApplicationErrorKind::PermissionDenied,
                "{key:?}"
            );
            assert_eq!(
                discard_host_image(&cache, key).unwrap_err().kind,
                ApplicationErrorKind::PermissionDenied,
                "{key:?}"
            );
        }
    }

    /// The sentinel must stay the byte the storage layer actually emits.
    #[test]
    fn the_scoped_key_sentinel_matches_the_storage_encoding() {
        for scoped in [
            arclain_data::CacheOwner::host().scoped_key("k"),
            arclain_data::CacheOwner::plugin("demo").scoped_key("k"),
        ] {
            assert!(
                scoped.starts_with(CACHE_SCOPED_KEY_SENTINEL),
                "storage scoped keys must still carry the sentinel: {scoped:?}"
            );
        }
    }

    /// Crafted key, plugin door: a host-owned key must not be writable
    /// through the plugin surface either. The refusal is what keeps a
    /// plugin-attributed write out of the shared host namespace that other
    /// plugins' legacy documents read from.
    #[test]
    fn the_plugin_image_surface_refuses_a_host_owned_key() {
        let (_root, cache) = test_content_cache();

        let error = write_plugin_image(&cache, "demo-plugin", "dlsite:image:RJ1", b"bytes", None)
            .unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
        assert!(read_host_image(&cache, "dlsite:image:RJ1").is_err());
    }

    #[test]
    fn read_host_image_returns_the_cached_bytes_byte_for_byte() {
        let (_root, cache) = test_content_cache();
        let bytes = vec![0xCD_u8; 2048];
        cache
            .put(
                "dlsite:image:RJ1",
                &bytes,
                arclain_db::CacheType::Screenshot,
                None,
                None,
            )
            .unwrap();

        assert_eq!(read_host_image(&cache, "dlsite:image:RJ1").unwrap(), bytes);
        assert!(discard_host_image(&cache, "dlsite:image:RJ1").unwrap());
        assert_eq!(
            read_host_image(&cache, "dlsite:image:RJ1")
                .unwrap_err()
                .kind,
            ApplicationErrorKind::NotFound
        );
    }

    /// The regression this pins: a host image *larger than the plugin cap*
    /// but within the host cap must keep round-tripping.
    ///
    /// Host reads have always been bounded by the content cache's default
    /// materialized-read ceiling, so entries in the 16-50 MiB band exist in
    /// real caches and render today. An earlier version of this surface
    /// gave host images the plugin cap, which retired every one of them
    /// permanently -- the read refused them and so did every URL-fallback
    /// refetch, so nothing could heal it. The two caps differ because the
    /// namespaces are different trust boundaries, and this is the case that
    /// tells them apart.
    #[test]
    fn a_host_image_larger_than_the_plugin_cap_still_round_trips() {
        let (_root, cache) = test_content_cache();
        let oversized_for_a_plugin = vec![0x5A_u8; MAX_PLUGIN_IMAGE_BYTES as usize + 1];
        assert!(
            oversized_for_a_plugin.len() < MAX_HOST_IMAGE_BYTES as usize,
            "the fixture must sit between the two caps for this test to mean anything"
        );
        cache
            .put(
                "dlsite:image:huge-cover",
                &oversized_for_a_plugin,
                arclain_db::CacheType::Screenshot,
                None,
                None,
            )
            .unwrap();

        assert_eq!(
            read_host_image(&cache, "dlsite:image:huge-cover").unwrap(),
            oversized_for_a_plugin
        );
    }

    /// Serves one PNG over loopback so a fetch has something real to
    /// fetch. Answers only a complete request head, so a stray connect
    /// from a concurrent test binary is not mistaken for a request.
    fn image_stub(body: Vec<u8>) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the image stub");
        let address = listener.local_addr().expect("read the stub address");
        let server = std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut chunk = [0_u8; 512];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match socket.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&chunk[..read]),
                }
            }
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len(),
            );
            let _ = socket.write_all(header.as_bytes());
            let _ = socket.write_all(&body);
            let _ = socket.flush();
        });
        (address, server)
    }

    /// The fetch-and-cache half of the two-cap split: a host image larger
    /// than the plugin cap but within the host cap must fetch, cache, and
    /// read back.
    ///
    /// This is the path that decides whether a broken asset can *heal* --
    /// a renderer that finds such an image missing refetches it, and a
    /// write refusing everything over the plugin cap would make that
    /// refetch fail forever instead of restoring the image. The read half
    /// is pinned by
    /// `a_host_image_larger_than_the_plugin_cap_still_round_trips`.
    ///
    /// Deliberately a unit test against [`test_content_cache`] rather than
    /// a bootstrapped application: `bootstrap` resolves the cache root from
    /// the real per-user cache directory, ignoring `paths_override`, so
    /// every bootstrapped test shares one physical blob store and a peer's
    /// reconciliation can delete this blob between the write and the read.
    /// That made the bootstrapped version of this test fail deterministically
    /// at 2, 4 and 8 test threads. The property under test is the size cap,
    /// which needs no bootstrap at all -- so it is tested where the cache
    /// root is this test's own.
    #[test]
    fn fetch_host_image_accepts_a_body_between_the_plugin_and_host_caps() {
        let (_root, cache) = test_content_cache();
        let body = png_fixture(MAX_PLUGIN_IMAGE_BYTES as usize + 1);
        assert!(
            body.len() < MAX_HOST_IMAGE_BYTES as usize,
            "the fixture must sit between the two caps for this test to mean anything"
        );
        let (address, server) = image_stub(body.clone());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let http = arclain_network::AsyncHttpClient::new(
            runtime.handle().clone(),
            Arc::new(parking_lot::RwLock::new(
                arclain_network::DomainWhitelist::default(),
            )),
            None,
        );

        let fetched = fetch_host_image(
            &cache,
            &http,
            "dlsite:image:cap-band",
            &format!("http://{address}/cover.png"),
            None,
        )
        .expect("a host image over the plugin cap must still fetch");

        assert_eq!(fetched.bytes.len(), body.len());
        assert!(!fetched.served_from_cache);
        assert_eq!(
            read_host_image(&cache, "dlsite:image:cap-band")
                .expect("and must read back afterwards")
                .len(),
            body.len()
        );
        server.join().expect("the stub thread must not panic");
    }

    /// A body that clears the fetch path's minimum-size floor.
    fn png_fixture(len: usize) -> Vec<u8> {
        let mut body = b"\x89PNG\r\n\x1a\n".to_vec();
        body.resize(len, 0x5A);
        body
    }

    /// The read half of the host cap: an entry already on disk that
    /// exceeds it is refused rather than materialized.
    #[test]
    fn read_host_image_rejects_an_asset_over_the_byte_cap() {
        let (_root, cache) = test_content_cache();
        let oversized = vec![0u8; MAX_HOST_IMAGE_BYTES as usize + 1];
        cache
            .put(
                "huge",
                &oversized,
                arclain_db::CacheType::Screenshot,
                None,
                None,
            )
            .unwrap();

        let error = read_host_image(&cache, "huge").unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::Internal);
    }

    #[test]
    fn read_plugin_image_rejects_an_asset_over_the_byte_cap() {
        let (_root, cache) = test_content_cache();
        let oversized = vec![0u8; MAX_PLUGIN_IMAGE_BYTES as usize + 1];
        cache
            .put_for_owner(
                &arclain_data::CacheOwner::plugin("demo-plugin"),
                "huge",
                &oversized,
                arclain_db::CacheType::Cover,
                None,
                None,
            )
            .unwrap();
        let encoded = encode_plugin_image_cache_key("demo-plugin", "huge");

        let error = read_plugin_image(&cache, &encoded).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::Internal);
    }

    #[test]
    fn show_toast_converts_into_a_bounded_host_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::ShowToast {
                message: "done".to_string(),
                level: wirt::ToastLevel::Success,
            },
            "demo",
            &mut intents,
            &mut outcome,
        );
        assert_eq!(
            intents,
            vec![PluginHostIntentDto::ShowToast {
                message: "done".to_string(),
                level: PluginToastLevelDto::Success,
            }]
        );
    }

    #[test]
    fn close_dialog_converts_into_a_bounded_host_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::CloseDialog,
            "demo",
            &mut intents,
            &mut outcome,
        );
        assert_eq!(intents, vec![PluginHostIntentDto::CloseDialog]);
        assert!(!outcome.needs_refresh);
    }

    #[test]
    fn copy_to_clipboard_converts_into_a_bounded_host_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::CopyToClipboard {
                text: "clip me".to_string(),
            },
            "demo",
            &mut intents,
            &mut outcome,
        );
        assert_eq!(
            intents,
            vec![PluginHostIntentDto::CopyToClipboard {
                text: "clip me".to_string(),
            }]
        );
    }

    #[test]
    fn set_page_display_name_converts_into_a_bounded_host_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::SetPageDisplayName {
                name: "New Title".to_string(),
            },
            "demo",
            &mut intents,
            &mut outcome,
        );
        assert_eq!(
            intents,
            vec![PluginHostIntentDto::SetPageDisplayName {
                name: "New Title".to_string(),
            }]
        );
    }

    /// Regression test for the review finding that `OpenLightbox` images
    /// left their `cache_key` raw (unlike every other image-bearing node
    /// `rewrite_cache_keys` encodes at normalization time), so every
    /// lightbox image would fail `read_plugin_image` with `NotFound`.
    /// Proves the full round trip: the intent's encoded key resolves
    /// through `read_plugin_image` to the exact bytes the plugin's own
    /// namespace has cached.
    #[test]
    fn open_lightbox_encodes_cache_keys_and_they_resolve_through_read_plugin_image() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            wirt::PluginAction::OpenLightbox {
                images: vec![
                    (
                        "cover:1".to_string(),
                        Some("https://example.invalid/1".to_string()),
                    ),
                    ("cover:2".to_string(), None),
                ],
                start_index: 1,
                title: Some("Gallery".to_string()),
            },
            "demo-plugin",
            &mut intents,
            &mut outcome,
        );

        let PluginHostIntentDto::OpenLightbox {
            images,
            start_index,
            title,
        } = &intents[0]
        else {
            panic!("expected an OpenLightbox intent");
        };
        assert_eq!(*start_index, 1);
        assert_eq!(title.as_deref(), Some("Gallery"));
        assert_eq!(
            images[0].cache_key,
            encode_plugin_image_cache_key("demo-plugin", "cover:1")
        );
        assert_eq!(images[0].url.as_deref(), Some("https://example.invalid/1"));
        assert_eq!(
            images[1].cache_key,
            encode_plugin_image_cache_key("demo-plugin", "cover:2")
        );

        // Round-trip: the exact encoded key the intent carries must
        // resolve back through `read_plugin_image` to the bytes cached
        // under the plugin's own namespace.
        let (_root, cache) = test_content_cache();
        let bytes = vec![0xCD_u8; 256];
        cache
            .put_for_owner(
                &arclain_data::CacheOwner::plugin("demo-plugin"),
                "cover:1",
                &bytes,
                arclain_db::CacheType::Cover,
                None,
                None,
            )
            .unwrap();
        let resolved = read_plugin_image(&cache, &images[0].cache_key).unwrap();
        assert_eq!(resolved, bytes);
    }
}

/// Domain-access surface tests: the whitelist read model, the URL
/// analysis mirror, and the serde shape of both. Kept in their own module
/// rather than appended to `tests` above for the same
/// minimal-merge-surface reason the source section is delimited.
#[cfg(test)]
mod domain_access_tests {
    use super::*;
    use arclain_network::features::security::DomainWarning;
    use arclain_network::features::whitelist::DomainWhitelist;

    /// The URL `crates/network`'s own `test_full_analysis_phishing`
    /// exercises: a lookalike subdomain chain on an abused free TLD.
    const PHISHING_URL: &str = "https://secure-login.google.com.evil.tk/verify";

    /// One value of every `DomainWarning` variant. The exhaustive `match`
    /// below has no wildcard arm, so adding a ninth variant upstream
    /// fails to compile here until this list grows too -- which is the
    /// whole point of a mirror type.
    fn every_source_warning() -> Vec<DomainWarning> {
        let all = vec![
            DomainWarning::HomographDetected {
                suspicious_char: '\u{0430}',
                position: 1,
                looks_like: 'a',
            },
            DomainWarning::SuspiciousSubdomain {
                subdomain: "google.com".to_string(),
                looks_like: "google.com".to_string(),
            },
            DomainWarning::UnusualTld {
                tld: "tk".to_string(),
            },
            DomainWarning::IpAddress {
                ip: "203.0.113.7".to_string(),
            },
            DomainWarning::LocalhostOrPrivate,
            DomainWarning::SuspiciousEncoding,
            DomainWarning::ExcessiveSubdomains { count: 5 },
            DomainWarning::SuspiciousKeywords {
                keywords: vec!["secure-".to_string(), "-login".to_string()],
            },
        ];
        for warning in &all {
            match warning {
                DomainWarning::HomographDetected { .. }
                | DomainWarning::SuspiciousSubdomain { .. }
                | DomainWarning::UnusualTld { .. }
                | DomainWarning::IpAddress { .. }
                | DomainWarning::LocalhostOrPrivate
                | DomainWarning::SuspiciousEncoding
                | DomainWarning::ExcessiveSubdomains { .. }
                | DomainWarning::SuspiciousKeywords { .. } => {}
            }
        }
        all
    }

    #[test]
    fn analyze_url_mirrors_the_network_analysis_field_for_field() {
        let source = arclain_network::features::security::analyze_url(PHISHING_URL)
            .expect("the network analysis itself must succeed");
        let dto = analyze_url(PHISHING_URL).expect("the mirrored analysis must succeed");

        assert_eq!(dto.full_url, source.full_url);
        assert_eq!(dto.effective_domain, source.effective_domain);
        assert_eq!(dto.host, source.host);
        assert_eq!(dto.tld, source.tld);
        assert_eq!(dto.warnings.len(), source.warnings.len());
        for (mirrored, original) in dto.warnings.iter().zip(source.warnings.iter()) {
            assert_eq!(mirrored, &DomainWarningDto::from(original.clone()));
            assert_eq!(mirrored.description(), original.description());
            assert_eq!(mirrored.is_critical(), original.is_critical());
        }
    }

    #[test]
    fn analyze_url_reports_the_known_warning_case_rather_than_an_empty_list() {
        let dto = analyze_url(PHISHING_URL).expect("analysis must succeed");

        assert_eq!(dto.effective_domain, "evil.tk");
        assert!(
            dto.warnings.iter().any(|warning| matches!(
                warning,
                DomainWarningDto::UnusualTld { tld } if tld == "tk"
            )),
            "expected the abused-TLD warning, got {:?}",
            dto.warnings
        );
    }

    #[test]
    fn analyze_url_reports_a_clean_url_with_no_warnings() {
        let dto = analyze_url("https://dlsite.com/product/123").expect("analysis must succeed");

        assert_eq!(dto.effective_domain, "dlsite.com");
        assert_eq!(dto.host, "dlsite.com");
        assert!(dto.warnings.is_empty(), "{:?}", dto.warnings);
    }

    #[test]
    fn analyze_url_rejects_an_unparsable_url_as_invalid_input() {
        let error = analyze_url("not a url at all").expect_err("a hostless string cannot analyze");

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("url"));
        assert!(error.diagnostic.is_some());
    }

    #[test]
    fn every_domain_warning_variant_mirrors_and_round_trips_through_serde() {
        for source in every_source_warning() {
            let dto = DomainWarningDto::from(source.clone());
            assert_eq!(dto.description(), source.description());
            assert_eq!(dto.is_critical(), source.is_critical());

            let json = serde_json::to_string(&dto).expect("serialize warning");
            let restored: DomainWarningDto =
                serde_json::from_str(&json).expect("deserialize warning");
            assert_eq!(restored, dto, "round trip changed {json}");
        }
    }

    #[test]
    fn domain_analysis_dto_round_trips_through_serde() {
        let dto = DomainAnalysisDto {
            full_url: PHISHING_URL.to_string(),
            effective_domain: "evil.tk".to_string(),
            host: "secure-login.google.com.evil.tk".to_string(),
            tld: "tk".to_string(),
            warnings: every_source_warning()
                .into_iter()
                .map(DomainWarningDto::from)
                .collect(),
        };

        let json = serde_json::to_string(&dto).expect("serialize analysis");
        let restored: DomainAnalysisDto =
            serde_json::from_str(&json).expect("deserialize analysis");

        assert_eq!(restored, dto);
    }

    #[test]
    fn domain_whitelist_entry_dto_round_trips_through_serde() {
        let dto = DomainWhitelistEntryDto {
            plugin_id: "demo-plugin".to_string(),
            domain: "dlsite.com".to_string(),
            approved: true,
        };

        let json = serde_json::to_string(&dto).expect("serialize entry");
        let restored: DomainWhitelistEntryDto =
            serde_json::from_str(&json).expect("deserialize entry");

        assert_eq!(restored, dto);
    }

    #[test]
    fn plugin_domain_whitelist_returns_only_the_named_plugin_in_domain_order() {
        let whitelist = DomainWhitelist::new();
        whitelist.add_pending("wanted", "zeta.example");
        whitelist.add_pending("wanted", "alpha.example");
        whitelist.approve("wanted", "middle.example");
        whitelist.add_pending("other", "unrelated.example");

        let entries = plugin_domain_whitelist(&whitelist, "wanted").expect("read the whitelist");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.domain.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha.example", "middle.example", "zeta.example"],
        );
        assert!(entries.iter().all(|entry| entry.plugin_id == "wanted"));
    }

    #[test]
    fn plugin_domain_whitelist_reports_approval_state_per_domain() {
        let whitelist = DomainWhitelist::new();
        whitelist.add_pending("demo", "pending.example");
        whitelist.approve("demo", "approved.example");
        whitelist.replace_manifest_domains("demo", ["manifest.example"]);

        let entries = plugin_domain_whitelist(&whitelist, "demo").expect("read the whitelist");

        let approval = |domain: &str| {
            entries
                .iter()
                .find(|entry| entry.domain == domain)
                .unwrap_or_else(|| panic!("{domain} missing from {entries:?}"))
                .approved
        };
        assert!(!approval("pending.example"));
        assert!(approval("approved.example"));
        assert!(
            approval("manifest.example"),
            "a manifest grant is an approval the plugin can already use",
        );
    }

    #[test]
    fn plugin_domain_whitelist_reports_an_unknown_plugin_as_having_no_domains() {
        let whitelist = DomainWhitelist::new();
        whitelist.add_pending("someone-else", "dlsite.com");

        let entries = plugin_domain_whitelist(&whitelist, "never-asked").expect("read");

        assert!(entries.is_empty());
    }

    #[test]
    fn plugin_domain_whitelist_rejects_a_blank_plugin_id() {
        let whitelist = DomainWhitelist::new();

        let error = plugin_domain_whitelist(&whitelist, "   ")
            .expect_err("a blank plugin id is a caller bug, not an empty result");

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("plugin_id"));
    }
}

/// The mirror-fidelity half of the plugin chrome / network-log surface:
/// everything provable without a running plugin manager. The seeded
/// read-back through a real `ArclainApp` (which needs a real WASM guest to
/// register a tab and write a log line) lives in
/// `crates/app/tests/plugin_sessions.rs`.
#[cfg(test)]
mod chrome_and_network_log_tests {
    use super::*;
    use arclain_plugins::manager::PluginStatusSummary;
    use arclain_plugins::types::{BadgeConfig, PluginCapability, TopTabConfig};

    /// One value of every `PluginCapability` variant. The exhaustive
    /// `match` has no wildcard arm, so a seventh capability upstream fails
    /// to compile here until this list grows too -- the same drift guard
    /// `domain_access_tests::every_source_warning` uses.
    fn every_source_capability() -> Vec<PluginCapability> {
        let all = vec![
            PluginCapability::FileRead,
            PluginCapability::FileWrite,
            PluginCapability::Network,
            PluginCapability::ArchiveMetadataRead,
            PluginCapability::ArchiveMetadataWrite,
            PluginCapability::ArchiveModify,
        ];
        for capability in &all {
            match capability {
                PluginCapability::FileRead
                | PluginCapability::FileWrite
                | PluginCapability::Network
                | PluginCapability::ArchiveMetadataRead
                | PluginCapability::ArchiveMetadataWrite
                | PluginCapability::ArchiveModify => {}
            }
        }
        all
    }

    #[test]
    fn every_capability_mirrors_to_a_distinct_dto_variant() {
        let source = every_source_capability();
        let mirrored: Vec<PluginCapabilityDto> = source
            .iter()
            .copied()
            .map(PluginCapabilityDto::from)
            .collect();

        let unique: std::collections::BTreeSet<PluginCapabilityDto> =
            mirrored.iter().copied().collect();
        assert_eq!(
            unique.len(),
            source.len(),
            "two source capabilities collapsed onto one mirrored variant: {mirrored:?}",
        );
    }

    /// Pins the exact rendering the pre-facade plugins page produced with
    /// `format!("{:?}", capability)`, so adopting [`PluginCapabilityDto`]
    /// in the permissions list is a rename, not a visible change.
    #[test]
    fn capability_labels_match_the_pre_facade_debug_spelling() {
        for capability in every_source_capability() {
            assert_eq!(
                PluginCapabilityDto::from(capability).label(),
                format!("{capability:?}"),
            );
        }
    }

    #[test]
    fn capability_serializes_under_its_own_stable_vocabulary() {
        assert_eq!(
            serde_json::to_string(&PluginCapabilityDto::ArchiveMetadataWrite).unwrap(),
            "\"archive_metadata_write\"",
        );
    }

    #[test]
    fn status_summary_mirrors_its_source_field_for_field() {
        let dto = PluginStatusSummaryDto::from(PluginStatusSummary {
            total: 7,
            enabled: 3,
        });

        assert_eq!(
            dto,
            PluginStatusSummaryDto {
                total: 7,
                enabled: 3
            }
        );
    }

    #[test]
    fn badge_mirrors_its_source_field_for_field() {
        let dto = PluginBadgeDto::from(BadgeConfig {
            count: Some(12),
            dot: true,
            color: "red".to_string(),
        });

        assert_eq!(
            dto,
            PluginBadgeDto {
                count: Some(12),
                dot: true,
                color: "red".to_string(),
            }
        );
    }

    /// A badge whose `count` is absent and whose `dot` is unset survives
    /// as-is rather than being normalized away: only a renderer knows
    /// whether "no count, no dot" means "draw nothing".
    #[test]
    fn badge_preserves_an_empty_configuration() {
        let dto = PluginBadgeDto::from(BadgeConfig {
            count: None,
            dot: false,
            color: String::new(),
        });

        assert_eq!(dto.count, None);
        assert!(!dto.dot);
        assert!(dto.color.is_empty());
    }

    fn sample_top_tab() -> TopTabConfig {
        TopTabConfig {
            id: "library".to_string(),
            label: "Library".to_string(),
            icon: "DATABASE".to_string(),
            badge: Some(BadgeConfig {
                count: Some(4),
                dot: false,
                color: "blue".to_string(),
            }),
            priority: 10,
        }
    }

    #[test]
    fn top_tab_mirrors_its_source_field_for_field_and_carries_its_owner() {
        let dto = PluginTopTabDto::from(("rj-metadata".to_string(), sample_top_tab()));

        assert_eq!(
            dto,
            PluginTopTabDto {
                plugin_id: "rj-metadata".to_string(),
                id: "library".to_string(),
                label: "Library".to_string(),
                icon: "DATABASE".to_string(),
                badge: Some(PluginBadgeDto {
                    count: Some(4),
                    dot: false,
                    color: "blue".to_string(),
                }),
                priority: 10,
            }
        );
    }

    #[test]
    fn top_tab_without_a_badge_stays_without_one() {
        let mut source = sample_top_tab();
        source.badge = None;

        let dto = PluginTopTabDto::from(("rj-metadata".to_string(), source));

        assert_eq!(dto.badge, None);
    }

    /// Display text a plugin over-declares is truncated, not dropped --
    /// and truncated on a char boundary, so a multi-byte glyph straddling
    /// the bound cannot produce invalid UTF-8 or panic.
    #[test]
    fn top_tab_display_text_is_bounded_on_a_char_boundary() {
        let overlong = "\u{1f600}".repeat(MAX_PLUGIN_TOP_TAB_TEXT_BYTES);
        let mut source = sample_top_tab();
        source.label = overlong.clone();
        source.icon = overlong;

        let dto = PluginTopTabDto::from(("rj-metadata".to_string(), source));

        assert!(dto.label.len() <= MAX_PLUGIN_TOP_TAB_TEXT_BYTES);
        assert!(dto.icon.len() <= MAX_PLUGIN_TOP_TAB_TEXT_BYTES);
        assert!(
            dto.label.len() > MAX_PLUGIN_TOP_TAB_TEXT_BYTES - 4,
            "the bound must be used, not merely respected",
        );
        assert!(dto.label.chars().all(|glyph| glyph == '\u{1f600}'));
    }

    /// Identity is passed through verbatim even when it is absurd:
    /// truncating a tab id would make two distinct tabs select each
    /// other, which is worse than carrying a long string nobody lays out.
    /// `runtime::bootstrap::sync_plugin_top_tab_items` makes the same
    /// split for the same reason.
    #[test]
    fn top_tab_identity_is_never_truncated() {
        let long_id = "x".repeat(MAX_PLUGIN_TOP_TAB_TEXT_BYTES * 2);
        let mut source = sample_top_tab();
        source.id = long_id.clone();

        let dto = PluginTopTabDto::from(("rj-metadata".to_string(), source));

        assert_eq!(dto.id, long_id);
    }

    #[test]
    fn network_log_entry_mirrors_its_source_pair() {
        let logged_at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_123);

        let dto =
            PluginNetworkLogEntryDto::from((logged_at, "GET https://example.test/api".to_string()));

        assert_eq!(dto.logged_at_unix_ms, 1_700_000_000_123);
        assert_eq!(dto.message, "GET https://example.test/api");
    }

    #[test]
    fn network_log_entry_reports_a_pre_epoch_time_as_negative_millis() {
        let logged_at = std::time::UNIX_EPOCH - std::time::Duration::from_millis(1_500);

        let dto = PluginNetworkLogEntryDto::from((logged_at, "clock skew".to_string()));

        assert_eq!(dto.logged_at_unix_ms, -1_500);
    }

    #[test]
    fn chrome_snapshot_default_is_the_no_plugin_runtime_answer() {
        let empty = PluginChromeSnapshot::default();

        assert_eq!(empty.summary.total, 0);
        assert_eq!(empty.summary.enabled, 0);
        assert!(empty.top_tabs.is_empty());
    }

    #[test]
    fn chrome_snapshot_round_trips_through_serde() {
        let snapshot = PluginChromeSnapshot {
            summary: PluginStatusSummaryDto {
                total: 2,
                enabled: 1,
            },
            top_tabs: vec![PluginTopTabDto::from((
                "rj-metadata".to_string(),
                sample_top_tab(),
            ))],
        };

        let encoded = serde_json::to_string(&snapshot).expect("encode");
        let decoded: PluginChromeSnapshot = serde_json::from_str(&encoded).expect("decode");

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn install_path_must_name_a_wasm_file() {
        let error = validate_install_path(std::path::Path::new("/tmp/plugin.txt"))
            .expect_err("a non-wasm file must be refused before any file is opened");

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("wasm_path"));
        assert!(error.diagnostic.is_some());
    }

    #[test]
    fn package_path_accepts_wirt_case_insensitively_and_rejects_wasm() {
        validate_package_path(std::path::Path::new("/tmp/plugin.WIRT"))
            .expect("mixed-case package extension must be accepted");
        let error = validate_package_path(std::path::Path::new("/tmp/plugin.wasm"))
            .expect_err("loose components are not package install inputs");
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("package_path"));
    }

    #[test]
    fn install_path_accepts_any_case_of_the_wasm_extension() {
        validate_install_path(std::path::Path::new("/tmp/plugin.WASM"))
            .expect("extension matching must not depend on the user's shift key");
    }

    #[test]
    fn install_path_must_not_be_empty() {
        let error = validate_install_path(std::path::Path::new(""))
            .expect_err("an empty path is a caller bug");

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("wasm_path"));
    }

    #[test]
    fn install_path_is_bounded() {
        let absurd = format!(
            "{}.wasm",
            "p".repeat(MAX_PLUGIN_INSTALL_PATH_BYTES.saturating_add(1))
        );

        let error = validate_install_path(std::path::Path::new(&absurd))
            .expect_err("a path no filesystem can hold must not reach the plugin manager");

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("wasm_path"));
        assert_eq!(
            error.path, None,
            "an absurd path must not be echoed back into the envelope",
        );
    }

    /// The user's own path is the only channel that survives: the
    /// diagnostic's path redaction scrubs the filename out of the
    /// backend's wording, so without the `path` field a user would never
    /// learn which file failed.
    #[test]
    fn install_failures_name_the_file_the_caller_chose() {
        let picked = std::path::Path::new("/downloads/rj123456-helper.wasm");

        let rejected_shape = validate_install_path(std::path::Path::new("/downloads/notes.txt"))
            .expect_err("a non-wasm file must be refused");
        let rejected_content = with_install_path(
            plugin_install_error(PluginError::LoadError("File does not exist".to_string())),
            picked,
        );

        assert_eq!(
            rejected_shape.path.as_deref(),
            Some(std::path::Path::new("/downloads/notes.txt")),
        );
        assert_eq!(rejected_content.path.as_deref(), Some(picked));
        assert!(
            !rejected_content
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("rj123456"),
            "the diagnostic must stay path-redacted; `path` is the vetted channel",
        );
    }

    /// The install error envelope: a plugin-side rejection is reported as
    /// a `Plugin` failure the user can act on, with the backend's own
    /// wording preserved (bounded and path-redacted by
    /// `ApplicationError::with_diagnostic`) rather than replaced.
    #[test]
    fn install_failures_carry_a_bounded_diagnostic() {
        let error = plugin_install_error(PluginError::LoadError("x".repeat(16 * 1024)));

        assert_eq!(error.kind, ApplicationErrorKind::Plugin);
        assert_eq!(error.recoverability, Recoverability::UserAction);
        assert_eq!(error.field.as_deref(), Some("wasm_path"));
        let diagnostic = error.diagnostic.expect("a diagnostic must be attached");
        assert!(
            diagnostic.len() <= 4096,
            "diagnostic must respect the 4 KiB envelope bound, got {}",
            diagnostic.len(),
        );
        assert!(diagnostic.ends_with("... [truncated]"));
    }

    #[test]
    fn install_reports_a_malformed_manifest_as_invalid_input() {
        let error = plugin_install_error(PluginError::InvalidManifest("no id".to_string()));

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn install_reports_an_io_failure_as_a_backend_failure() {
        let error = plugin_install_error(PluginError::Io(std::io::Error::other("disk gone")));

        assert_eq!(error.kind, ApplicationErrorKind::Backend);
    }

    #[test]
    fn package_errors_keep_stable_semantic_classes() {
        let cases = [
            (
                PluginError::InvalidPackage("not a package".to_string()),
                ApplicationErrorKind::InvalidInput,
            ),
            (
                PluginError::Unsupported("future ABI".to_string()),
                ApplicationErrorKind::Unsupported,
            ),
            (
                PluginError::Conflict("already installed".to_string()),
                ApplicationErrorKind::Conflict,
            ),
            (
                PluginError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                )),
                ApplicationErrorKind::PermissionDenied,
            ),
            (
                PluginError::Io(std::io::Error::other("disk gone")),
                ApplicationErrorKind::Backend,
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(plugin_package_error(source).kind, expected);
        }
    }
}
