//! Unified asynchronous cached-image loading, decoding, and texture ownership.
//!
//! Cached bytes and image decoding stay on the Tokio blocking pool. Renderers
//! only observe this store, upload a ready RGBA buffer on the UI thread, and
//! clone the resulting shared texture handle.

use crate::core::tabs::TabId;
use eframe::egui;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

/// Whether `key` addresses the **plugin-scoped** image namespace rather
/// than the host-owned one -- i.e. whether the application stamped it with
/// an owning plugin when it normalized that plugin's document.
///
/// This is the frontend's namespace-routing decision, so its name states
/// which namespace it selects. (It was `is_host_owned_image_key`, meaning
/// "host-*stamped*"; once it started choosing between
/// `read_plugin_image` and `read_host_image`, that name read as the exact
/// opposite of the branch it guards, which is a dangerous thing for a
/// reader to "correct".)
///
/// A plugin's cache namespace is an isolation boundary this codebase
/// enforces deliberately, and such a key encodes its own owner -- so an
/// unvalidated key is a bearer token for whatever namespace it names.
/// Only the host ever produces one (during normalization, from a plugin id
/// it already knows); a value carrying that shape which did *not* come
/// from there is a forgery attempt.
///
/// Delegates to the application's own predicate rather than re-deriving
/// the encoding: this frontend and the application must agree on which
/// namespace a key belongs to, and two copies of one prefix literal is
/// precisely how they would stop agreeing.
pub fn is_plugin_scoped_image_key(key: &str) -> bool {
    arclain_app::plugins::is_plugin_image_key(key)
}

/// The plugin a plugin-scoped key names as its owner, or `None` for a
/// host-owned key.
pub fn plugin_scoped_image_key_owner(key: &str) -> Option<&str> {
    arclain_app::plugins::plugin_image_key_owner(key)
}

