use crate::PluginCapability;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Failed to load plugin: {0}")]
    LoadError(String),
    #[error("Failed to initialize plugin: {0}")]
    InitError(String),
    #[error("Plugin execution failed: {0}")]
    ExecutionError(String),
    #[error("Plugin unavailable: {0}")]
    Unavailable(String),
    #[error("Capability denied: {0:?}")]
    CapabilityDenied(PluginCapability),
    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("Invalid Wirt package: {0}")]
    InvalidPackage(String),
    #[error("Unsupported plugin package: {0}")]
    Unsupported(String),
    #[error("Plugin package conflict: {0}")]
    Conflict(String),
    #[error("Plugin not found: {0}")]
    NotFound(String),
    #[error("WASM error: {0}")]
    WasmError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("TOML parse error: {0}")]
    TomlError(#[from] toml::de::Error),
}

pub type Result<T> = std::result::Result<T, PluginError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_error_exposes_only_the_redacted_host_reason() {
        let error = PluginError::Unavailable("fuel quota exceeded".to_string());
        assert_eq!(error.to_string(), "Plugin unavailable: fuel quota exceeded");
    }
}
