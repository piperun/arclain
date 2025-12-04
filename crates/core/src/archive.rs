use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zeroize::Zeroizing;

use crate::{ArchiveBackend, ArchiveInfo};

/// Archive handle with dependency injection pattern.
/// 
/// This is the canonical way to pass archives around in the codebase.
/// Like a database connector, it encapsulates the backend, file path, and credentials,
/// providing a unified interface for all archive operations.
/// 
/// # Design Philosophy
/// 
/// - **Dependency Injection**: Functions accept `&Archive` instead of raw paths/backends
/// - **Secure by Default**: Passwords are zeroized, never leaked in debug output
/// - **Flexible**: Works with or without passwords, auto-detects encryption
/// - **Ergonomic**: Single type for all archive operations
/// 
/// # Example
/// 
/// ```rust,ignore
/// // Create archive handle with backend
/// let archive = Archive::new(backend, "game.rar");
/// 
/// // Or with password from database
/// let archive = Archive::with_resolver(backend, "game.rar", || {
///     password_db.lookup("game.rar")
/// });
/// 
/// // Pass to any function needing archive access
/// organize_game(&archive, output_path, metadata)?;
/// ```
pub struct Archive {
    backend: Arc<dyn ArchiveBackend>,
    path: PathBuf,
    password: Option<Zeroizing<String>>,
    /// Whether this archive requires a password (detected on first access)
    needs_password: Option<bool>,
}

impl Archive {
    /// Create a new Archive handle without a password
    pub fn new(backend: Arc<dyn ArchiveBackend>, path: impl Into<PathBuf>) -> Self {
        Self {
            backend,
            path: path.into(),
            password: None,
            needs_password: None,
        }
    }

    /// Create a new Archive handle with a password
    pub fn with_password(
        backend: Arc<dyn ArchiveBackend>,
        path: impl Into<PathBuf>,
        password: String,
    ) -> Self {
        Self {
            backend,
            path: path.into(),
            password: Some(Zeroizing::new(password)),
            needs_password: Some(true),
        }
    }

    /// Create Archive with a password resolver (for database lookups)
    pub fn with_resolver<F>(
        backend: Arc<dyn ArchiveBackend>,
        path: impl Into<PathBuf>,
        resolver: F,
    ) -> Self
    where
        F: FnOnce() -> Option<String>,
    {
        let password = resolver().map(Zeroizing::new);
        Self {
            backend,
            path: path.into(),
            needs_password: password.as_ref().map(|_| true),
            password,
        }
    }

    /// Get the archive file path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the backend used by this archive
    pub fn backend(&self) -> &dyn ArchiveBackend {
        self.backend.as_ref()
    }

    /// Get the password as an Option<&str> for passing to backend methods
    pub fn password_ref(&self) -> Option<&str> {
        self.password.as_ref().map(|z| z.as_str())
    }

    /// List archive contents
    pub fn list(&mut self) -> Result<ArchiveInfo> {
        let info = self
            .backend
            .list(&self.path, self.password_ref())
            .context("listing archive contents")?;

        // Cache whether password is needed
        if self.needs_password.is_none() {
            self.needs_password = Some(info.encrypted);
        }

        // If encrypted but no password provided, return error with context
        if info.encrypted && self.password.is_none() {
            return Err(anyhow::anyhow!(
                "Archive '{}' is encrypted but no password was provided",
                self.path.display()
            ));
        }

        Ok(info)
    }

    /// Extract all contents to destination
    pub fn extract_all(&self, dest: &Path) -> Result<()> {
        self.backend
            .extract_all(&self.path, dest, self.password_ref())
            .context("extracting archive")
    }

    /// Extract specific files
    pub fn extract_files(&self, dest: &Path, files: &[String]) -> Result<()> {
        self.backend
            .extract_files(&self.path, dest, files, self.password_ref())
            .context("extracting files from archive")
    }

    /// Extract a directory
    pub fn extract_directory(&self, dest: &Path, dir_path: &str) -> Result<()> {
        self.backend
            .extract_directory(&self.path, dest, dir_path, self.password_ref())
            .context("extracting directory from archive")
    }

    /// Read a text file from the archive
    pub fn read_text_file(&self, path_in_archive: &str) -> Result<String> {
        self.backend
            .read_text_file(&self.path, path_in_archive, self.password_ref())
            .context("reading text file from archive")
    }

    /// Compute CRC32 of an entry
    pub fn crc32_of_entry(&self, path_in_archive: &str) -> Result<String> {
        self.backend
            .crc32_of_entry(&self.path, path_in_archive, self.password_ref())
            .context("computing CRC32 of archive entry")
    }

    /// Check if this archive needs a password
    pub fn needs_password(&self) -> Option<bool> {
        self.needs_password
    }

    /// Check if a password has been provided
    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    /// Update the password (useful if initial password was wrong)
    pub fn set_password(&mut self, password: String) {
        self.password = Some(Zeroizing::new(password));
        self.needs_password = Some(true);
    }

    /// Clear the password
    pub fn clear_password(&mut self) {
        self.password = None;
    }
}

// Custom Debug to avoid leaking password
impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Archive")
            .field("path", &self.path)
            .field("backend", &self.backend.name())
            .field("has_password", &self.has_password())
            .field("needs_password", &self.needs_password)
            .finish()
    }
}