//! Plugin discovery and loading

use crate::runtime::{LoadedComponent, WasmRuntime};
use crate::{PluginError, PluginId, PluginInfo, PluginManifest, Result, WIRT_ABI_VERSION};
use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

pub(crate) const MAX_PLUGIN_MANIFEST_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PLUGIN_WASM_BYTES: usize = 64 * 1024 * 1024;
const MAX_PLUGIN_NAME_BYTES: usize = 128;
const MAX_PLUGIN_VERSION_BYTES: usize = 64;
const MAX_PLUGIN_AUTHOR_BYTES: usize = 256;
const MAX_PLUGIN_DESCRIPTION_BYTES: usize = 16 * 1024;
const MAX_PLUGIN_NETWORK_DOMAINS: usize = 64;
const MAX_PLUGIN_DOMAIN_BYTES: usize = 253;
const MAX_PLUGIN_HTTP_REQUESTS_PER_MINUTE: u32 = 600;

fn load_error(message: impl Into<String>) -> PluginError {
    PluginError::LoadError(message.into())
}

fn read_opened_file_bounded(mut file: File, max_bytes: usize, kind: &str) -> Result<Vec<u8>> {
    let metadata = file
        .metadata()
        .map_err(|error| load_error(format!("Failed to inspect {kind} file: {error}")))?;
    if !metadata.is_file() {
        return Err(load_error(format!("{kind} path is not a regular file")));
    }
    if metadata.len() > max_bytes as u64 {
        return Err(load_error(format!(
            "{kind} file exceeds the {max_bytes}-byte limit"
        )));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| load_error(format!("Failed to read {kind} file: {error}")))?;
    if bytes.len() > max_bytes {
        return Err(load_error(format!(
            "{kind} file grew beyond the {max_bytes}-byte limit while reading"
        )));
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &cap_std::fs::Metadata) -> Self {
        Self {
            device: MetadataExt::dev(metadata),
            inode: MetadataExt::ino(metadata),
        }
    }
}

fn open_directory_no_follow(path: &Path, kind: &str) -> Result<Dir> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let file_name = path.file_name().ok_or_else(|| {
        load_error(format!(
            "{kind} has no final directory component: {}",
            path.display()
        ))
    })?;
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|error| {
        load_error(format!(
            "Failed to resolve {kind} parent {}: {error}",
            parent.display()
        ))
    })?;
    let parent = Dir::open_ambient_dir(&parent, cap_std::ambient_authority()).map_err(|error| {
        load_error(format!(
            "Failed to open {kind} parent {}: {error}",
            parent.display()
        ))
    })?;
    parent.open_dir_nofollow(file_name).map_err(|error| {
        load_error(format!(
            "Failed to open {kind} without following links {}: {error}",
            path.display()
        ))
    })
}

/// Capability handle for the exact plugin root captured when the loader starts.
pub struct TrustedPluginRoot {
    configured_path: PathBuf,
    canonical_path: PathBuf,
    dir: Dir,
    identity: FileIdentity,
}

impl TrustedPluginRoot {
    fn open(configured_path: PathBuf) -> Result<Self> {
        let dir = open_directory_no_follow(&configured_path, "plugin root")?;
        let metadata = dir.dir_metadata().map_err(|error| {
            load_error(format!("Failed to inspect opened plugin root: {error}"))
        })?;
        if !metadata.is_dir() {
            return Err(load_error(format!(
                "Plugin root is not a directory: {}",
                configured_path.display()
            )));
        }
        let canonical_path = configured_path.canonicalize().map_err(|error| {
            load_error(format!(
                "Failed to resolve plugin root {}: {error}",
                configured_path.display()
            ))
        })?;
        Ok(Self {
            configured_path,
            canonical_path,
            identity: FileIdentity::from_metadata(&metadata),
            dir,
        })
    }

    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    pub fn configured_path(&self) -> &Path {
        &self.configured_path
    }

    pub fn revalidate_current_path(&self) -> Result<()> {
        let current = open_directory_no_follow(&self.configured_path, "plugin root")?;
        let current_metadata = current.dir_metadata().map_err(|error| {
            load_error(format!("Failed to inspect current plugin root: {error}"))
        })?;
        if FileIdentity::from_metadata(&current_metadata) != self.identity {
            return Err(load_error(format!(
                "Configured plugin root was replaced after it was opened: {}",
                self.configured_path.display()
            )));
        }
        Ok(())
    }

