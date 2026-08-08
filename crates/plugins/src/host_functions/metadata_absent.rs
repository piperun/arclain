//! Metadata host functions with the metadata engine compiled out.
//!
//! Compiled in place of `metadata.rs` when the `gameta` feature is off.
//! The WIT surface must stay identical at both settings, so every entry
//! point the `Host` impl dispatches to exists here with the same
//! signature and answers with the absent shape a service-less runtime
//! already produces — same strings, same log levels, same return
//! values.
//!
//! On the service-backed read paths that is indistinguishable from
//! "engine present but no LibraryService configured". Three paths do
//! differ, because the engine does work before or beyond the store
//! lookup:
//!
//! - the emit entry points answer `false` outright, where the engine
//!   parses the payload, keys the write budget, saves through the data
//!   service, fires the UI signal and reports success — none of which
//!   needs a LibraryService;
//! - `cached_metadata_count` reports the missing service first, where
//!   the engine validates the source argument before looking for it;
//! - `get_product_metadata` answers `None`, where the engine falls
//!   through to its cache and server tiers after the store misses.
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
    ) -> Vec<wirt::bindings::wirt::plugin::host::MetadataSummary> {
        warn!("LibraryService not initialized");
        Vec::new()
    }

    pub(super) fn impl_get_metadata_summaries_for_source(
        &self,
        _source: String,
        _ids: Vec<String>,
    ) -> Result<Vec<wirt::bindings::wirt::plugin::host::MetadataSummary>, String> {
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
