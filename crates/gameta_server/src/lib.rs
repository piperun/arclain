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
//! use gameta_server::{Server, ServerConfig};
//!
//! let config = ServerConfig::builder()
//!     .port(8080)
//!     .database_path("./gameta.db")
//!     .build();
//!
//! let server = Server::new(config);
//! server.run().await?;
//! ```

pub mod backup;
pub mod config;
pub mod http;
pub mod service;

// Re-export core types
pub use gameta_core::{MetadataProvider, MetadataSource, ProductMetadata, SearchResult};
pub use gameta_lib::providers::dlsite::DLSiteProvider;

use config::ServerConfig;
use service::{create_service, MetadataService};
use std::net::SocketAddr;
use std::sync::Arc;

/// The metadata server
pub struct Server {
    config: ServerConfig,
    service: Arc<MetadataService>,
}

impl Server {
    /// Create a new server with the given configuration
    pub fn new(config: ServerConfig) -> Self {
        let service = create_service(config.clone());
        Self { config, service }
    }

    /// Initialize the server (database, etc.)
    pub async fn init(&self) -> anyhow::Result<()> {
        self.service.init().await
    }

    /// Run the server (blocking)
    pub async fn run(&self) -> anyhow::Result<()> {
        // Initialize database
        self.init().await?;

        // Create router
        let router = http::create_router(self.service.clone());

        // Bind address
        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr).await?;

        tracing::info!("Starting gameta_server on http://{}", addr);
        tracing::info!("Swagger UI available at http://{}/api/docs", addr);

        // Run server
        axum::serve(listener, router).await?;

        Ok(())
    }

    /// Get server configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Get the metadata service (for testing or custom integration)
    pub fn service(&self) -> &Arc<MetadataService> {
        &self.service
    }
}
