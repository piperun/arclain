//! UI abstraction layer
//!
//! Placeholder for future UI rendering abstraction.

use metastore_types::{ProductMetadata, SearchResult};

/// Trait for UI backends (future implementation)
pub trait UiRenderer: Send + Sync {
    /// Render a metadata detail view
    fn render_metadata_detail(&self, meta: &ProductMetadata);

    /// Render search results list
    fn render_search_results(&self, results: &[SearchResult]);

    /// Render a metadata card/tile
    fn render_metadata_card(&self, meta: &ProductMetadata);
}

// Future: Dioxus, egui, or other implementations
