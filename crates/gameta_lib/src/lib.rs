//! gameta_lib - Pure game metadata parsing library
//!
//! This crate provides parsing-only functionality for extracting metadata
//! from various game distribution platforms. It has NO I/O dependencies.
//!
//! # Supported Platforms
//! - DLSite (RJ/VJ/BJ codes)
//! - Steam (coming soon)
//! - itch.io (coming soon)
//!
//! # Usage
//! ```ignore
//! use gameta_lib::parsers::dlsite;
//!
//! // Parse DLSite data from raw responses
//! let metadata = dlsite::parse_dlsite("RJ123456", Some(api_json), Some(html))?;
//! ```

pub mod detect;
pub mod parsers;
pub mod types;
pub mod urls;

// Re-export commonly used types at crate root
pub use types::{MetadataSource, ParseError, ProductMetadata, SearchResult};
