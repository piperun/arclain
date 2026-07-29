//! Unified asynchronous cached-image loading, decoding, and texture ownership.
//!
//! Cached bytes and image decoding stay on the Tokio blocking pool. Renderers
//! only observe this store, upload a ready RGBA buffer on the UI thread, and
//! clone the resulting shared texture handle.

use crate::core::tabs::TabId;
use arclain_core::{CacheType, ContentCache};
use eframe::egui;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Prefix `arclain_app`'s normalizer stamps onto every image reference a
/// plugin document carries. Matched here rather than imported because the
/// facade deliberately keeps the codec private -- `read_plugin_image` /
/// `write_plugin_image` are the only supported ways to resolve one, and
/// this module's whole job is to route those keys to them.
const PLUGIN_IMAGE_KEY_PREFIX: &str = "plugin-image:";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageOwner {
    PluginPage {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
    },
    PluginDialog {
        plugin_id: String,
        dialog_id: String,
        origin_tab: TabId,
    },
    PluginPanel {
        plugin_id: String,
        panel_id: String,
        origin_tab: TabId,
    },
    PluginSettings {
        plugin_id: String,
    },
    Lightbox {
        tab: TabId,
        /// The plugin whose action opened this lightbox, if any. Carried
        /// so a lightbox is ownership-checked like every other
        /// plugin-scoped surface rather than being the one hole in
        /// [`image_key_is_addressable_by`]'s coverage.
        plugin_id: Option<String>,
    },
}

impl ImageOwner {
    pub fn plugin_page(
        plugin_id: impl Into<String>,
        page_id: impl Into<String>,
        origin_tab: TabId,
    ) -> Self {
        Self::PluginPage {
            plugin_id: plugin_id.into(),
            page_id: page_id.into(),
            origin_tab,
        }
    }

    pub fn plugin_dialog(
        plugin_id: impl Into<String>,
        dialog_id: impl Into<String>,
        origin_tab: TabId,
    ) -> Self {
        Self::PluginDialog {
            plugin_id: plugin_id.into(),
            dialog_id: dialog_id.into(),
            origin_tab,
        }
    }

    pub fn plugin_panel(
        plugin_id: impl Into<String>,
        panel_id: impl Into<String>,
        origin_tab: TabId,
    ) -> Self {
        Self::PluginPanel {
            plugin_id: plugin_id.into(),
            panel_id: panel_id.into(),
            origin_tab,
        }
    }

    pub fn plugin_settings(plugin_id: impl Into<String>) -> Self {
        Self::PluginSettings {
            plugin_id: plugin_id.into(),
        }
    }

    /// Which plugin's surface this owner represents, when it represents
    /// one. `None` for [`Self::Lightbox`], which is a host overlay that
    /// can be populated from any plugin's intent.
    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            Self::PluginPage { plugin_id, .. }
            | Self::PluginDialog { plugin_id, .. }
            | Self::PluginPanel { plugin_id, .. }
            | Self::PluginSettings { plugin_id } => Some(plugin_id),
            Self::Lightbox { plugin_id, .. } => plugin_id.as_deref(),
        }
    }
}

/// Whether `key` is one the application facade stamped with an owning
/// plugin (`plugin-image:{owner}:{key}`), reserving it to the host.
///
/// A plugin's cache namespace is an isolation boundary this codebase
/// enforces deliberately, and a key encodes its own owner -- so an
/// unvalidated key is a bearer token for whatever namespace it names.
/// Only the host ever produces this prefix (during normalization, from a
/// plugin id it already knows); a value carrying it that did *not* come
/// from there is a forgery attempt.
pub fn is_host_owned_image_key(key: &str) -> bool {
    key.starts_with(PLUGIN_IMAGE_KEY_PREFIX)
}

/// The plugin a host-stamped key names as its owner, or `None` for any
/// other key.
pub fn host_owned_image_key_owner(key: &str) -> Option<&str> {
    key.strip_prefix(PLUGIN_IMAGE_KEY_PREFIX)?
        .split_once(':')
        .map(|(plugin_id, _)| plugin_id)
}

