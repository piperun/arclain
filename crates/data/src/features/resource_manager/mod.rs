//! Resource manager feature
//!
//! Provides unified data access without exposing storage details.
//! Consumers just say "I need this data" - ResourceManager handles
//! whether to fetch from cache, disk, or network.

mod manager;

pub use manager::{validate_image, ImageValidation, ResourceManager};