/// Whether a surface belonging to `acting_plugin_id` may address `key`.
///
/// The single rule behind both choke points below
/// ([`ImageAssetStore::request`] for reads,
/// `crate::shared::image_fetcher::trigger_image_fetch` for fetches): a
/// plugin-scoped key may only be used by the plugin it names. Host-owned
/// keys are unrestricted, exactly as before.
///
/// `acting_plugin_id` is `None` only for a surface with no owning plugin
/// at all -- a host-opened lightbox, which carries no plugin-authored
/// keys, so there is nothing to compare against.
pub fn image_key_is_addressable_by(key: &str, acting_plugin_id: Option<&str>) -> bool {
    match (plugin_scoped_image_key_owner(key), acting_plugin_id) {
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
/// critically -- fills them from a URL.
///
/// [`Self::fetch`] is on the *same* trait as [`Self::get`] deliberately. A
/// URL fallback and the read that asked for it must agree on which cache
/// namespace the key belongs to; when they were separate code paths (a
/// content-cache write at the fetch site, a namespace-decoding read here)
/// they silently disagreed for plugin-owned keys, producing an entry the
/// read could never find and a recovery loop that could never succeed.
/// Routing both through one implementation makes that class of mismatch
/// unrepresentable.
///
/// Public so this crate's integration tests can drive the asset lifecycle
/// against an instrumented source. Production has exactly one
/// implementation -- [`FacadeImageBytes`], installed by
/// [`ImageAssetStore::new`].
pub trait ImageBytes: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    /// Fetch `url` and cache it under `key`, in the same namespace
    /// [`Self::get`] reads `key` from.
    ///
    /// `plugin_id` is the plugin whose document referenced `key`, as the
    /// *host* knows it -- never anything the plugin supplied. It decides
    /// whose network budget pays for the request, and for a plugin-scoped
    /// key it must match the owner the key encodes.
    ///
    /// The fetched bytes are not returned: this store publishes an asset
    /// by re-reading it through [`Self::get`] (see
    /// [`ImageAssetStore::cache_ready`]), which is what keeps a fetch
    /// completing mid-load from racing the load already in flight.
    fn fetch(&self, plugin_id: Option<&str>, key: &str, url: &str) -> anyhow::Result<()>;
    fn remove(&self, key: &str);
    /// Whether a successful [`Self::fetch`] for `key` would actually
    /// retain anything. `false` lets callers skip work whose result has
    /// nowhere to land -- see
    /// `crate::shared::image_fetcher::trigger_image_fetch`.
    fn can_store(&self, key: &str) -> bool;
}

/// Resolves every image reference through the application facade: plugin
/// document keys through the plugin-scoped namespace, everything else
/// through the host-owned one.
///
/// The facade decides which namespace a key belongs to and enforces the
/// per-asset size cap on both; this frontend holds no cache handle and no
/// HTTP client of its own. The branch below is the same predicate the
/// facade itself uses ([`is_plugin_scoped_image_key`]), so read, fetch and
/// eviction always land in one namespace per key.
struct FacadeImageBytes {
    facade: arclain_app::ArclainApp,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl FacadeImageBytes {
    /// Awaits a facade future from this store's own runtime.
    ///
    /// Every caller runs on [`ImageAssetStore`]'s blocking pool (see
    /// [`ImageAssetStore::spawn_load_inner`] and
    /// [`ImageAssetStore::fetch_into_cache`]), never on a worker thread,
    /// so blocking here cannot stall the runtime. The facade's futures are
    /// executor-agnostic by contract, so awaiting one from this crate's
    /// runtime rather than the application's own is supported.
    fn block_on<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        self.runtime.block_on(future)
    }
}

impl ImageBytes for FacadeImageBytes {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let read = if is_plugin_scoped_image_key(key) {
            self.block_on(self.facade.read_plugin_image(key.to_string()))
        } else {
            self.block_on(self.facade.read_host_image(key.to_string()))
        };
        match read {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind == arclain_app::error::ApplicationErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(anyhow::anyhow!(error.summary)),
        }
    }

    /// Fills a missing asset from its URL fallback, into the one namespace
    /// [`Self::get`] reads that key from. A host-namespace write for a
    /// plugin key would be invisible to that read, so the asset would
    /// render broken and re-fetch every 30 s forever, leaving one orphaned
    /// cache entry per attempt.
    ///
    /// `plugin_id` is the host's own knowledge of which plugin's document
    /// referenced this key. The facade refuses a plugin-scoped key
    /// claiming a *different* owner: without that, a key is a bearer token
    /// for a cache namespace, and any caller holding a
    /// `plugin-image:victim:k` string could write into `victim`'s
    /// namespace and have `victim` later render those bytes as its own.
    /// The frontend-side half of the guard is the two choke points every
    /// image path funnels through -- [`ImageAssetStore::request`] for reads
    /// and `crate::shared::image_fetcher::trigger_image_fetch` for fetches
    /// -- so neither half alone has to be perfect.
    fn fetch(&self, plugin_id: Option<&str>, key: &str, url: &str) -> anyhow::Result<()> {
        let fetched = if is_plugin_scoped_image_key(key) {
            let Some(plugin_id) = plugin_id else {
                anyhow::bail!(
                    "refusing to fetch a plugin-namespaced image with no owning plugin known"
                );
            };
            self.block_on(self.facade.fetch_plugin_image(
                plugin_id.to_string(),
                key.to_string(),
                url.to_string(),
            ))
        } else {
            self.block_on(self.facade.fetch_host_image(
                key.to_string(),
                url.to_string(),
                plugin_id.map(str::to_string),
            ))
        };
        fetched
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!(error.summary))
    }

    fn remove(&self, key: &str) {
        // A plugin-owned entry is not this frontend's to evict: that
        // namespace belongs to the plugin, and a corrupt entry there is
        // re-fetched by the plugin, not dropped by us. The facade refuses
        // such a key here anyway; returning early keeps the refusal out of
        // the logs for a case that is expected rather than exceptional.
        if is_plugin_scoped_image_key(key) {
            return;
        }
        if let Err(error) = self.block_on(self.facade.discard_host_image(key.to_string())) {
            tracing::warn!(
                summary = %error.summary,
                "failed to remove a corrupt cached image"
            );
        }
    }

    /// The application owns both image namespaces, so every key has
    /// somewhere to go.
    fn can_store(&self, _key: &str) -> bool {
        true
    }
}

struct EmptyImageBytes;

