//! Inversion-of-control traits.
//!
//! `arclain_data` historically called into `arclain_core` directly
//! (`MetadataStoreResolver` constructed with `Arc<LibraryService>`,
//! `ContentCache` with `Arc<CacheService>`). That created a hard
//! `data → core` dependency, which prevented `arclain_core` from
//! re-exporting `ContentCache` / `ResourceManager` to UI consumers
//! (audit "UI imports `arclain_data` directly" finding).
//!
//! These traits flip the dep: `arclain_data` declares the surface it
//! needs, `arclain_core` implements it on its services. The cycle is
//! broken; UI now reaches `ContentCache` through `arclain_core`.

use anyhow::Result;
use arclain_db::{CacheEntry, CacheType, ProductMetadata};

/// Read+write surface for the persistent product-metadata store.
/// Implemented by `arclain_core::LibraryService`.
pub trait MetadataReader: Send + Sync {
    /// Look up a metadata row by its full id (e.g. `"dlsite:RJ001"`).
    fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>>;

    /// Persist a metadata row, applying the implementation's quality
    /// guards (refusing to overwrite good data with geo-blocked data,
    /// refusing to downgrade completeness — see
    /// `LibraryService::save_metadata`).
    fn save_metadata(&self, meta: &ProductMetadata) -> Result<()>;
}

/// CRUD surface for the cache-index table that backs `ContentCache`.
/// Implemented by `arclain_core::CacheService`.
pub trait CacheIndex: Send + Sync {
    fn upsert(
        &self,
        key: &str,
        product_id: Option<&str>,
        content_hash: &str,
        source_url: Option<&str>,
        cache_type: CacheType,
        size_bytes: Option<i64>,
    ) -> Result<i64>;

    fn get(&self, key: &str) -> Result<Option<CacheEntry>>;
    fn has(&self, key: &str) -> Result<bool>;
    fn delete(&self, key: &str) -> Result<bool>;
    fn delete_by_pattern(&self, pattern: &str) -> Result<usize>;
    fn update_last_accessed(&self, key: &str) -> Result<()>;
}
