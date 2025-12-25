use crate::backends::fallback_backend::FallbackBackend;
use crate::backends::sevenz_cli::SevenZipCli;
use crate::backends::unrar_backend::UnrarBackend;
use crate::backends::zip_backend::ZipBackend;
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
    /// - "native": Use native backends where available (RAR uses UnrarBackend, ZIP uses ZipBackend, etc.)
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
            "zip" => {
                // Use ZipBackend as primary, 7z CLI as fallback
                let zip_native = Arc::new(ZipBackend::new());
                let sevenz = Arc::new(SevenZipCli::detect(None)?);

                let backend = Arc::new(FallbackBackend::new(zip_native, sevenz));

                info!(
                    "Selected {} → 7z (CLI) fallback chain for {} (extension: .{})",
                    "Zip (Native)",
                    archive.display(),
                    ext
                );
                backend
            }
            "rar" | "r00" | "r01" | "r02" | "r03" => {
                // Try UnRAR Native → UnRAR CLI (if available) → 7z CLI fallback chain
                use crate::backends::unrar_cli_backend::UnrarCli;

                let unrar_native = Arc::new(UnrarBackend::new());
                let sevenz = Arc::new(SevenZipCli::detect(None)?);

                // Check if UnRAR CLI is available (WinRAR or standalone unrar)
                let backend: Arc<dyn ArchiveBackend> = if let Some(unrar_cli) = UnrarCli::detect() {
                    // Full chain: UnRAR Native → UnRAR CLI → 7z CLI
                    let unrar_cli = Arc::new(unrar_cli);

                    // Build: UnRAR CLI → 7z CLI
                    let secondary = Arc::new(FallbackBackend::new(unrar_cli, sevenz));
                    // Build: UnRAR Native → (UnRAR CLI → 7z CLI)
                    let backend = Arc::new(FallbackBackend::new(unrar_native, secondary));

                    info!(
                        "Selected {} → UnRAR (CLI) → 7z (CLI) fallback chain for {} (extension: .{})",
                        backend.name(),
                        archive.display(),
                        ext
                    );
                    backend
                } else {
                    // No UnRAR CLI: UnRAR Native → 7z CLI
                    let backend = Arc::new(FallbackBackend::new(unrar_native, sevenz));

                    info!(
                        "Selected {} → 7z (CLI) fallback chain for {} (extension: .{})",
                        backend.name(),
                        archive.display(),
                        ext
                    );
                    backend
                };

                backend
            }
            "7z" => {
                // Use 7z CLI directly for 7z files
                let backend = Arc::new(SevenZipCli::detect(None)?);
                info!(
                    "Selected {} backend for {} (extension: .{})",
                    backend.name(),
                    archive.display(),
                    ext
                );
                backend
            }
            _ => {
                // For other formats (tar, tar.gz, etc.), use 7z.exe CLI
                let backend = Arc::new(SevenZipCli::detect(None)?);
                info!(
                    "Selected {} backend for {} (extension: .{})",
                    backend.name(),
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
