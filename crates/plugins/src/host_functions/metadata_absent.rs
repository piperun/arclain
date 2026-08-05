//! Metadata host functions with the metadata engine compiled out.
//!
//! Compiled in place of `metadata.rs` when the `gameta` feature is off.
//! The WIT surface must stay identical at both settings, so every entry
//! point the `Host` impl dispatches to exists here with the same
//! signature and answers with the exact absent shape a service-less
//! runtime already produces — same strings, same log levels, same
//! return values. A plugin cannot distinguish "engine compiled out"
//! from "engine present but no LibraryService configured".
//!
//! The emit entry points answer `false` before any source parsing:
//! source validation, write-budget keying and the ProductMetadata
//! mapping are all metadata-engine code, so no absent-shape body may
//! depend on them.

use super::HostFunctions;
use tracing::warn;

impl HostFunctions {
    pub(super) fn impl_emit_metadata(&mut self, _metadata_json: String) -> bool {
        warn!("LibraryService not initialized");
        false
    }

    pub(super) fn impl_emit_metadata_for_source(
        &mut self,
        _source: String,
        _metadata_json: String,
    ) -> bool {
        warn!("LibraryService not initialized");
        false
    }

    pub(super) fn impl_list_cached_entries(&mut self) -> Vec<String> {
        warn!("LibraryService not initialized");
        vec![]
    }

    pub(super) fn impl_cached_metadata_count(&self, _source: String) -> Result<u64, String> {
        Err("LibraryService not initialized".to_string())
    }

    pub(super) fn impl_list_cached_metadata(
        &self,
        _source: String,
        _offset: u32,
        _limit: u32,
    ) -> Result<Vec<String>, String> {
        Err("LibraryService not initialized".to_string())
    }

    pub(super) fn impl_get_metadata_summaries(
        &mut self,
        _ids: Vec<String>,
    ) -> Vec<crate::arclain::plugin::host::MetadataSummary> {
        warn!("LibraryService not initialized");
        Vec::new()
    }

    pub(super) fn impl_get_metadata_summaries_for_source(
        &self,
        _source: String,
        _ids: Vec<String>,
    ) -> Result<Vec<crate::arclain::plugin::host::MetadataSummary>, String> {
        Err("LibraryService not initialized".to_string())
    }

    pub(super) fn impl_get_product_metadata(
        &mut self,
        _product_id: String,
        _source: String,
    ) -> Option<String> {
        warn!("LibraryService not initialized");
        None
    }
}