/// Whether a surface belonging to `acting_plugin_id` may address `key`.
///
/// The single rule behind both choke points below
/// ([`ImageAssetStore::request`] for reads,
/// `crate::shared::image_fetcher::trigger_image_fetch` for writes): a
/// host-stamped key may only be used by the plugin it names. Unstamped
/// keys are host-namespace and unrestricted, exactly as before.
///
/// `acting_plugin_id` is `None` only for a surface with no owning plugin
/// at all -- a host-opened lightbox, which carries no plugin-authored
/// keys, so there is nothing to compare against.
pub fn image_key_is_addressable_by(key: &str, acting_plugin_id: Option<&str>) -> bool {
    match (host_owned_image_key_owner(key), acting_plugin_id) {
        (None, _) => true,
        (Some(owner), Some(acting)) => owner == acting,
        (Some(_), None) => true,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageAssetState {
    Loading,
    Decoded,
    Uploaded,
    Failed(String),
}

/// Where this store reads image bytes from, removes them from, and --
/// critically -- writes them back to.
///
/// `put` is on the *same* trait as `get` deliberately. A URL-fallback
/// fetch and the read that asked for it must agree on which cache
/// namespace the key belongs to; when they were separate code paths (a
/// `ContentCache::put` at the fetch site, a namespace-decoding read here)
/// they silently disagreed for plugin-owned keys, producing an entry the
/// read could never find and a recovery loop that could never succeed.
/// Routing both through one implementation makes that class of mismatch
/// unrepresentable.
trait ImageBytes: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    /// `plugin_id` is the plugin whose document referenced `key`, as the
    /// *host* knows it -- never anything the plugin supplied. Sources that
    /// can write into a per-plugin namespace check the key's own claimed
    /// owner against it; see `FacadeImageBytes::put`.
    fn put(
        &self,
        plugin_id: Option<&str>,
        key: &str,
        bytes: &[u8],
        source_url: Option<&str>,
    ) -> anyhow::Result<()>;
    fn remove(&self, key: &str);
    /// Whether a successful [`Self::put`] for `key` would actually retain
    /// anything. `false` lets callers skip work whose result has nowhere
    /// to land -- see `crate::shared::image_fetcher::trigger_image_fetch`.
    fn can_store(&self, key: &str) -> bool;
}

struct ContentCacheBytes(Arc<ContentCache>);

impl ImageBytes for ContentCacheBytes {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.0.get(key)
    }

    fn put(
        &self,
        _plugin_id: Option<&str>,
        key: &str,
        bytes: &[u8],
        source_url: Option<&str>,
    ) -> anyhow::Result<()> {
        self.0
            .put(key, bytes, CacheType::Screenshot, None, source_url)
            .map(|_| ())
    }

    fn remove(&self, key: &str) {
        if let Err(error) = self.0.remove(key) {
            tracing::warn!(%error, %key, "failed to remove corrupt cached image");
        }
    }

    fn can_store(&self, _key: &str) -> bool {
        true
    }
}

/// Resolves plugin-document image references through the application
/// facade and everything else through the content cache.
///
/// `read_plugin_image` decodes the owning plugin out of the key and reads
/// the bytes from that plugin's own cache namespace under the facade's
/// per-asset size cap. A bare content-cache read cannot do either: the
/// namespace is not recoverable from the key, and the cap is the facade's
/// to enforce.
struct FacadeImageBytes {
    facade: arclain_app::ArclainApp,
    runtime: Arc<tokio::runtime::Runtime>,
    fallback: Option<Arc<ContentCache>>,
}

