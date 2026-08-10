use super::wirt_crate::bindings::wirt::plugin::host::{
    ArchiveInfo, DataRequest, DataResult, DataStatus, Host, LogLevel, MetadataSummary,
};
use super::wirt_crate::{sandboxed_wasi_ctx, PluginStoreLimiter, WirtStoreState};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxView, WasiView};

pub struct StubHost {
    table: ResourceTable,
    ctx: WasiCtx,
    store_limiter: PluginStoreLimiter,
    probe: String,
    observed_log_calls: usize,
}

impl StubHost {
    pub fn new() -> Self {
        Self {
            table: ResourceTable::new(),
            ctx: sandboxed_wasi_ctx(),
            store_limiter: PluginStoreLimiter,
            probe: String::new(),
            observed_log_calls: 0,
        }
    }

    pub fn set_probe(&mut self, value: impl Into<String>) {
        self.probe = value.into();
    }

    pub fn probe(&self) -> &str {
        &self.probe
    }

    pub fn observed_log_calls(&self) -> usize {
        self.observed_log_calls
    }
}

impl WasiView for StubHost {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl WirtStoreState for StubHost {
    fn store_limiter(&mut self) -> &mut PluginStoreLimiter {
        &mut self.store_limiter
    }
}

impl Host for StubHost {
    fn log(&mut self, _level: LogLevel, _message: String) {
        self.observed_log_calls += 1;
    }

    fn log_network_activity(&mut self, _message: String) {}

    fn get_setting(&mut self, _key: String) -> Option<String> {
        None
    }

    fn set_setting(&mut self, _key: String, _value: String) {}

    fn request_data(&mut self, _request: DataRequest) -> String {
        "stub-request".to_string()
    }

    fn poll_data(&mut self, _request_id: String) -> DataResult {
        DataResult {
            status: DataStatus::Failed,
            data: None,
            error: Some("unsupported by stub host".to_string()),
        }
    }

    fn has_data(&mut self, _key: String) -> bool {
        false
    }

    fn get_data(&mut self, _key: String) -> Option<Vec<u8>> {
        None
    }

    fn fetch_to_cache(&mut self, _request: DataRequest) -> bool {
        false
    }

    fn play_cached_blob(
        &mut self,
        _key: String,
        _extension: String,
    ) -> std::result::Result<(), String> {
        Err("unsupported by stub host".to_string())
    }

    fn invalidate_cache(&mut self, _key: String) -> bool {
        false
    }

    fn current_archive_info(&mut self) -> Option<ArchiveInfo> {
        None
    }

    fn list_archive_files(&mut self) -> std::result::Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn archive_file_count(&mut self) -> std::result::Result<u64, String> {
        Ok(0)
    }

    fn list_archive_files_page(
        &mut self,
        _offset: u32,
        _limit: u32,
    ) -> std::result::Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn rename_archive(&mut self, _new_name: String) -> std::result::Result<String, String> {
        Err("unsupported by stub host".to_string())
    }

    fn emit_metadata(&mut self, _metadata_json: String) {}

    fn emit_metadata_for_source(&mut self, _source: String, _metadata_json: String) -> bool {
        false
    }

    fn show_message(&mut self, _title: String, _message: String) {}

    fn set_status_message(&mut self, _message: String) {}

    fn list_cached_entries(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn cached_metadata_count(&mut self, _source: String) -> std::result::Result<u64, String> {
        Ok(0)
    }

    fn list_cached_metadata(
        &mut self,
        _source: String,
        _offset: u32,
        _limit: u32,
    ) -> std::result::Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn get_metadata_summaries(&mut self, _ids: Vec<String>) -> Vec<MetadataSummary> {
        Vec::new()
    }

    fn get_metadata_summaries_for_source(
        &mut self,
        _source: String,
        _ids: Vec<String>,
    ) -> std::result::Result<Vec<MetadataSummary>, String> {
        Ok(Vec::new())
    }

    fn get_product_metadata(&mut self, _product_id: String, _source: String) -> Option<String> {
        None
    }

    fn create_file(
        &mut self,
        _filename: String,
        _content: Vec<u8>,
    ) -> std::result::Result<String, String> {
        Err("unsupported by stub host".to_string())
    }
}

impl super::wirt_crate::bindings::wirt::plugin::ui::Host for StubHost {}
impl super::wirt_crate::bindings::wirt::plugin::rules::Host for StubHost {}
impl super::wirt_crate::bindings::wirt::plugin::meta::Host for StubHost {}