impl ImageBytes for EmptyImageBytes {
    fn get(&self, _key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    /// Silently discarded rather than an error: this source exists for
    /// contexts with no application behind them (early init, hand-built
    /// test fixtures), where a fetch having nowhere to land is expected,
    /// not a failure the caller should surface. Callers avoid reaching
    /// here at all -- see
    /// `crate::shared::image_fetcher::trigger_image_fetch`'s storability
    /// guard.
    fn fetch(&self, _plugin_id: Option<&str>, _key: &str, _url: &str) -> anyhow::Result<()> {
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
    /// The production store: every image reference resolves through
    /// `facade` -- see [`FacadeImageBytes`].
    pub fn new(facade: arclain_app::ArclainApp, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self::from_source(
            Arc::new(FacadeImageBytes {
                facade,
                runtime: runtime.clone(),
            }),
            runtime,
        )
    }

    /// A store with no byte source at all: every read misses and every
    /// fetch is refused as unstorable. For contexts with no application
    /// behind them -- early initialization and hand-built test fixtures.
    pub fn without_source(runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self::from_source(Arc::new(EmptyImageBytes), runtime)
    }

    /// Builds a store over an arbitrary byte source.
    ///
    /// Exists for this crate's own lifecycle tests, which need a source
    /// they can count and stall; production always goes through
    /// [`Self::new`].
    pub fn from_source(source: Arc<dyn ImageBytes>, runtime: Arc<tokio::runtime::Runtime>) -> Self {
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

    /// Fills `key` from its URL fallback, then retries the key so the
    /// decode/upload pipeline picks the bytes up.
    ///
    /// The single fill entry point, deliberately paired with the read on
    /// [`ImageBytes`]: the caller supplies a key, a URL, and the id of the
    /// plugin whose document referenced them, and does not get to decide
    /// which cache namespace the bytes land in -- that decision must match
    /// the one the read makes for the same key. See [`ImageBytes`]'s own
    /// doc comment for the failure this prevents.
    ///
    /// **`async`, and the fetch itself runs on the blocking pool.** Both
    /// are load-bearing rather than stylistic:
    ///
    /// - [`ImageBytes::fetch`] is synchronous and genuinely blocking (a
    ///   facade round trip that awaits a future internally). Its only
    ///   caller is `crate::shared::image_fetcher::trigger_image_fetch`,
    ///   which runs inside a task *on this store's own runtime* -- and
    ///   `Runtime::block_on` from a thread that runtime is driving panics
    ///   with "Cannot start a runtime from within a runtime". Handing the
    ///   fetch to `spawn_blocking` puts it on a blocking-pool thread,
    ///   which is not an entered runtime context, exactly as the read half
    ///   ([`Self::spawn_load_inner`]) has always done.
    /// - Being `async` is what makes that safe by construction: a caller
    ///   must already be in an async context to await this, so the
    ///   blocking hop can never be skipped by a future call site the way
    ///   it was when this was a plain sync method.
    pub async fn fetch_into_cache(
        &self,
        plugin_id: Option<String>,
        key: String,
        url: String,
        ctx: egui::Context,
    ) -> anyhow::Result<()> {
        let inner = self.inner.clone();
        let fetch_key = key.clone();
        inner
            .runtime
            .clone()
            .spawn_blocking(move || inner.source.fetch(plugin_id.as_deref(), &fetch_key, &url))
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
        assert!(!is_plugin_scoped_image_key("dlsite:image:RJ1"));
    }

    /// The whole point: a stamped key may only be used by the plugin it
    /// names, so a plugin authoring `plugin-image:victim:k` addresses
    /// nothing.
    #[test]
    fn a_stamped_key_is_addressable_only_by_the_plugin_it_names() {
        let key = "plugin-image:victim:secret";
        assert_eq!(plugin_scoped_image_key_owner(key), Some("victim"));
        assert!(image_key_is_addressable_by(key, Some("victim")));
        assert!(!image_key_is_addressable_by(key, Some("attacker")));
    }

    /// The host stamps its own id in front, so a plugin that tries to
    /// forge one ends up owning the whole forged string as its own key --
    /// the encoding is not ambiguous, and this pins that.
    #[test]
    fn a_forged_prefix_inside_a_stamped_key_still_resolves_to_the_stamping_plugin() {
        let doubly_stamped = "plugin-image:attacker:plugin-image:victim:secret";
        assert_eq!(
            plugin_scoped_image_key_owner(doubly_stamped),
            Some("attacker")
        );
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