impl ImageBytes for FacadeImageBytes {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !key.starts_with(PLUGIN_IMAGE_KEY_PREFIX) {
            return match &self.fallback {
                Some(cache) => cache.get(key),
                None => Ok(None),
            };
        }
        // Runs on `ImageAssetStore`'s own blocking pool (see
        // `spawn_load_inner`), never on a worker thread, so blocking on
        // the facade future here cannot stall the runtime. The facade's
        // futures are executor-agnostic by contract, so awaiting one from
        // this crate's runtime rather than the application's own is
        // supported.
        match self
            .runtime
            .block_on(self.facade.read_plugin_image(key.to_string()))
        {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind == arclain_app::error::ApplicationErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(anyhow::anyhow!(error.summary)),
        }
    }

    /// Writes a URL-fallback fetch back into the *plugin's* namespace,
    /// the only namespace [`Self::get`] reads plugin keys from. A
    /// host-namespace write here would be invisible to that read, so the
    /// asset would render broken and re-fetch every 30 s forever, leaving
    /// one orphaned host cache entry per attempt.
    ///
    /// `plugin_id` is the host's own knowledge of which plugin's document
    /// referenced this key, and is passed to the facade so it can refuse a
    /// key claiming a *different* owner. Without it, a key is a bearer
    /// token for a cache namespace: any caller holding a
    /// `plugin-image:victim:k` string could write into `victim`'s
    /// namespace and have `victim` later render those bytes as its own.
    /// The frontend-side half of this guard is the two choke points every
    /// image path funnels through -- [`ImageAssetStore::request`] for
    /// reads and `crate::shared::image_fetcher::trigger_image_fetch` for
    /// fetches. This is the facade-side half, so neither alone has to be
    /// perfect.
    fn put(
        &self,
        plugin_id: Option<&str>,
        key: &str,
        bytes: &[u8],
        source_url: Option<&str>,
    ) -> anyhow::Result<()> {
        if !key.starts_with(PLUGIN_IMAGE_KEY_PREFIX) {
            return match &self.fallback {
                Some(cache) => cache
                    .put(key, bytes, CacheType::Screenshot, None, source_url)
                    .map(|_| ()),
                None => Ok(()),
            };
        }
        let Some(plugin_id) = plugin_id else {
            anyhow::bail!(
                "refusing to write a plugin-namespaced image with no owning plugin known"
            );
        };
        self.runtime
            .block_on(self.facade.write_plugin_image(
                plugin_id.to_string(),
                key.to_string(),
                bytes.to_vec(),
                source_url.map(str::to_string),
            ))
            .map_err(|error| anyhow::anyhow!(error.summary))
    }

    fn remove(&self, key: &str) {
        // A plugin-owned entry is not this frontend's to evict: the
        // facade owns that namespace, and a corrupt entry there is
        // re-fetched by the plugin, not by us. Only fall-through keys
        // (host-owned cache entries) are evictable here, matching
        // `ContentCacheBytes`'s behavior for exactly those keys.
        if key.starts_with(PLUGIN_IMAGE_KEY_PREFIX) {
            return;
        }
        let Some(cache) = &self.fallback else {
            return;
        };
        if let Err(error) = cache.remove(key) {
            tracing::warn!(%error, %key, "failed to remove corrupt cached image");
        }
    }

    /// A plugin key always has somewhere to go (the facade owns that
    /// namespace, independent of this frontend's own cache); anything else
    /// needs the host cache to exist.
    fn can_store(&self, key: &str) -> bool {
        key.starts_with(PLUGIN_IMAGE_KEY_PREFIX) || self.fallback.is_some()
    }
}

struct EmptyImageBytes;

