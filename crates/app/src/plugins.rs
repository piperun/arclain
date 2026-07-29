//! Renderer-neutral plugin sessions: the application-facade wrapper
//! around `arclain_plugins`'s host-side plugin manager and its
//! renderer-neutral UI model (`arclain_plugins::ui_model`).
//!
//! `arclain_plugins::ui_model` defines the node/document *shape*
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
//!   `arclain_plugins::ui_model::PluginUiNodeDto::find`.
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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex as SyncMutex, RwLock as SyncRwLock};
use tokio::sync::Mutex as AsyncMutex;

use arclain_plugins::types::{PluginArchiveAccess, PluginArchiveContextId};
use arclain_plugins::ui_model::{self, PluginUiNormalizeError};
use arclain_plugins::{ActiveTabBridge, PluginError, PluginManager};

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
pub use arclain_plugins::ui_model::{
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

/// Rewrites every image-bearing node's cache key reference to the
/// encoded form [`encode_plugin_image_cache_key`] produces, recursing
/// into every container kind. Applied once, right after
/// `arclain_plugins::ui_model::normalize_layout` succeeds -- see
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
    let (plugin_id, raw_key) = decode_plugin_image_cache_key(cache_key).ok_or_else(|| {
        ApplicationError::new(
            ApplicationErrorKind::NotFound,
            "unknown plugin image cache key",
        )
        .with_recoverability(Recoverability::Fatal)
    })?;
    // The key names its own owner, so on its own it is a bearer token for
    // a cache namespace: anyone holding the string `plugin-image:victim:k`
    // could write bytes that `victim` would later render as its own. The
    // caller must independently state which plugin it is acting for, and
    // the two must agree.
    if plugin_id != expected_plugin_id {
        return Err(ApplicationError::new(
            ApplicationErrorKind::PermissionDenied,
            "plugin image cache key belongs to a different plugin",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("cache_key"));
    }
    // Rejects a syntactically decodable but structurally impossible owner
    // before it can mint a cache namespace (with its own quota accounting)
    // that no installed plugin could ever read back.
    if arclain_plugins::types::PluginId::parse(plugin_id).is_err() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "plugin image cache key names a malformed plugin id",
        )
        .with_recoverability(Recoverability::Fatal)
        .with_field("cache_key"));
    }
    if bytes.len() > MAX_PLUGIN_IMAGE_BYTES as usize {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "plugin image exceeds the maximum size",
        )
        .with_diagnostic(format!(
            "{} bytes exceeds the {MAX_PLUGIN_IMAGE_BYTES}-byte limit",
            bytes.len()
        ))
        .with_recoverability(Recoverability::Fatal));
    }
    content_cache
        .put_for_owner(
            &arclain_data::CacheOwner::plugin(plugin_id),
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

// ============================================================================
// Facade-level DTOs (wrap `arclain_plugins::ui_model` shapes with this
// crate's own opaque ids).
// ============================================================================

/// One plugin as reported by [`crate::ArclainApp::plugins`]: enough to
/// render a plugin list/settings row without a caller needing to reach
/// into `arclain_plugins::PluginListItem`/`PluginManifest` directly.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// `Some(reason)` if this plugin was discovered on disk but failed to
    /// load -- see `arclain_plugins::manager::FailedPlugin`. A plugin
    /// reported this way has no running instance and `arclain_plugins`
    /// records only its id and failure reason, not its manifest: `enabled`
    /// is always `false`, and `name`/`version` are always empty strings
    /// rather than whatever the manifest claimed.
    pub load_error: Option<String>,
}

