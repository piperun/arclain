use crate::backends::fallback_backend::FallbackBackend;
use crate::backends::libarchive_backend::LibarchiveBackend;
use crate::backends::sevenz_cli::SevenZipCli;
use crate::backends::unrar_backend::UnrarBackend;
use crate::ArchiveBackend;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// Selects the appropriate backend for a given archive
#[derive(Clone)]
pub struct BackendSelector {
    backend_mode: String,
}

impl BackendSelector {
    /// Create a new backend selector with the given mode
    /// - "native": Use native backends where available (RAR uses UnrarBackend, 7z uses SevenZBackend, others use 7z CLI)
    /// - "cli": Always use 7z.exe CLI for all formats
    pub fn new(backend_mode: String) -> Self {
        Self { backend_mode }
    }

    /// Create a selector that always uses native backends
    pub fn new_native() -> Self {
        Self::new("native".to_string())
    }

    /// Create a selector that always uses CLI
    pub fn new_cli() -> Self {
        Self::new("cli".to_string())
    }

    /// Auto-select backend based on archive extension and configured mode
    pub fn select(&self, archive: &Path) -> Result<Arc<dyn ArchiveBackend>> {
        if self.backend_mode == "cli" {
            // Always use 7z.exe if CLI mode is selected
            let backend = Arc::new(SevenZipCli::detect(None)?);
            info!(
                "Selected {} backend for {} (mode: CLI)",
                backend.name(),
                archive.display()
            );
            return Ok(backend);
        }

        // Native mode: use native backends where available
        let ext = archive
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let backend: Arc<dyn ArchiveBackend> = match ext.as_str() {
            "rar" | "r00" | "r01" | "r02" | "r03" => {
                // Try native RAR backend first, fallback to 7z CLI
                let primary = Arc::new(UnrarBackend::new());
                let fallback = Arc::new(SevenZipCli::detect(None)?);
                let backend = Arc::new(FallbackBackend::new(primary, fallback));
                info!(
                    "Selected {} → {} fallback chain for {} (extension: .{})",
                    backend.name(),
                    "7z (CLI)",
                    archive.display(),
                    ext
                );
                backend
            }
            "7z" => {
                // Use 7z CLI directly for 7z files (faster than native sevenz-rust2)
                let backend = Arc::new(SevenZipCli::detect(None)?);
                info!(
                    "Selected {} backend for {} (extension: .{}) - using CLI for optimal performance",
                    backend.name(),
                    archive.display(),
                    ext
                );
                backend
            }
            _ => {
                // Try libarchive for other formats (zip, tar, tar.gz, etc.), fallback to 7z.exe CLI
                let primary = Arc::new(LibarchiveBackend::new());
                let fallback = Arc::new(SevenZipCli::detect(None)?);
                let backend = Arc::new(FallbackBackend::new(primary, fallback));
                info!(
                    "Selected {} → {} fallback chain for {} (extension: .{})",
                    "Libarchive (Native)",
                    "7z (CLI)",
                    archive.display(),
                    ext
                );
                backend
            }
        };

        Ok(backend)
    }

    /// Create default selector (use native backends)
    pub fn default() -> Self {
        Self::new_native()
    }
}