impl ImageBytes for EmptyImageBytes {
    fn get(&self, _key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    /// Silently discarded rather than an error: this source exists for
    /// contexts with no cache at all (early init, hand-built test
    /// fixtures), where a fetch having nowhere to land is expected, not a
    /// failure the caller should surface. Callers avoid reaching here at
    /// all -- see `crate::shared::image_fetcher::trigger_image_fetch`'s
    /// storability guard.
    fn put(
        &self,
        _plugin_id: Option<&str>,
        _key: &str,
        _bytes: &[u8],
        _source_url: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, _key: &str) {}

    fn can_store(&self, _key: &str) -> bool {
        false
    }
}

enum ImageAsset {
    Loading {
        generation: u64,
        restart_pending: bool,
        owners: HashSet<ImageOwner>,
    },
    Decoded {
        size: [usize; 2],
        rgba: Vec<u8>,
        owners: HashSet<ImageOwner>,
    },
    Uploaded {
        texture: egui::TextureHandle,
        owners: HashSet<ImageOwner>,
    },
    Failed {
        message: String,
        owners: HashSet<ImageOwner>,
    },
}

impl ImageAsset {
    fn owners(&self) -> &HashSet<ImageOwner> {
        match self {
            Self::Loading { owners, .. }
            | Self::Decoded { owners, .. }
            | Self::Uploaded { owners, .. }
            | Self::Failed { owners, .. } => owners,
        }
    }

    fn owners_mut(&mut self) -> &mut HashSet<ImageOwner> {
        match self {
            Self::Loading { owners, .. }
            | Self::Decoded { owners, .. }
            | Self::Uploaded { owners, .. }
            | Self::Failed { owners, .. } => owners,
        }
    }

    fn state(&self) -> ImageAssetState {
        match self {
            Self::Loading { .. } => ImageAssetState::Loading,
            Self::Decoded { .. } => ImageAssetState::Decoded,
            Self::Uploaded { .. } => ImageAssetState::Uploaded,
            Self::Failed { message, .. } => ImageAssetState::Failed(message.clone()),
        }
    }
}

struct ImageAssetStoreInner {
    assets: Mutex<HashMap<String, ImageAsset>>,
    active_owners: Mutex<HashSet<ImageOwner>>,
    source: Arc<dyn ImageBytes>,
    runtime: Arc<tokio::runtime::Runtime>,
    generation: AtomicU64,
}

#[derive(Clone)]
pub struct ImageAssetStore {
    inner: Arc<ImageAssetStoreInner>,
}

impl ImageAssetStore {
    pub fn new(cache: Arc<ContentCache>, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self::from_source(Arc::new(ContentCacheBytes(cache)), runtime)
    }

    pub fn without_cache(runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self::from_source(Arc::new(EmptyImageBytes), runtime)
    }

    /// The production source: plugin-document image references resolve
    /// through the facade, everything else through `cache` -- see
    /// [`FacadeImageBytes`].
    pub fn with_plugin_images(
        cache: Option<Arc<ContentCache>>,
        facade: arclain_app::ArclainApp,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self::from_source(
            Arc::new(FacadeImageBytes {
                facade,
                runtime: runtime.clone(),
                fallback: cache,
            }),
            runtime,
        )
    }

    fn from_source(source: Arc<dyn ImageBytes>, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self {
            inner: Arc::new(ImageAssetStoreInner {
                assets: Mutex::new(HashMap::new()),
                active_owners: Mutex::new(HashSet::new()),
                source,
                runtime,
                generation: AtomicU64::new(1),
            }),
        }
    }

    pub fn request(&self, owner: ImageOwner, key: &str, ctx: egui::Context) -> ImageAssetState {
        // The read choke point for cross-plugin key forgery. Every image
        // read in this frontend funnels through here with the owner of the
        // *surface doing the reading*, so checking the key's claimed owner
        // against it covers the flat plugin renderer, the document
        // renderer, the carousel and anything added later -- unlike a
        // per-renderer guard, which a new call site can simply not have.
        if !image_key_is_addressable_by(key, owner.plugin_id()) {
            // The key is guest-authored, so it is never logged; this
            // crate's plugin traces stay guest-value-free.
            tracing::warn!(
                plugin_id = owner.plugin_id().unwrap_or("<none>"),
                "refused an image key naming a different plugin's cache namespace"
            );
            return ImageAssetState::Failed(
                "image key is not addressable by this plugin".to_string(),
            );
        }
        self.mark_owner_active(owner.clone());
        {
            let mut assets = self.inner.assets.lock();
            if let Some(asset) = assets.get_mut(key) {
                asset.owners_mut().insert(owner);
                return asset.state();
            }
        }

        let generation = self.inner.generation.fetch_add(1, Ordering::Relaxed);
        {
            let mut assets = self.inner.assets.lock();
            if let Some(asset) = assets.get_mut(key) {
                asset.owners_mut().insert(owner);
                return asset.state();
            }
            assets.insert(
                key.to_string(),
                ImageAsset::Loading {
                    generation,
                    restart_pending: false,
                    owners: HashSet::from([owner]),
                },
            );
        }

        self.spawn_load(key.to_string(), generation, ctx);
        ImageAssetState::Loading
    }

