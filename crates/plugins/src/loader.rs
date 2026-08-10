//! Arclain compatibility adapter over Wirt's secure plugin loader.

use crate::runtime::LoadedPlugin;
use crate::types::{PluginManifest, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) use wirt::TrustedPluginRoot;
pub use wirt::{DiscoveredPlugin, PluginArtifact};

/// Discovers and loads plugins through Wirt's product-neutral loader.
pub struct PluginLoader {
    inner: wirt::PluginLoader,
}

impl PluginLoader {
    /// Create a new plugin loader rooted at `plugins_dir`.
    pub fn new(plugins_dir: PathBuf) -> Result<Self> {
        wirt::PluginLoader::new(plugins_dir).map(|inner| Self { inner })
    }

    /// Discover all valid plugins in the configured root.
    pub fn discover_plugins(&self) -> Result<Vec<DiscoveredPlugin>> {
        self.inner.discover_plugins()
    }

    /// Validate a plugin manifest.
    pub fn validate_manifest(&self, manifest: &PluginManifest) -> Result<()> {
        self.inner.validate_manifest(manifest)
    }

    /// Load a discovered plugin component.
    pub fn load_plugin(&self, plugin: &DiscoveredPlugin) -> Result<LoadedPlugin> {
        self.inner.load_plugin(plugin).map(LoadedPlugin::from)
    }

    pub(crate) fn load_plugin_with_fingerprint(
        &self,
        plugin: &DiscoveredPlugin,
    ) -> Result<(LoadedPlugin, wirt::PackageFingerprint)> {
        self.inner
            .load_plugin_with_fingerprint(plugin)
            .map(|(loaded, fingerprint)| (LoadedPlugin::from(loaded), fingerprint))
    }

    /// Load a plugin directly from component bytes.
    pub fn load_wasm(&self, bytes: &[u8]) -> Result<LoadedPlugin> {
        self.inner.load_wasm(bytes).map(LoadedPlugin::from)
    }

    #[cfg(test)]
    pub(crate) fn read_wasm_file(&self, wasm_path: &Path) -> Result<Vec<u8>> {
        self.inner.read_wasm_file(wasm_path)
    }

    pub(crate) fn read_package_file(&self, package_path: &Path) -> Result<wirt::ValidatedPackage> {
        self.inner.read_package_file(package_path)
    }

    pub(crate) fn discover_plugin_from_folder(
        &self,
        manifest_path: &Path,
    ) -> Result<DiscoveredPlugin> {
        self.inner.discover_plugin_from_folder(manifest_path)
    }

    /// Get the configured plugin directory.
    pub fn plugins_dir(&self) -> &Path {
        self.inner.plugins_dir()
    }

    pub(crate) fn trusted_root(&self) -> Arc<TrustedPluginRoot> {
        self.inner.trusted_root()
    }
}

#[cfg(test)]
mod adapter_shape {
    use super::PluginLoader;

    fn assert_loader_is_exact_wirt_wrapper(adapter: PluginLoader) {
        let PluginLoader { inner } = adapter;
        let _: wirt::PluginLoader = inner;
    }

    const _: fn(PluginLoader) = assert_loader_is_exact_wirt_wrapper;
}
