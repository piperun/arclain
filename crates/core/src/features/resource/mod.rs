//! Resource management feature
//!
//! Provides unified data access without exposing storage details.
//! Consumers just say "I need this data" - ResourceManager handles
//! whether to fetch from cache, disk, or network.
//!
//! # Example
//!
//! ```ignore
//! use arclain_core::features::resource::{ResourceManager, ResourceRequest};
//!
//! let manager = ResourceManager::new(cache, config);
//!
//! // Check if we have it
//! if manager.has("dlsite:RJ123:img_0") {
//!     let data = manager.get("dlsite:RJ123:img_0");
//! } else {
//!     // Fetch from network (manager handles caching)
//!     let request = ResourceRequest::from_url("dlsite:RJ123:img_0", "https://...");
//!     let data = manager.fetch_sync(&request, |url| http_get(url))?;
//! }
//! ```

mod manager;
pub mod types;

pub use manager::ResourceManager;
pub use types::{
    ResourceConfig, ResourceData, ResourceRequest, ResourceSource, ResourceStatus, ResourceType,
    StorageStrategy,
};