    /// Stores bytes a URL-fallback fetch produced for `key`, then retries
    /// the key so the decode/upload pipeline picks them up.
    ///
    /// The single write entry point, deliberately paired with the read on
    /// [`ImageBytes`]: the caller supplies a key, bytes, and the id of the
    /// plugin whose document referenced them, and does not get to decide
    /// which cache namespace they land in -- that decision must match the
    /// one the read makes for the same key. See [`ImageBytes`]'s own doc
    /// comment for the failure this prevents.
    ///
    /// **`async`, and the actual write runs on the blocking pool.** Both
    /// are load-bearing rather than stylistic:
    ///
    /// - [`ImageBytes::put`] is synchronous and genuinely blocking (a
    ///   `cacache` write, and for a plugin key a facade round trip that
    ///   itself blocks on a future). Its only caller is
    ///   `crate::shared::image_fetcher::trigger_image_fetch`, which runs
    ///   inside a task *on this store's own runtime* -- and
    ///   `Runtime::block_on` from a thread that runtime is driving panics
    ///   with "Cannot start a runtime from within a runtime". Handing the
    ///   write to `spawn_blocking` puts it on a blocking-pool thread,
    ///   which is not an entered runtime context, exactly as the read half
    ///   ([`Self::spawn_load_inner`]) has always done.
    /// - Being `async` is what makes that safe by construction: a caller
    ///   must already be in an async context to await this, so the
    ///   blocking hop can never be skipped by a future call site the way
    ///   it was when this was a plain sync method.
    ///
    /// It also fixes a pre-existing smell the panic masked: the old code
    /// called `ContentCache::put` (a synchronous disk write) directly on a
    /// runtime worker thread.
    pub async fn store_fetched(
        &self,
        plugin_id: Option<String>,
        key: String,
        bytes: Vec<u8>,
        source_url: Option<String>,
        ctx: egui::Context,
    ) -> anyhow::Result<()> {
        let inner = self.inner.clone();
        let write_key = key.clone();
        inner
            .runtime
            .clone()
            .spawn_blocking(move || {
                inner.source.put(
                    plugin_id.as_deref(),
                    &write_key,
                    &bytes,
                    source_url.as_deref(),
                )
            })
            .await
            .map_err(|error| anyhow::anyhow!("image store worker failed: {error}"))??;
        self.cache_ready(&key, ctx);
        Ok(())
    }

    /// Whether a fetch for `key` has anywhere to land -- see
    /// [`ImageBytes::can_store`]. Lets `crate::shared::image_fetcher`
    /// avoid issuing a request whose result would be silently discarded
    /// and then re-requested every 30 s.
    pub fn can_store(&self, key: &str) -> bool {
        self.inner.source.can_store(key)
    }

    /// Retry a key after its asynchronous network fetch has populated the
    /// content cache. Existing owners are retained across the new load.
    pub fn cache_ready(&self, key: &str, ctx: egui::Context) {
        let generation_to_spawn = {
            let mut assets = self.inner.assets.lock();
            let Some(asset) = assets.get_mut(key) else {
                return;
            };
            match asset {
                ImageAsset::Loading {
                    restart_pending, ..
                } => {
                    *restart_pending = true;
                    None
                }
                ImageAsset::Failed { .. } => {
                    let generation = self.inner.generation.fetch_add(1, Ordering::Relaxed);
                    let owners = std::mem::take(asset.owners_mut());
                    *asset = ImageAsset::Loading {
                        generation,
                        restart_pending: false,
                        owners,
                    };
                    Some(generation)
                }
                ImageAsset::Decoded { .. } | ImageAsset::Uploaded { .. } => None,
            }
        };

        if let Some(generation) = generation_to_spawn {
            self.spawn_load(key.to_string(), generation, ctx);
        }
    }

