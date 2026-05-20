//! Archive operations - listing, reading, writing files

use super::AppState;
use anyhow::Result;
use arclain_core::utilities::auto_password_for;
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

impl AppState {
    pub fn list_archive(&mut self, path: &Path) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());
        self.signals.tabs.get().active().current_password.set(None);

        // Select appropriate backend based on file extension
        let backend = self.backend_selector.select(path)?;

        let info = match backend.list(path, None) {
            Ok(info) => {
                if info.headers_encrypted {
                    debug!("Archive has encrypted headers, trying auto-password");
                    let archive_name = path.to_str();
                    let pw = auto_password_for(&self.pass_rules, archive_name, &vec![]);
                    if let Some(ref password) = pw {
                        info!("Attempting to open encrypted archive with auto-detected password");
                        match backend.list(path, Some(password)) {
                            Ok(new_info) => {
                                self.signals.tabs.get().active().current_password.set(Some(password.clone()));
                                new_info
                            }
                            Err(e) => {
                                warn!("Failed to open archive with auto-detected password: {}", e);
                                info
                            }
                        }
                    } else {
                        debug!("No auto-password found for encrypted archive");
                        info
                    }
                } else {
                    debug!("Archive opened without password");
                    info
                }
            }
            Err(e) => {
                debug!("Initial listing failed, trying with auto-password: {}", e);
                let archive_name = path.to_str();
                let pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
                if let Some(ref password) = pw {
                    info!("Attempting to open archive with auto-detected password");
                    let info = backend.list(path, Some(password))?;
                    self.signals.tabs.get().active().current_password.set(Some(password.clone()));
                    info
                } else {
                    debug!("No auto-password found");
                    return Err(e);
                }
            }
        };
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        let archive_path = Some(path.to_path_buf());
        let tab = self.signals.tabs.get().active().clone();
        tab.archive_path.set(archive_path.clone());

        // Update archive_extras signal — the encryption fields are
        // backend-reported, not derivable from entries+path, so they
        // live on the dedicated extras signal. The full ArchiveInfo
        // is now a Computed that reads entries + archive_path +
        // archive_extras (post 2026-05-20 Tier 2 item 6 audit).
        tab.archive_extras.set(crate::core::operations::archive::ArchiveExtras {
            archive_encrypted: info.encrypted,
            headers_encrypted: info.headers_encrypted,
            encryption_method: info.encryption_method.clone(),
        });
        crate::core::operations::navigation_signals::reset_navigation(&self.signals);

        // Update reactive signals for async UI updates
        tab.entries
            .set(std::sync::Arc::new(info.entries.clone()));

        // Attempt password detection with correct archive context
        if tab.current_password.read().is_none() {
            let archive_name = archive_path.as_ref().and_then(|p| p.to_str());
            debug!(
                "Attempting auto-password detection for archive: {:?}",
                archive_name
            );
            let detected_pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
            if let Some(ref pwd) = detected_pw {
                info!("Auto-detected password for archive (length: {})", pwd.len());
                tab.current_password.set(Some(pwd.clone()));
            } else if info.encrypted {
                warn!("Archive is encrypted but no password was auto-detected from rules");
            } else {
                debug!("No password needed - archive is not encrypted");
            }
        } else {
            info!(
                "Password already set (length: {})",
                tab.current_password
                    .get()
                    .as_ref()
                    .map(|p| p.len())
                    .unwrap_or(0)
            );
        }

        // Store OnArchiveOpen event for deferred dispatch
        tab.metadata.set(None);
        if self.plugin_event_sender.is_some() {
            use arclain_plugins::PluginEvent;
            let event = PluginEvent::OnArchiveOpen {
                path: path.to_string_lossy().into_owned(),
                kind: info.archive_kind,
                password: tab.current_password.get(),
            };

            self.pending_plugin_event = Some(event);
            tab.ui_ready.set(false);

            info!(
                "Archive opened successfully with {} entries (plugin event pending)",
                tab.entries.get().len()
            );
        }

        if self.plugin_event_sender.is_none() {
            info!(
                "Archive opened successfully with {} entries",
                tab.entries.get().len()
            );
        }

        // Create Archive handle and store in signal for session operations
        // Re-select backend since the one from earlier was consumed
        let backend = self.backend_selector.select(path)?;
        let archive = if let Some(pw) = tab.current_password.get() {
            arclain_core::Archive::with_password(backend, path.to_path_buf(), pw)
        } else {
            arclain_core::Archive::new(backend, path.to_path_buf())
        };
        tab.opened_archive
            .set(Some(Arc::new(RwLock::new(archive))));

        Ok(tab.entries.get().as_ref().clone())
    }

    pub fn list_with_password(
        &mut self,
        path: &Path,
        password: &str,
    ) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Listing archive with manually provided password");
        let backend = self.backend_selector.select(path)?;
        let info = backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        let tab = self.signals.tabs.get().active().clone();
        tab.archive_path.set(Some(path.to_path_buf()));

        // Update archive_extras — see `list_archive` above for the
        // post-Tier 2 (item 6) rationale.
        tab.archive_extras.set(crate::core::operations::archive::ArchiveExtras {
            archive_encrypted: info.encrypted,
            headers_encrypted: info.headers_encrypted,
            encryption_method: info.encryption_method.clone(),
        });
        crate::core::operations::navigation_signals::reset_navigation(&self.signals);
        tab.current_password
            .set(Some(password.to_string()));

        // Store OnArchiveOpen event for deferred dispatch
        if self.plugin_event_sender.is_some() {
            use arclain_plugins::PluginEvent;
            let event = PluginEvent::OnArchiveOpen {
                path: path.to_string_lossy().into_owned(),
                kind: info.archive_kind.clone(),
                password: Some(password.to_string()),
            };

            self.pending_plugin_event = Some(event);
            tab.ui_ready.set(false);
        }

        // Create Archive handle with password and store in signal for session operations
        let archive =
            arclain_core::Archive::with_password(backend, path.to_path_buf(), password.to_string());
        tab.opened_archive
            .set(Some(Arc::new(RwLock::new(archive))));

        Ok(tab.entries.get().as_ref().clone())
    }

    /// Dispatch any pending plugin event after UI has rendered.
    pub fn dispatch_pending_plugin_event(&mut self) {
        if let Some(event) = self.pending_plugin_event.take() {
            debug!("Dispatching deferred plugin event after UI render");

            if let Some(ref sender) = self.plugin_event_sender {
                if let Err(e) = sender.send(event) {
                    warn!("Failed to send deferred event to plugin worker: {}", e);
                }
            }

            self.signals.tabs.get().active().ui_ready.set(true);
        }
    }

    pub fn get_current_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        let tab = self.signals.tabs.get().active().clone();
        tab.navigation
            .get()
            .filter_entries(&tab.entries.get())
    }

    pub fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_files(archive, &files)
    }

    pub fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let archive_name = archive.to_str();
        let auto_pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
        let signal_pw = self.signals.tabs.get().active().current_password.get();
        let pw = signal_pw.as_deref().or(auto_pw.as_deref());
        let backend = self.backend_selector.select(archive)?;
        backend.read_text_file(archive, path_in_archive, pw)
    }

    pub fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.delete_files(archive, files)
    }

    pub fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_or_update_file_from_str(archive, path_in_archive, content)
    }
}
