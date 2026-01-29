//! gameta_server - Optional metadata server daemon
//!
//! This crate provides an HTTP server for game metadata with:
//! - REST API for metadata queries
//! - Background fetching workers
//! - Cache management
//! - WebSocket support for real-time updates
//!
//! # Usage
//!
//! Run as standalone daemon:
//! ```ignore
//! gameta_server --port 8080 --db ./metadata.db
//! ```
//!
//! Or embed in your application:
//! ```ignore
//! use gameta_server::Server;
//!
//! let server = Server::new(config).await?;
//! server.run().await?;
//! ```

pub mod config;
pub mod http;
pub mod service;

// Re-export core types
pub use gameta_core::{
    MetadataProvider, MetadataSource, ProductMetadata, SearchResult,
};
pub use gameta_lib::DLSiteProvider;

use config::ServerConfig;

/// The metadata server
pub struct Server {
    config: ServerConfig,
}

impl Server {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    /// Run the server (blocking)
    pub async fn run(&self) -> anyhow::Result<()> {
        tracing::info!("Starting gameta_server on port {}", self.config.port);

        // TODO: Implement server startup
        // 1. Initialize database connection
        // 2. Start background workers
        // 3. Start HTTP server

        Ok(())
    }

    /// Get server configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }
}
