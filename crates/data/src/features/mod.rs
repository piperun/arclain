//! Feature modules for the data crate
//!
//! Each feature module encapsulates a specific capability:
//! - `api`: Plugin/UI facing DataService
//! - `content_cache`: Binary content caching (images, etc.)
//! - `resolver`: Data source resolvers (trait-based)
//! - `resource_manager`: Storage orchestration

pub mod api;
pub mod content_cache;
pub mod resolver;
pub mod resource_manager;
pub mod streaming_download;
