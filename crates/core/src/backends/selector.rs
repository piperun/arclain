use crate::backends::fallback_backend::FallbackBackend;
use crate::backends::sevenz_backend::SevenZBackend;
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
    ///
    /// In native mode the 7-Zip CLI is optional: it is the last tier of each
    /// format's fallback chain, attached only when it is actually detected.
    /// A machine without 7-Zip still lists and extracts zip/rar/7z through
    /// the native backends. It stays mandatory where it is the *only*
    /// backend -- CLI mode, and formats with no native implementation.
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
                // The CLI is a fallback tier, not a listing dependency: when
                // 7-Zip is absent the native backend serves alone, and
                // operations that genuinely need the CLI (extract, create,
                // convert) detect it themselves at invocation time with their
                // own "7z CLI not found" context.
                let sevenz = SevenZipCli::detect(None).ok().map(Arc::new);

                let backend: Arc<dyn ArchiveBackend> = match sevenz {
                    Some(sevenz) => {
                        let backend = Arc::new(FallbackBackend::new(zip_native, sevenz));

                        info!(
                            "Selected {} → 7z (CLI) fallback chain for {} (extension: .{})",
                            "Zip (Native)",
                            archive.display(),
                            ext
                        );
                        backend
                    }
                    None => {
                        info!(
                            "7-Zip not found; {} runs without a CLI fallback for {}",
                            "Zip (Native)",
                            archive.display()
                        );
                        zip_native
                    }
                };

                backend
            }
            "rar" | "r00" | "r01" | "r02" | "r03" => {
                // Try UnRAR Native → UnRAR CLI (if available) → 7z CLI fallback chain
                use crate::backends::unrar_cli::UnrarCli;

                let unrar_native = Arc::new(UnrarBackend::new());
                // Same reasoning as the zip arm: reading a RAR never needs the
                // 7z CLI, so its absence only shortens the chain.
                let sevenz = SevenZipCli::detect(None).ok().map(Arc::new);

                // Check if UnRAR CLI is available (WinRAR or standalone unrar)
                let unrar_cli = UnrarCli::detect().map(Arc::new);

                let backend: Arc<dyn ArchiveBackend> = match (unrar_cli, sevenz) {
                    (Some(unrar_cli), Some(sevenz)) => {
                        // Full chain: UnRAR Native → UnRAR CLI → 7z CLI.
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
                    }
                    (None, Some(sevenz)) => {
                        // No UnRAR CLI: UnRAR Native → 7z CLI
                        let backend = Arc::new(FallbackBackend::new(unrar_native, sevenz));

                        info!(
                            "Selected {} → 7z (CLI) fallback chain for {} (extension: .{})",
                            backend.name(),
                            archive.display(),
                            ext
                        );
                        backend
                    }
                    (Some(unrar_cli), None) => {
                        // No 7-Zip: UnRAR Native → UnRAR CLI
                        let backend = Arc::new(FallbackBackend::new(unrar_native, unrar_cli));

                        info!(
                            "7-Zip not found; {} → UnRAR (CLI) runs without a 7z CLI fallback for {}",
                            backend.name(),
                            archive.display()
                        );
                        backend
                    }
                    (None, None) => {
                        info!(
                            "7-Zip not found; {} runs without a CLI fallback for {}",
                            unrar_native.name(),
                            archive.display()
                        );
                        unrar_native
                    }
                };

                backend
            }
            "7z" | "exe" | "sfx" => {
                // Use Native 7z → 7z CLI fallback
                // This enables features like signal-based progress/cancel for 7z files
                // while maintaining robustness of CLI for tricky archives or SFX
                let sevenz_native = Arc::new(SevenZBackend::new());
                // Same reasoning as the zip arm; the native 7z backend is
                // full-featured on its own, so losing the CLI tier costs only
                // its robustness on tricky archives and SFX.
                let sevenz_cli = SevenZipCli::detect(None).ok().map(Arc::new);

                let backend: Arc<dyn ArchiveBackend> = match sevenz_cli {
                    Some(sevenz_cli) => {
                        let backend = Arc::new(FallbackBackend::new(sevenz_native, sevenz_cli));

                        info!(
                            "Selected {} → 7z (CLI) fallback chain for {} (extension: .{})",
                            "7z (Native)",
                            archive.display(),
                            ext
                        );
                        backend
                    }
                    None => {
                        info!(
                            "7-Zip not found; {} runs without a CLI fallback for {}",
                            "7z (Native)",
                            archive.display()
                        );
                        sevenz_native
                    }
                };

                backend
            }
            _ => {
                // For other formats (tar, tar.gz, etc.), use 7z.exe CLI.
                // Here the CLI is not a fallback but the only implementation,
                // so its absence is still a hard failure.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    /// Serializes every test below that reads or replaces `PATH`.
    /// `std::env::set_var` is process-global while cargo runs unit tests on
    /// parallel threads: two concurrent swaps could restore each other's
    /// emptied value and leave `PATH` clobbered for the rest of the run, and
    /// a test that only *reads* `PATH` could observe another's empty one.
    static PATH_LOCK: Mutex<()> = Mutex::new(());

    fn lock_path() -> MutexGuard<'static, ()> {
        // A panicking assert inside one of these tests poisons the lock but
        // leaves no shared state broken -- `EmptyPath` restores `PATH` while
        // unwinding -- so the poison is not a reason to fail every later test.
        PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Points `PATH` at an empty directory for as long as it is alive, and
    /// restores the original value on drop -- including while unwinding from
    /// a failed assert, so one failure cannot cascade into the rest of the
    /// run.
    ///
    /// This is total isolation for 7-Zip specifically: `SevenZipCli::detect`
    /// resolves through a `which` lookup over `PATH` and nothing else -- no
    /// config file, no database, no well-known install directories. (Its
    /// `UnrarCli::detect` neighbour is *not* PATH-only on Windows, where it
    /// also probes WinRAR/scoop/chocolatey locations, so the RAR assertions
    /// below are written to hold whether or not an UnRAR CLI is found.)
    struct EmptyPath {
        _guard: MutexGuard<'static, ()>,
        _dir: TempDir,
        original: Option<OsString>,
    }

    impl EmptyPath {
        fn set() -> Self {
            let guard = lock_path();
            let dir = tempfile::tempdir().expect("create an empty directory to point PATH at");
            let original = std::env::var_os("PATH");
            std::env::set_var("PATH", dir.path());
            Self {
                _guard: guard,
                _dir: dir,
                original,
            }
        }
    }

    impl Drop for EmptyPath {
        fn drop(&mut self) {
            match self.original.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    /// The control every no-7-Zip test depends on: if 7-Zip were still
    /// detectable, those tests would pass for the wrong reason.
    fn assert_sevenzip_is_undetectable() {
        assert!(
            SevenZipCli::detect(None).is_err(),
            "control: 7-Zip must be undetectable for this test to mean anything"
        );
    }

    /// Listing an archive never invokes the CLI, so selection must not need
    /// it: a machine without 7-Zip can still open zip/rar/7z.
    #[test]
    fn native_selection_succeeds_without_sevenzip() {
        let _path = EmptyPath::set();
        assert_sevenzip_is_undetectable();

        for archive in ["a.zip", "a.rar", "a.7z"] {
            assert!(
                BackendSelector::new_native()
                    .select(Path::new(archive))
                    .is_ok(),
                "{archive} must select a backend with no 7-Zip present"
            );
        }
    }

    /// Proof that the chain really degraded rather than the CLI sneaking in
    /// anyway: `FallbackBackend` unions its two tiers' capabilities, so an
    /// attached 7z CLI (`full_featured`) would make zip and rar writable.
    /// Native zip and rar are read-only; native 7z is full-featured alone.
    #[test]
    fn zip_and_rar_degrade_to_their_read_only_native_backend_without_sevenzip() {
        let _path = EmptyPath::set();
        assert_sevenzip_is_undetectable();

        let selector = BackendSelector::new_native();
        let zip = selector.select(Path::new("a.zip")).expect("select zip");
        let rar = selector.select(Path::new("a.rar")).expect("select rar");
        let sevenz = selector.select(Path::new("a.7z")).expect("select 7z");

        assert!(
            zip.capabilities().is_read_only(),
            "no CLI tier means the native read-only zip backend serves alone"
        );
        assert!(
            rar.capabilities().is_read_only(),
            "no CLI tier means only read-only RAR backends remain"
        );
        assert!(
            sevenz.capabilities().can_create,
            "the native 7z backend is full-featured on its own"
        );
    }

    /// CLI mode *is* the CLI -- nothing to degrade to.
    #[test]
    fn cli_mode_still_requires_sevenzip() {
        let _path = EmptyPath::set();
        assert_sevenzip_is_undetectable();

        assert!(BackendSelector::new_cli()
            .select(Path::new("a.zip"))
            .is_err());
    }

    /// Formats with no native backend (tar and friends) still fail up front:
    /// there the CLI is the only implementation, not a fallback tier.
    #[test]
    fn cli_only_formats_still_require_sevenzip() {
        let _path = EmptyPath::set();
        assert_sevenzip_is_undetectable();

        let selector = BackendSelector::new_native();
        assert!(selector.select(Path::new("a.tar")).is_err());
        assert!(selector.select(Path::new("a.tar.gz")).is_err());
    }

    /// With 7-Zip present the chains are exactly what they always were: the
    /// unioned `full_featured` capabilities show the CLI tier is still
    /// attached to zip and rar.
    #[test]
    fn zip_and_rar_keep_the_cli_fallback_when_sevenzip_is_present() {
        let _guard = lock_path();
        if SevenZipCli::detect(None).is_err() {
            // No 7-Zip on this machine; there is no "present" case to assert.
            return;
        }

        let selector = BackendSelector::new_native();
        let zip = selector.select(Path::new("a.zip")).expect("select zip");
        let rar = selector.select(Path::new("a.rar")).expect("select rar");

        assert!(
            zip.capabilities().can_create,
            "7-Zip present -> the zip chain still unions in the CLI's capabilities"
        );
        assert!(
            rar.capabilities().can_create,
            "7-Zip present -> the rar chain still unions in the CLI's capabilities"
        );
    }
}
