//! Server configuration

use std::path::PathBuf;

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Port to listen on
    pub port: u16,
    /// Database path
    pub database_path: PathBuf,
    /// Cache directory
    pub cache_dir: PathBuf,
    /// Maximum cache size in bytes
    pub max_cache_size: u64,
    /// Enable background fetching
    pub background_fetch: bool,
    /// Number of worker threads
    pub workers: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            database_path: PathBuf::from("./gameta.db"),
            cache_dir: PathBuf::from("./cache"),
            max_cache_size: 1024 * 1024 * 1024, // 1GB
            background_fetch: true,
            workers: 4,
        }
    }
}

impl ServerConfig {
    /// Create a new configuration builder
    pub fn builder() -> ServerConfigBuilder {
        ServerConfigBuilder::default()
    }
}

/// Builder for ServerConfig
#[derive(Default)]
pub struct ServerConfigBuilder {
    config: ServerConfig,
}

impl ServerConfigBuilder {
    pub fn port(mut self, port: u16) -> Self {
        self.config.port = port;
        self
    }

    pub fn database_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.database_path = path.into();
        self
    }

    pub fn cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.cache_dir = path.into();
        self
    }

    pub fn max_cache_size(mut self, size: u64) -> Self {
        self.config.max_cache_size = size;
        self
    }

    pub fn background_fetch(mut self, enabled: bool) -> Self {
        self.config.background_fetch = enabled;
        self
    }

    pub fn workers(mut self, count: usize) -> Self {
        self.config.workers = count;
        self
    }

    pub fn build(self) -> ServerConfig {
        self.config
    }
}
