//! Resource management feature
//!
//! Provides unified data access without exposing storage details.
//! Consumers just say "I need this data" - ResourceManager handles
//! whether to fetch from cache, disk, or network.

mod manager;
pub mod types;

pub use manager::ResourceManager;
pub use types::{
    ResourceConfig, ResourceData, ResourceRequest, ResourceSource, ResourceStatus, ResourceType,
    StorageStrategy,
};