    fn spawn_load(&self, key: String, generation: u64, ctx: egui::Context) {
        let inner = self.inner.clone();
        Self::spawn_load_inner(inner, key, generation, ctx);
    }

    fn spawn_load_inner(
        inner: Arc<ImageAssetStoreInner>,
        key: String,
        generation: u64,
        ctx: egui::Context,
    ) {
        let runtime = inner.runtime.clone();
        runtime.spawn_blocking(move || {
            let result = inner.source.get(&key).and_then(|bytes| {
                let Some(bytes) = bytes else {
                    anyhow::bail!("image is not present in the content cache");
                };
                let image = image::load_from_memory(&bytes).map_err(|error| {
                    inner.source.remove(&key);
                    anyhow::anyhow!("cached image decode failed: {error}")
                })?;
                let rgba = image.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                Ok((size, rgba.into_raw()))
            });

            let restart_generation = {
                let mut assets = inner.assets.lock();
                let Some(asset) = assets.get_mut(&key) else {
                    return;
                };
                if !matches!(
                    asset,
                    ImageAsset::Loading {
                        generation: current,
                        ..
                    } if *current == generation
                ) {
                    return;
                }
                let restart_pending = matches!(
                    asset,
                    ImageAsset::Loading {
                        restart_pending: true,
                        ..
                    }
                );
                let owners = std::mem::take(asset.owners_mut());
                if restart_pending {
                    let next_generation = inner.generation.fetch_add(1, Ordering::Relaxed);
                    *asset = ImageAsset::Loading {
                        generation: next_generation,
                        restart_pending: false,
                        owners,
                    };
                    Some(next_generation)
                } else {
                    *asset = match result {
                        Ok((size, rgba)) => ImageAsset::Decoded { size, rgba, owners },
                        Err(error) => ImageAsset::Failed {
                            message: error.to_string(),
                            owners,
                        },
                    };
                    None
                }
            };

            if let Some(next_generation) = restart_generation {
                Self::spawn_load_inner(inner, key, next_generation, ctx);
            } else {
                ctx.request_repaint();
            }
        });
    }

    pub fn upload_ready(&self, key: &str, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let mut assets = self.inner.assets.lock();
        let asset = assets.get_mut(key)?;
        if let ImageAsset::Uploaded { texture, .. } = asset {
            return Some(texture.clone());
        }
        if !matches!(asset, ImageAsset::Decoded { .. }) {
            return None;
        }

        let placeholder = ImageAsset::Failed {
            message: "texture upload interrupted".to_string(),
            owners: HashSet::new(),
        };
        let decoded = std::mem::replace(asset, placeholder);
        let ImageAsset::Decoded { size, rgba, owners } = decoded else {
            unreachable!("decoded state checked before replacement");
        };
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
        let texture = ctx.load_texture(key, color_image, egui::TextureOptions::default());
        *asset = ImageAsset::Uploaded {
            texture: texture.clone(),
            owners,
        };
        Some(texture)
    }

    pub fn get_texture(&self, owner: &ImageOwner, key: &str) -> Option<egui::TextureHandle> {
        let assets = self.inner.assets.lock();
        match assets.get(key) {
            Some(ImageAsset::Uploaded { texture, owners }) if owners.contains(owner) => {
                Some(texture.clone())
            }
            _ => None,
        }
    }

    pub fn release_owner(&self, owner: &ImageOwner) {
        let mut assets = self.inner.assets.lock();
        assets.retain(|_, asset| {
            asset.owners_mut().remove(owner);
            !asset.owners().is_empty()
        });
    }

