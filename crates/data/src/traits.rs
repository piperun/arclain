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
//!
//! `CacheIndex` is unconditional. `MetadataReader` and the
//! `MetadataStoreResolver` written in its types compile only with the
//! `gameta` feature — without it there is no metadata store to read.

use anyhow::Result;
#[cfg(feature = "gameta")]
use arclain_db::ProductMetadata;
use arclain_db::{CacheEntry, CacheType};

/// Read+write surface for the persistent product-metadata store.
/// Implemented by `arclain_core::LibraryService`.
#[cfg(feature = "gameta")]
pub trait MetadataReader: Send + Sync {
    /// Look up a metadata row by its full id (e.g. `"dlsite:RJ001"`).
    fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>>;

    /// Check for a metadata row without materializing its text fields.
    fn has_metadata(&self, id: &str) -> Result<bool> {
        Ok(self.get_metadata(id)?.is_some())
    }

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

    /// Removes every index row. Complete production indexes should
    /// override this with one transactional statement; the compatibility
    /// implementation is deliberately conservative and refuses an
    /// incomplete view rather than claiming it cleared rows it cannot see.
    fn clear_all(&self) -> Result<()> {
        if !self.has_complete_lru_view() {
            anyhow::bail!("cache index does not support complete enumeration");
        }
        for entry in self.entries_lru()? {
            self.delete(&entry.key)?;
        }
        Ok(())
    }

    /// Check whether any row references a physical content hash. Production
    /// indexes should implement this as an existence query; the compatibility
    /// default preserves third-party implementations.
    fn has_content_hash(&self, content_hash: &str) -> Result<bool> {
        if !self.has_complete_lru_view() {
            // An incomplete view cannot prove that a blob is unreferenced.
            // Fail closed so compatibility implementations leak at worst
            // instead of deleting data still owned by another key.
            return Ok(true);
        }
        Ok(self
            .entries_lru()?
            .iter()
            .any(|entry| entry.content_hash == content_hash))
    }

    /// List entries from least to most recently used for deterministic quota
    /// reconciliation. Custom indexes that do not support quota eviction may
    /// keep the compatibility default.
    fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
        Ok(Vec::new())
    }

    fn entries_lru_page(&self, offset: usize, limit: usize) -> Result<Vec<CacheEntry>> {
        Ok(self
            .entries_lru()?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    fn supports_lru_paging(&self) -> bool {
        false
    }

    fn content_hashes(&self) -> Result<Vec<String>> {
        let mut hashes: Vec<_> = self
            .entries_lru()?
            .into_iter()
            .map(|entry| entry.content_hash)
            .collect();
        hashes.sort();
        hashes.dedup();
        Ok(hashes)
    }

    /// Whether `entries_lru` is a complete view suitable for destructive
    /// reconciliation. Compatibility implementations default to false so a
    /// cache never mistakes an unsupported listing for an empty index.
    fn has_complete_lru_view(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_db::CacheType;

    struct IncompleteIndex;

    impl CacheIndex for IncompleteIndex {
        fn upsert(
            &self,
            _key: &str,
            _product_id: Option<&str>,
            _content_hash: &str,
            _source_url: Option<&str>,
            _cache_type: CacheType,
            _size_bytes: Option<i64>,
        ) -> Result<i64> {
            Ok(1)
        }

        fn get(&self, _key: &str) -> Result<Option<CacheEntry>> {
            Ok(None)
        }

        fn has(&self, _key: &str) -> Result<bool> {
            Ok(false)
        }

        fn delete(&self, _key: &str) -> Result<bool> {
            Ok(false)
        }

        fn delete_by_pattern(&self, _pattern: &str) -> Result<usize> {
            Ok(0)
        }

        fn update_last_accessed(&self, _key: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn incomplete_indexes_conservatively_preserve_unknown_content_hashes() {
        assert!(IncompleteIndex.has_content_hash("possibly-shared").unwrap());
    }
}
