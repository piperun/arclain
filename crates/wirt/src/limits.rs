pub const MAX_PLUGIN_METADATA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PLUGIN_GUEST_DATA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LINEAR_MEMORY_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_TABLE_ELEMENTS: usize = 100_000;
// `ResourceLimiter::instances` counts the component's adapter-internal core
// instances, not user-visible plugin instances. Each Store still owns exactly
// one `PluginWorld`; 32 accommodates the 20 core instances used by current
// components while bounding malformed components.
pub const MAX_CORE_INSTANCES: usize = 32;
pub const MAX_TABLES: usize = 8;
pub const MAX_MEMORIES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreQuotaKind {
    Memory,
    Table,
}

#[derive(Debug)]
pub struct StoreQuotaExceeded {
    pub kind: StoreQuotaKind,
}

impl std::fmt::Display for StoreQuotaExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("plugin store resource quota exceeded")
    }
}

impl std::error::Error for StoreQuotaExceeded {}

#[derive(Debug, Default)]
pub struct PluginStoreLimiter;

impl wasmtime::ResourceLimiter for PluginStoreLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > MAX_LINEAR_MEMORY_BYTES {
            return Err(wasmtime::Error::new(StoreQuotaExceeded {
                kind: StoreQuotaKind::Memory,
            }));
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > MAX_TABLE_ELEMENTS {
            return Err(wasmtime::Error::new(StoreQuotaExceeded {
                kind: StoreQuotaKind::Table,
            }));
        }
        Ok(true)
    }

    fn instances(&self) -> usize {
        MAX_CORE_INSTANCES
    }

    fn tables(&self) -> usize {
        MAX_TABLES
    }

    fn memories(&self) -> usize {
        MAX_MEMORIES
    }
}

pub fn metadata_value_within_limit(value: &serde_json::Value) -> bool {
    struct LimitWriter {
        written: usize,
    }

    impl std::io::Write for LimitWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.written.saturating_add(buffer.len()) > MAX_PLUGIN_METADATA_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "metadata publication limit exceeded",
                ));
            }
            self.written += buffer.len();
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    serde_json::to_writer(&mut LimitWriter { written: 0 }, value).is_ok()
}