    pub fn retain_owners(&self, active: &HashSet<ImageOwner>) {
        let mut assets = self.inner.assets.lock();
        assets.retain(|_, asset| {
            asset.owners_mut().retain(|owner| active.contains(owner));
            !asset.owners().is_empty()
        });
    }

    /// Mark a surface owner active for the current render pass, even when its
    /// current layout contains no image or its image is outside the viewport.
    pub fn mark_owner_active(&self, owner: ImageOwner) {
        self.inner.active_owners.lock().insert(owner);
    }

    /// Drain owners observed during this render pass. `update_app` feeds the
    /// result to `retain_owners` after rendering every image-owning surface.
    pub fn take_active_owners(&self) -> HashSet<ImageOwner> {
        std::mem::take(&mut *self.inner.active_owners.lock())
    }

    pub fn state(&self, key: &str) -> Option<ImageAssetState> {
        self.inner.assets.lock().get(key).map(ImageAsset::state)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.inner.assets.lock().contains_key(key)
    }

    pub fn is_decoded(&self, key: &str) -> bool {
        matches!(
            self.inner.assets.lock().get(key),
            Some(ImageAsset::Decoded { .. })
        )
    }

    pub fn cpu_byte_len(&self, key: &str) -> usize {
        match self.inner.assets.lock().get(key) {
            Some(ImageAsset::Decoded { rgba, .. }) => rgba.len(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod key_ownership_tests {
    use super::*;

    /// An unstamped key is host-namespace and unrestricted -- every
    /// pre-facade plugin image key looks like this, so restricting them
    /// would break the legacy renderers wholesale.
    #[test]
    fn an_unstamped_key_is_addressable_by_anyone() {
        assert!(image_key_is_addressable_by("dlsite:image:RJ1", Some("a")));
        assert!(image_key_is_addressable_by("dlsite:image:RJ1", None));
        assert!(!is_host_owned_image_key("dlsite:image:RJ1"));
    }

    /// The whole point: a stamped key may only be used by the plugin it
    /// names, so a plugin authoring `plugin-image:victim:k` addresses
    /// nothing.
    #[test]
    fn a_stamped_key_is_addressable_only_by_the_plugin_it_names() {
        let key = "plugin-image:victim:secret";
        assert_eq!(host_owned_image_key_owner(key), Some("victim"));
        assert!(image_key_is_addressable_by(key, Some("victim")));
        assert!(!image_key_is_addressable_by(key, Some("attacker")));
    }

    /// The host stamps its own id in front, so a plugin that tries to
    /// forge one ends up owning the whole forged string as its own key --
    /// the encoding is not ambiguous, and this pins that.
    #[test]
    fn a_forged_prefix_inside_a_stamped_key_still_resolves_to_the_stamping_plugin() {
        let doubly_stamped = "plugin-image:attacker:plugin-image:victim:secret";
        assert_eq!(host_owned_image_key_owner(doubly_stamped), Some("attacker"));
        assert!(!image_key_is_addressable_by(doubly_stamped, Some("victim")));
    }

    /// Only the lightbox has no owning plugin, and its keys come from
    /// facade-stamped intents; the legacy ingress filters them itself
    /// (see `plugin_controller`'s `OpenLightbox` arm).
    #[test]
    fn a_surface_with_no_owning_plugin_is_not_restricted() {
        assert!(image_key_is_addressable_by("plugin-image:anyone:k", None));
        assert_eq!(
            ImageOwner::Lightbox {
                tab: TabId(1),
                plugin_id: None
            }
            .plugin_id(),
            None
        );
        assert_eq!(
            ImageOwner::Lightbox {
                tab: TabId(1),
                plugin_id: Some("demo".to_string())
            }
            .plugin_id(),
            Some("demo")
        );
        assert_eq!(
            ImageOwner::plugin_settings("demo").plugin_id(),
            Some("demo")
        );
    }
}