/// A renderer-neutral plugin UI document: [`arclain_plugins::ui_model::
/// PluginUiNodeDto`]'s normalized tree, plus the session/plugin/revision
/// identity `arclain_plugins::ui_model` itself cannot carry (it has no
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
    pub(crate) fn plugins(manager: &SyncMutex<PluginManager>) -> Vec<PluginSummary> {
        let manager = manager.lock();
        let mut summaries: Vec<PluginSummary> = manager
            .list_plugins()
            .into_iter()
            .map(|item| PluginSummary {
                id: item.id,
                name: item.manifest.plugin.name,
                version: item.manifest.plugin.version,
                enabled: item.enabled,
                load_error: None,
            })
            .collect();
        summaries.extend(
            manager
                .failed_plugins()
                .into_iter()
                .map(|failed| PluginSummary {
                    id: failed.original_id,
                    name: String::new(),
                    version: String::new(),
                    enabled: false,
                    load_error: Some(failed.error),
                }),
        );
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
        result.map_err(|_| plugin_not_found(plugin_id))
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
        let root = self
            .fetch_and_normalize(&manager, &plugin_id, &extension_point, handle)
            .await?;

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
    pub(crate) fn document(
        &self,
        session_id: PluginSessionId,
    ) -> Result<PluginUiDocument, ApplicationError> {
        self.sessions
            .read()
            .get(&session_id)
            .map(|record| record.document(session_id))
            .ok_or_else(|| unknown_plugin_session(session_id))
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
    /// `arclain_plugins::ui_model::PluginUiNodeDto::find`'s doc comment.
    pub(crate) async fn dispatch_action(
        &self,
        manager: Arc<SyncMutex<PluginManager>>,
        request: PluginActionRequest,
        handle: &tokio::runtime::Handle,
    ) -> Result<PluginUiUpdate, ApplicationError> {
        let (plugin_id, extension_point, pinned_archive_session) = {
            let sessions = self.sessions.read();
            let record = sessions
                .get(&request.session_id)
                .ok_or_else(|| unknown_plugin_session(request.session_id))?;
            if let Some(target) = record.root.find(&request.node_id) {
                if !target.visible || !target.enabled {
                    return Err(action_rejected(&request.node_id));
                }
            }
            (
                record.plugin_id.clone(),
                record.extension_point.clone(),
                record.pinned_archive_session,
            )
        };

        let plugin_lock = self.lock_for_plugin(&plugin_id);
        let _guard = plugin_lock.lock().await;

        let event_id = request.node_id.clone();
        let value = match request.action {
            PluginActionDto::Activate => None,
            PluginActionDto::SetValue { value } => value,
        };
        let dispatch_manager = manager.clone();
        let dispatch_plugin_id = plugin_id.clone();
        let actions = handle
            .spawn_blocking(move || {
                let instance = dispatch_manager
                    .lock()
                    .get_plugin_instance(&dispatch_plugin_id)
                    .ok_or_else(|| plugin_not_found(&dispatch_plugin_id))?;
                let mut instance = instance.lock();
                instance
                    .send_ui_event(&event_id, value)
                    .map_err(plugin_execution_error)
            })
            .await
            .map_err(|join_error| {
                ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "plugin action worker failed",
                )
                .with_diagnostic(join_error.to_string())
            })??;

        let bounded = arclain_plugins::action_policy::bound_plugin_actions(actions);
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

        let refreshed_root = if outcome.needs_refresh {
            Some(
                self.fetch_and_normalize(&manager, &plugin_id, &extension_point, handle)
                    .await?,
            )
        } else {
            None
        };

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
        let manager = manager.clone();
        let worker_plugin_id = plugin_id.to_string();
        let host_extension_point = to_host_extension_point(extension_point);
        let layout = handle
            .spawn_blocking(move || {
                let instance = manager
                    .lock()
                    .get_plugin_instance(&worker_plugin_id)
                    .ok_or_else(|| plugin_not_found(&worker_plugin_id))?;
                let mut instance = instance.lock();
                instance
                    .get_ui_layout(host_extension_point)
                    .map_err(plugin_execution_error)
            })
            .await
            .map_err(|join_error| {
                ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "plugin layout worker failed",
                )
                .with_diagnostic(join_error.to_string())
            })??;
        let root = ui_model::normalize_layout(&layout).map_err(normalize_error)?;
        Ok(rewrite_cache_keys(root, plugin_id))
    }
}

fn to_host_extension_point(
    extension_point: &PluginExtensionPointDto,
) -> arclain_plugins::types::PluginExtensionPoint {
    use arclain_plugins::types::PluginExtensionPoint as HostExtensionPoint;
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
    let Some(instance) = manager.lock().get_plugin_instance(plugin_id) else {
        tracing::warn!(plugin_id, "dropped a RequestFetch for an unknown plugin");
        return;
    };
    let pinned = pinned_archive_session.map(ArchiveSessionId::into_raw);
    for key in keys {
        let instance = instance.clone();
        let worker_plugin_id = plugin_id.to_string();
        let outcome = handle
            .spawn_blocking(move || {
                arclain_plugins::resolve_interactive_request_fetch(
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

/// Applies one bounded (`arclain_plugins::action_policy`-passed)
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
    action: arclain_plugins::types::PluginAction,
    plugin_id: &str,
    intents: &mut Vec<PluginHostIntentDto>,
    outcome: &mut BoundedActionOutcome,
) {
    use arclain_plugins::types::PluginAction;

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
    use arclain_plugins::types::{PluginLayout, PluginUiElement};
    use arclain_plugins::ui_model::PluginToastLevelDto;
    use std::collections::HashMap as StdHashMap;
    use std::sync::Mutex as StdMutex;

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

    fn test_content_cache() -> (tempfile::TempDir, arclain_data::ContentCache) {
        let root = tempfile::tempdir().unwrap();
        let cache = arclain_data::ContentCache::new(
            root.path().join("cache"),
            Arc::new(InMemoryCacheIndex::default()),
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

    #[test]
    fn refresh_panel_sets_the_refresh_flag_and_is_not_surfaced_as_an_intent() {
        let mut intents = Vec::new();
        let mut outcome = BoundedActionOutcome::default();
        apply_bounded_action(
            arclain_plugins::types::PluginAction::RefreshPanel {
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
            arclain_plugins::types::PluginAction::RequestFetch {
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
                arclain_plugins::types::PluginAction::RequestFetch {
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
            arclain_plugins::types::PluginAction::ShowToast {
                message: "done".to_string(),
                level: arclain_plugins::types::ToastLevel::Success,
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
            arclain_plugins::types::PluginAction::CloseDialog,
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
            arclain_plugins::types::PluginAction::CopyToClipboard {
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
            arclain_plugins::types::PluginAction::SetPageDisplayName {
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
            arclain_plugins::types::PluginAction::OpenLightbox {
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
