//! Archive operations - listing, reading, writing files

use super::{AppState, PendingPluginEvent};
use crate::core::signals::AppSignals;
use crate::core::tabs::{TabId, TabState};
use anyhow::Result;
use arclain_core::archive::ArchiveKind;
use arclain_core::utilities::auto_password_for;
use arclain_core::ArchiveEntry;
use arclain_plugins::PluginEvent;
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tracing::{debug, info, warn};

fn clear_archive_selection(tab: &TabState) {
    let mut view_state = tab.browser_view_state.get();
    if view_state.selection.clear() {
        tab.browser_view_state.set_if_changed(view_state);
    }
    tab.selection_count.set_if_changed(0);
}

fn publish_archive_entries(tab: &TabState, entries: Vec<ArchiveEntry>) -> Arc<Vec<ArchiveEntry>> {
    let entries = Arc::new(entries);
    tab.entries.set(entries.clone());
    entries
}

fn archive_open_event(
    path: &Path,
    kind: ArchiveKind,
    password: Option<String>,
    entries: Arc<Vec<ArchiveEntry>>,
    tab: &TabState,
) -> PluginEvent {
    PluginEvent::OnArchiveOpen {
        path: path.to_string_lossy().into_owned(),
        kind,
        password,
        entries,
        metadata_signal: tab.metadata.clone(),
    }
}

fn dispatch_pending_plugin_events(
    pending: &mut Vec<PendingPluginEvent>,
    sender: Option<&Sender<PluginEvent>>,
    signals: &AppSignals,
) {
    let tabs = signals.tabs.get();
    for pending_event in pending.drain(..) {
        if let Some(sender) = sender {
            if let Err(error) = sender.send(pending_event.event) {
                warn!("Failed to send deferred event to plugin worker: {}", error);
            }
        }

        if let Some(tab) = tabs.get(pending_event.origin_tab_id) {
            tab.ui_ready.set(true);
        }
    }
}

