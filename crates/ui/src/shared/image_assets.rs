//! Unified asynchronous cached-image loading, decoding, and texture ownership.
//!
//! Cached bytes and image decoding stay on the Tokio blocking pool. Renderers
//! only observe this store, upload a ready RGBA buffer on the UI thread, and
//! clone the resulting shared texture handle.

use crate::core::tabs::TabId;
use arclain_core::ContentCache;
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
    Lightbox(TabId),
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageAssetState {
    Loading,
    Decoded,
    Uploaded,
    Failed(String),
}

trait ImageBytes: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    fn remove(&self, key: &str);
}

struct ContentCacheBytes(Arc<ContentCache>);

impl ImageBytes for ContentCacheBytes {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.0.get(key)
    }

    fn remove(&self, key: &str) {
        if let Err(error) = self.0.remove(key) {
            tracing::warn!(%error, %key, "failed to remove corrupt cached image");
        }
    }
}

struct EmptyImageBytes;

impl ImageBytes for EmptyImageBytes {
    fn get(&self, _key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn remove(&self, _key: &str) {}
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