    fn relative_path(&self, path: &Path, kind: &str) -> Result<PathBuf> {
        let relative = path
            .strip_prefix(&self.configured_path)
            .or_else(|_| path.strip_prefix(&self.canonical_path))
            .map_err(|_| {
                load_error(format!(
                    "{kind} is not lexically beneath the configured plugin root: {}",
                    path.display()
                ))
            })?;
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err(load_error(format!(
                    "{kind} contains a non-portable path component: {}",
                    path.display()
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
        if components.is_empty() {
            return Err(load_error(format!(
                "{kind} resolves to the plugin root itself"
            )));
        }
        Ok(components.into_iter().collect())
    }

    fn open_parent_no_follow(&self, relative: &Path, kind: &str) -> Result<(Dir, OsString)> {
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err(load_error(format!(
                    "{kind} contains a non-portable relative path: {}",
                    relative.display()
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
        let (file_name, parents) = components
            .split_last()
            .ok_or_else(|| load_error(format!("{kind} has no file component")))?;
        let mut parent = self.dir.try_clone().map_err(|error| {
            load_error(format!(
                "Failed to clone trusted plugin root handle: {error}"
            ))
        })?;
        for component in parents {
            parent = parent.open_dir_nofollow(component).map_err(|error| {
                load_error(format!(
                    "Failed to open {kind} parent without following links: {error}"
                ))
            })?;
        }
        Ok((parent, file_name.clone()))
    }

    fn open_file_no_follow_with_hook(
        &self,
        path: &Path,
        kind: &str,
        before_open: impl FnOnce(),
    ) -> Result<File> {
        let relative = self.relative_path(path, kind)?;
        let (parent, file_name) = self.open_parent_no_follow(&relative, kind)?;
        before_open();
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        parent.open_with(file_name, &options).map_err(|error| {
            load_error(format!(
                "Failed to open {kind} file without following links: {error}"
            ))
        })
    }

    fn read_file_bounded_with_hook(
        &self,
        path: &Path,
        max_bytes: usize,
        kind: &str,
        before_open: impl FnOnce(),
    ) -> Result<Vec<u8>> {
        let file = self.open_file_no_follow_with_hook(path, kind, before_open)?;
        read_opened_file_bounded(file, max_bytes, kind)
    }

    fn read_file_bounded(&self, path: &Path, max_bytes: usize, kind: &str) -> Result<Vec<u8>> {
        self.read_file_bounded_with_hook(path, max_bytes, kind, || {})
    }

    fn validate_regular_file(&self, path: &Path, max_bytes: usize, kind: &str) -> Result<()> {
        let file = self.open_file_no_follow_with_hook(path, kind, || {})?;
        let metadata = file
            .metadata()
            .map_err(|error| load_error(format!("Failed to inspect {kind} file: {error}")))?;
        if !metadata.is_file() {
            return Err(load_error(format!("{kind} path is not a regular file")));
        }
        if metadata.len() > max_bytes as u64 {
            return Err(load_error(format!(
                "{kind} file exceeds the {max_bytes}-byte limit"
            )));
        }
        Ok(())
    }
}

fn read_ambient_file_bounded(path: &Path, max_bytes: usize, kind: &str) -> Result<Vec<u8>> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let file_name = path.file_name().ok_or_else(|| {
        load_error(format!(
            "{kind} path has no file component: {}",
            path.display()
        ))
    })?;
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .map_err(|error| load_error(format!("Failed to open {kind} parent directory: {error}")))?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = parent.open_with(file_name, &options).map_err(|error| {
        load_error(format!(
            "Failed to open {kind} file without following links: {error}"
        ))
    })?;
    read_opened_file_bounded(file, max_bytes, kind)
}

fn validate_bounded_text(field: &str, value: &str, max_bytes: usize, required: bool) -> Result<()> {
    if required && value.trim().is_empty() {
        return Err(PluginError::InvalidManifest(format!(
            "Plugin {field} is empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(PluginError::InvalidManifest(format!(
            "Plugin {field} exceeds the {max_bytes}-byte limit"
        )));
    }
    if value.contains('\0') {
        return Err(PluginError::InvalidManifest(format!(
            "Plugin {field} contains a NUL character"
        )));
    }
    Ok(())
}

fn is_canonical_hostname(domain: &str) -> bool {
    if domain.is_empty()
        || domain.len() > MAX_PLUGIN_DOMAIN_BYTES
        || !domain.is_ascii()
        || domain != domain.trim()
        || domain.bytes().any(|byte| byte.is_ascii_uppercase())
        || domain.ends_with('.')
    {
        return false;
    }

    domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    })
}

/// Discovers and loads plugins from a directory
pub struct PluginLoader {
    plugins_dir: PathBuf,
    trusted_root: Arc<TrustedPluginRoot>,
    runtime: WasmRuntime,
}

impl PluginLoader {
    /// Create a new plugin loader
    pub fn new(plugins_dir: PathBuf) -> Result<Self> {
        let runtime = WasmRuntime::new()?;

        // Create plugins directory if it doesn't exist
        if !plugins_dir.exists() {
            std::fs::create_dir_all(&plugins_dir)?;
            info!("Created plugins directory: {}", plugins_dir.display());
        }

        let trusted_root = Arc::new(TrustedPluginRoot::open(plugins_dir.clone())?);

        Ok(Self {
            plugins_dir,
            trusted_root,
            runtime,
        })
    }

    /// Discover all plugins in the plugins directory
    /// Supports two discovery modes:
    /// 1. Folder mode (preferred): plugins/plugin-name/plugin-name.toml + plugin-name.wasm
    /// 2. Flat mode (legacy): plugins/plugin-name.toml + plugin-name.wasm
    pub fn discover_plugins(&self) -> Result<Vec<DiscoveredPlugin>> {
        info!("Discovering plugins in: {}", self.plugins_dir.display());

        let mut discovered = Vec::new();
        let mut discovered_ids = HashSet::new();
        let mut entries = self
            .trusted_root
            .dir()
            .entries()
            .map_err(|error| load_error(format!("Failed to scan plugin root: {error}")))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(cap_std::fs::DirEntry::file_name);

        for entry in entries {
            let file_name = entry.file_name();
            let path = self.plugins_dir.join(&file_name);
            let entry_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    warn!("Rejected plugin entry {}: {}", path.display(), error);
                    continue;
                }
            };

            if entry_type.is_symlink() {
                warn!("Rejected linked plugin entry {}", path.display());
                continue;
            }

            if entry_type.is_dir() {
                // Folder mode: Look for manifest inside directory
                if let Some(plugin_name) = file_name.to_str() {
                    let manifest_path = path.join(format!("{}.toml", plugin_name));

                    let relative_manifest =
                        PathBuf::from(&file_name).join(format!("{}.toml", plugin_name));
                    if self
                        .trusted_root
                        .dir()
                        .symlink_metadata(&relative_manifest)
                        .is_ok()
                    {
                        match self.discover_plugin_from_folder(&manifest_path) {
                            Ok(plugin) => {
                                let plugin_id = PluginId::parse(plugin.manifest.plugin.id.clone())?;
                                let identity_key = plugin_id.identity_key();
                                if !discovered_ids.insert(identity_key.clone()) {
                                    return Err(PluginError::InvalidManifest(format!(
                                        "Duplicate discovered plugin ID: {identity_key}"
                                    )));
                                }
                                debug!(
                                    "Discovered plugin (folder): {} v{}",
                                    plugin.manifest.plugin.name, plugin.manifest.plugin.version
                                );
                                discovered.push(plugin);
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to discover plugin in folder {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            } else if entry_type.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("toml")
            {
                // Flat mode: Look for .toml manifest files in plugins root
                match self.discover_plugin_flat(&path) {
                    Ok(plugin) => {
                        let plugin_id = PluginId::parse(plugin.manifest.plugin.id.clone())?;
                        let identity_key = plugin_id.identity_key();
                        if !discovered_ids.insert(identity_key.clone()) {
                            return Err(PluginError::InvalidManifest(format!(
                                "Duplicate discovered plugin ID: {identity_key}"
                            )));
                        }
                        debug!(
                            "Discovered plugin (flat): {} v{}",
                            plugin.manifest.plugin.name, plugin.manifest.plugin.version
                        );
                        discovered.push(plugin);
                    }
                    Err(e) => {
                        warn!("Failed to discover plugin at {}: {}", path.display(), e);
                    }
                }
            }
        }

        info!("Discovered {} plugins", discovered.len());
        Ok(discovered)
    }

    /// Discover plugin from folder structure (preferred)
    /// Expected structure: plugins/plugin-name/plugin-name.toml + plugin-name.wasm
    pub fn discover_plugin_from_folder(&self, manifest_path: &Path) -> Result<DiscoveredPlugin> {
        self.validate_path_within_plugin_root(manifest_path, true, "plugin manifest")?;
        // Read and parse manifest
        let manifest_bytes = self.trusted_root.read_file_bounded(
            manifest_path,
            MAX_PLUGIN_MANIFEST_BYTES,
            "plugin manifest",
        )?;
        let manifest_content = std::str::from_utf8(&manifest_bytes).map_err(|error| {
            PluginError::InvalidManifest(format!("Plugin manifest is not UTF-8: {error}"))
        })?;
        let manifest: PluginManifest = toml::from_str(&manifest_content)?;

        // Validate manifest
        self.validate_manifest(&manifest)?;
        self.validate_manifest_id_matches_filename(&manifest, manifest_path)?;

        // Find corresponding .wasm file in the same directory
        let wasm_path = manifest_path.with_extension("wasm");
        self.validate_path_within_plugin_root(&wasm_path, true, "plugin WASM")?;
        self.trusted_root.validate_regular_file(
            &wasm_path,
            MAX_PLUGIN_WASM_BYTES,
            "plugin WASM",
        )?;

        debug!("Found plugin WASM: {}", wasm_path.display());

        Ok(DiscoveredPlugin {
            manifest,
            manifest_path: manifest_path.to_path_buf(),
            wasm_path,
        })
    }

    /// Discover plugin from flat structure (legacy compatibility)
    /// Expected structure: plugins/plugin-name.toml + plugins/plugin-name.wasm
    fn discover_plugin_flat(&self, manifest_path: &Path) -> Result<DiscoveredPlugin> {
        self.validate_path_within_plugin_root(manifest_path, true, "plugin manifest")?;
        // Read and parse manifest
        let manifest_bytes = self.trusted_root.read_file_bounded(
            manifest_path,
            MAX_PLUGIN_MANIFEST_BYTES,
            "plugin manifest",
        )?;
        let manifest_content = std::str::from_utf8(&manifest_bytes).map_err(|error| {
            PluginError::InvalidManifest(format!("Plugin manifest is not UTF-8: {error}"))
        })?;
        let manifest: PluginManifest = toml::from_str(&manifest_content)?;

        // Validate manifest
        self.validate_manifest(&manifest)?;
        self.validate_manifest_id_matches_filename(&manifest, manifest_path)?;

        // Find corresponding .wasm file
        let wasm_path = manifest_path.with_extension("wasm");
        self.validate_path_within_plugin_root(&wasm_path, true, "plugin WASM")?;
        self.trusted_root.validate_regular_file(
            &wasm_path,
            MAX_PLUGIN_WASM_BYTES,
            "plugin WASM",
        )?;

        debug!("Found plugin WASM: {}", wasm_path.display());

        Ok(DiscoveredPlugin {
            manifest,
            manifest_path: manifest_path.to_path_buf(),
            wasm_path,
        })
    }

    fn validate_path_within_plugin_root(
        &self,
        path: &Path,
        require_file: bool,
        kind: &str,
    ) -> Result<()> {
        let file = self
            .trusted_root
            .open_file_no_follow_with_hook(path, kind, || {})?;
        if require_file {
            let metadata = file.metadata().map_err(|error| {
                load_error(format!("Failed to inspect opened {kind} file: {error}"))
            })?;
            if !metadata.is_file() {
                return Err(load_error(format!(
                    "{kind} is not a regular file: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// Validate a plugin manifest
    pub fn validate_manifest(&self, manifest: &PluginManifest) -> Result<()> {
        if manifest.wirt.abi != WIRT_ABI_VERSION {
            return Err(PluginError::InvalidManifest(format!(
                "unsupported Wirt ABI {:?}; expected {WIRT_ABI_VERSION}",
                manifest.wirt.abi
            )));
        }

        PluginId::parse(manifest.plugin.id.clone())?;

        validate_bounded_text("name", &manifest.plugin.name, MAX_PLUGIN_NAME_BYTES, true)?;
        validate_bounded_text(
            "version",
            &manifest.plugin.version,
            MAX_PLUGIN_VERSION_BYTES,
            true,
        )?;
        validate_bounded_text(
            "author",
            &manifest.plugin.author,
            MAX_PLUGIN_AUTHOR_BYTES,
            false,
        )?;
        validate_bounded_text(
            "description",
            &manifest.plugin.description,
            MAX_PLUGIN_DESCRIPTION_BYTES,
            false,
        )?;

        let domains = &manifest.capabilities.network_domains;
        if domains.len() > MAX_PLUGIN_NETWORK_DOMAINS {
            return Err(PluginError::InvalidManifest(format!(
                "Plugin declares more than {MAX_PLUGIN_NETWORK_DOMAINS} network domains"
            )));
        }
        let mut unique_domains = HashSet::with_capacity(domains.len());
        for domain in domains {
            if !is_canonical_hostname(domain) {
                return Err(PluginError::InvalidManifest(format!(
                    "Network domain is not a canonical ASCII hostname: {domain:?}"
                )));
            }
            if !unique_domains.insert(domain.as_str()) {
                return Err(PluginError::InvalidManifest(format!(
                    "Duplicate network domain: {domain}"
                )));
            }
        }

        if manifest.rate_limits.http_requests_per_minute > MAX_PLUGIN_HTTP_REQUESTS_PER_MINUTE {
            return Err(PluginError::InvalidManifest(format!(
                "HTTP request rate exceeds {MAX_PLUGIN_HTTP_REQUESTS_PER_MINUTE} per minute"
            )));
        }

        Ok(())
    }

    fn validate_manifest_id_matches_filename(
        &self,
        manifest: &PluginManifest,
        manifest_path: &Path,
    ) -> Result<()> {
        let file_id = manifest_path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                PluginError::InvalidManifest(
                    "Plugin manifest filename is not valid UTF-8".to_string(),
                )
            })?;
        let manifest_id = PluginId::parse(manifest.plugin.id.clone())?;
        let file_id = PluginId::parse(file_id.to_string()).map_err(|_| {
            PluginError::InvalidManifest(format!(
                "Plugin manifest filename is not a portable plugin ID: {file_id:?}"
            ))
        })?;
        if manifest_id.identity_key() != file_id.identity_key() {
            return Err(PluginError::InvalidManifest(format!(
                "Plugin manifest ID {:?} does not match filename {:?}",
                manifest.plugin.id,
                file_id.as_str()
            )));
        }
        Ok(())
    }

    /// Load a plugin from a discovered plugin
    pub fn load_plugin(&self, discovered: &DiscoveredPlugin) -> Result<LoadedComponent> {
        info!("Loading plugin: {}", discovered.manifest.plugin.name);

        self.validate_path_within_plugin_root(&discovered.wasm_path, true, "plugin WASM")?;
        let wasm_bytes = self.trusted_root.read_file_bounded(
            &discovered.wasm_path,
            MAX_PLUGIN_WASM_BYTES,
            "plugin WASM",
        )?;
        let loaded = self
            .runtime
            .load_component_from_bytes(discovered.manifest.plugin.id.clone(), &wasm_bytes)?;

        info!(
            "Plugin loaded successfully: {}",
            discovered.manifest.plugin.name
        );
        Ok(loaded)
    }

    /// Load a plugin directly from WASM bytes
    /// This is used for plugin installation to validate the plugin before copying files
    pub fn load_wasm(&self, wasm_bytes: &[u8]) -> Result<LoadedComponent> {
        if wasm_bytes.len() > MAX_PLUGIN_WASM_BYTES {
            return Err(load_error(format!(
                "plugin WASM exceeds the {MAX_PLUGIN_WASM_BYTES}-byte limit"
            )));
        }
        // For temporary validation load, we use a placeholder ID
        // Real loading uses discover_plugin/load_plugin flow
        self.runtime
            .load_component_from_bytes("temp-validation".to_string(), wasm_bytes)
    }

    pub fn read_wasm_file(&self, wasm_path: &Path) -> Result<Vec<u8>> {
        read_ambient_file_bounded(wasm_path, MAX_PLUGIN_WASM_BYTES, "plugin WASM")
    }

    #[cfg(test)]
    fn read_plugin_file_bounded_with_hook(
        &self,
        path: &Path,
        max_bytes: usize,
        kind: &str,
        before_open: impl FnOnce(),
    ) -> Result<Vec<u8>> {
        self.trusted_root
            .read_file_bounded_with_hook(path, max_bytes, kind, before_open)
    }

    /// Get the plugins directory path
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    pub fn trusted_root(&self) -> Arc<TrustedPluginRoot> {
        self.trusted_root.clone()
    }
}

/// A discovered plugin with its manifest and paths
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    pub manifest_path: PathBuf,
    pub wasm_path: PathBuf,
}

impl DiscoveredPlugin {
    /// Get plugin info for display
    pub fn to_plugin_info(&self) -> PluginInfo {
        PluginInfo {
            metadata: crate::PluginMetadata {
                id: self.manifest.plugin.id.clone(),
                name: self.manifest.plugin.name.clone(),
                version: self.manifest.plugin.version.clone(),
                author: self.manifest.plugin.author.clone(),
                description: self.manifest.plugin.description.clone(),
            },
            capabilities: self.manifest.capabilities.to_capabilities(),
            manifest_path: self.manifest_path.clone(),
            wasm_path: self.wasm_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