impl AppState {
    /// List an archive and populate the **named** tab's per-tab signals.
    ///
    /// Pre-tab-aware refactor this used `signals.tabs.get().active()`
    /// to decide which tab to write into — fine for synchronous calls
    /// from the active tab, but catastrophic for the background
    /// `load_archive_into_tab` worker pool that serializes on the
    /// `AppState` mutex. Five concurrent multi-drop opens would all
    /// resolve `active()` to whichever tab was active when each lock
    /// was acquired (usually the last-created one), so all 5 events
    /// wrote their entries + archive_extras + queued plugin event
    /// against the same tab — the per-event metadata-signal handles
    /// in the queued events were therefore all the active tab's
    /// signal, and every plugin emit clobbered the same tab.
    ///
    /// Callers now pass the explicit `target_tab_id`. Active-tab
    /// call sites pass `signals.tabs.get().active_id()`.
    pub fn list_archive(
        &mut self,
        path: &Path,
        target_tab_id: TabId,
    ) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());
        let tab = self
            .signals
            .tabs
            .get()
            .get(target_tab_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "list_archive: target tab {:?} not found in collection",
                    target_tab_id
                )
            })?
            .clone();
        tab.current_password.set(None);

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
                                tab.current_password.set(Some(password.clone()));
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
                    tab.current_password.set(Some(password.clone()));
                    info
                } else {
                    debug!("No auto-password found");
                    return Err(e);
                }
            }
        };
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        let archive_path = Some(path.to_path_buf());
        tab.archive_path.set(archive_path.clone());

        // Update archive_extras signal — the encryption fields are
        // backend-reported, not derivable from entries+path, so they
        // live on the dedicated extras signal. The full ArchiveInfo
        // is now a Computed that reads entries + archive_path +
        // archive_extras (post 2026-05-20 Tier 2 item 6 audit).
        tab.archive_extras
            .set(crate::core::operations::archive::ArchiveExtras {
                archive_encrypted: info.encrypted,
                headers_encrypted: info.headers_encrypted,
                encryption_method: info.encryption_method.clone(),
            });
        tab.navigation
            .set(arclain_core::archive::NavigationState::new());

        // Matching root paths are common across unrelated archives. Reset
        // selection before publishing a replacement so extract/delete state
        // cannot leak from the archive previously shown in this tab.
        clear_archive_selection(&tab);

        // Keep this exact Arc for the plugin event as well as the tab signal.
        let published_entries = publish_archive_entries(&tab, info.entries.clone());

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

        // Queue OnArchiveOpen for deferred dispatch.
        //
        // The event carries per-tab snapshots (entries list,
        // metadata signal handle) captured RIGHT NOW from this
        // tab, so the worker can route the plugin handler's host-
        // function reads (`current_archive_info`,
        // `list_archive_files`) and emit_metadata writes to *this*
        // tab even if subsequent archive opens push more events
        // onto the queue and the user switches tabs by the time
        // the worker processes us.
        tab.metadata.set(None);
        if self.plugin_event_sender.is_some() {
            let event = archive_open_event(
                path,
                info.archive_kind,
                tab.current_password.get(),
                published_entries.clone(),
                &tab,
            );

            self.pending_plugin_events
                .push(PendingPluginEvent::new(target_tab_id, event));
            tab.ui_ready.set(false);

            info!(
                "Archive opened successfully with {} entries (plugin event pending, queue depth {})",
                tab.entries.get().len(),
                self.pending_plugin_events.len()
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
        tab.opened_archive.set(Some(Arc::new(RwLock::new(archive))));

        Ok(published_entries.as_ref().clone())
    }

    /// Re-list a tab's archive after the user has manually entered a
    /// password (from the password dialog). Takes an explicit
    /// `target_tab_id` for the same reason as `list_archive` — the
    /// password unlock can resolve while the user has navigated to a
    /// different tab.
    pub fn list_with_password(
        &mut self,
        path: &Path,
        password: &str,
        target_tab_id: TabId,
    ) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Listing archive with manually provided password");
        let backend = self.backend_selector.select(path)?;
        let info = backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        let tab = self
            .signals
            .tabs
            .get()
            .get(target_tab_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "list_with_password: target tab {:?} not found",
                    target_tab_id
                )
            })?
            .clone();
        tab.archive_path.set(Some(path.to_path_buf()));

        // Update archive_extras — see `list_archive` above for the
        // post-Tier 2 (item 6) rationale.
        tab.archive_extras
            .set(crate::core::operations::archive::ArchiveExtras {
                archive_encrypted: info.encrypted,
                headers_encrypted: info.headers_encrypted,
                encryption_method: info.encryption_method.clone(),
            });
        tab.navigation
            .set(arclain_core::archive::NavigationState::new());
        tab.current_password.set(Some(password.to_string()));

        clear_archive_selection(&tab);
        let published_entries = publish_archive_entries(&tab, info.entries.clone());

        // Queue OnArchiveOpen for deferred dispatch — see the
        // sibling `list_archive` site above for why the payload
        // carries per-tab snapshots of entries + the metadata
        // signal handle.
        if self.plugin_event_sender.is_some() {
            let event = archive_open_event(
                path,
                info.archive_kind.clone(),
                Some(password.to_string()),
                published_entries.clone(),
                &tab,
            );

            self.pending_plugin_events
                .push(PendingPluginEvent::new(target_tab_id, event));
            tab.ui_ready.set(false);
        }

        // Create Archive handle with password and store in signal for session operations
        let archive =
            arclain_core::Archive::with_password(backend, path.to_path_buf(), password.to_string());
        tab.opened_archive.set(Some(Arc::new(RwLock::new(archive))));

        Ok(published_entries.as_ref().clone())
    }

    /// Drain queued plugin events into the worker channel.
    ///
    /// Pre-queue this took a single `Option<PluginEvent>` and sent
    /// at most one event per call — multi-archive drag-drops lost
    /// all but the last open silently. Now we drain the whole Vec
    /// each call so every queued open reaches the worker.
    pub fn dispatch_pending_plugin_event(&mut self) {
        if self.pending_plugin_events.is_empty() {
            return;
        }
        debug!(
            "Dispatching {} deferred plugin event(s) after UI render",
            self.pending_plugin_events.len()
        );

        dispatch_pending_plugin_events(
            &mut self.pending_plugin_events,
            self.plugin_event_sender.as_ref(),
            &self.signals,
        );
    }

    pub fn get_current_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        let tab = self.signals.tabs.get().active().clone();
        tab.navigation.get().filter_entries(&tab.entries.get())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::signals::AppSignals;
    use crate::core::state::PendingPluginEvent;
    use arclain_core::archive::ArchiveKind;
    use arclain_core::ArchiveEntry;
    use arclain_plugins::PluginEvent;
    use std::sync::mpsc;

    fn entry(path: &str) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_string(),
            size: 0,
            packed_size: 0,
            modified: None,
            is_dir: false,
            encrypted: false,
            crc32: None,
        }
    }

    fn event(path: &str, tab: &crate::core::tabs::TabState) -> PluginEvent {
        PluginEvent::OnArchiveOpen {
            path: path.to_string(),
            kind: ArchiveKind::Zip,
            password: None,
            entries: tab.entries.get(),
            metadata_signal: tab.metadata.clone(),
        }
    }

    #[test]
    fn preparing_a_replacement_archive_clears_selection_and_count() {
        let tab = crate::core::tabs::TabState::new(TabId(7));
        tab.browser_view_state.update(|state| {
            state.selection.insert("same/path.txt".to_string());
        });
        tab.selection_count.set(1);

        clear_archive_selection(&tab);

        assert!(tab.browser_view_state.get().selection.is_empty());
        assert_eq!(tab.selection_count.get(), 0);
    }

    #[test]
    fn decrypted_entries_are_the_published_event_and_browser_snapshot() {
        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();
        tab.entries.set(Arc::new(vec![entry("locked-name.txt")]));

        let published = publish_archive_entries(&tab, vec![entry("decrypted-name.txt")]);
        let plugin_event = archive_open_event(
            Path::new("encrypted.zip"),
            ArchiveKind::Zip,
            Some("secret".to_string()),
            published.clone(),
            &tab,
        );
        crate::core::operations::navigation_view::refresh_view_entries_for_tab(&signals, tab.id);

        assert!(Arc::ptr_eq(&published, &tab.entries.get()));
        let PluginEvent::OnArchiveOpen { entries, .. } = plugin_event;
        assert!(Arc::ptr_eq(&published, &entries));
        assert_eq!(published[0].path, "decrypted-name.txt");
        assert_eq!(
            tab.browser_entries.get().entries[0].name,
            "decrypted-name.txt"
        );
    }

    #[test]
    fn queued_plugin_events_keep_order_and_mark_every_origin_tab_ready() {
        let signals = AppSignals::new();
        let first_id = signals.tabs.get().active_id();
        let second_id = {
            let mut tabs = signals.tabs.get();
            let id = tabs.open(None);
            signals.tabs.set(tabs);
            id
        };
        let tabs = signals.tabs.get();
        let first = tabs.get(first_id).unwrap().clone();
        let second = tabs.get(second_id).unwrap().clone();
        first.ui_ready.set(false);
        second.ui_ready.set(false);

        let mut pending = vec![
            PendingPluginEvent::new(first_id, event("first.zip", &first)),
            PendingPluginEvent::new(second_id, event("second.zip", &second)),
            PendingPluginEvent::new(first_id, event("third.zip", &first)),
        ];
        let (sender, receiver) = mpsc::channel();

        dispatch_pending_plugin_events(&mut pending, Some(&sender), &signals);

        let paths = receiver
            .try_iter()
            .map(|event| match event {
                PluginEvent::OnArchiveOpen { path, .. } => path,
            })
            .collect::<Vec<_>>();
        assert_eq!(paths, ["first.zip", "second.zip", "third.zip"]);
        assert!(pending.is_empty());
        assert!(first.ui_ready.get());
        assert!(second.ui_ready.get());
    }
}
