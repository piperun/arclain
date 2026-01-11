//! Archive Browser feature action handling
//!
//! This module defines the actions that can be triggered by the archive browser UI
//! and provides the handler context for processing them.

use crate::core::navigation::PageNavigator;
use crate::core::operations;
use crate::features::archive_operations::ArchiveOperationsState;
use crate::features::file_editing::FileEditDialog;
use crate::features::organization::OrganizationFeature;
use crate::features::password_management::dialogs::PasswordDialog;
use crate::shared::components::file_list::FileEntry;
use crate::shared::components::StatusBarInfo;
use crate::shared::SharedState;

/// Actions that can be triggered by the archive browser UI
#[derive(Debug, Clone)]
pub enum Action {
    /// Navigate into a folder within the archive
    NavigateToFolder(String),
    /// Navigate to a specific path in the archive
    NavigateToPath(String),
    /// Open/preview a file from the archive
    OpenFile(String),
    /// Open a nested archive in a new tab
    OpenArchiveInTab(String),
    /// Edit a text file from the archive
    EditFile(String),
    /// Delete a file from the archive
    DeleteFile(String),
    /// Open the organize panel
    Organize,
    /// Metadata JSON received from a plugin
    Metadata(String),
    /// Extract a single file to default location
    Extract(String),
    /// Extract a file to a user-selected location
    ExtractTo(String),
    /// Copy the file path to clipboard
    CopyPath(String),
    /// Show file properties panel
    ShowProperties(String),
    /// No action
    None,
}

/// Context required for handling archive browser actions
/// This struct decouples action handlers from ArclainApp
pub struct ActionContext<'a> {
    pub shared: &'a SharedState,
    pub browser_state: &'a mut super::ArchiveBrowserState,
    pub archive_ops_state: &'a mut ArchiveOperationsState,
    pub status_info: &'a mut StatusBarInfo,
    pub password_dialog: &'a mut PasswordDialog,
    pub edit_dialog: &'a mut FileEditDialog,
    pub organization_feature: &'a mut OrganizationFeature,
    pub page_navigator: &'a mut PageNavigator,
    pub egui_ctx: &'a egui::Context,
}

impl<'a> ActionContext<'a> {
    /// Handle navigation actions (simple, self-contained)
    pub fn handle_navigation(&mut self, action: &Action) -> bool {
        match action {
            Action::NavigateToFolder(folder) => {
                super::navigation::navigate_to_folder(self.browser_state, self.shared, folder);
                true
            }
            Action::NavigateToPath(path) => {
                super::navigation::navigate_to_path(self.browser_state, self.shared, path);
                true
            }
            Action::ShowProperties(file) => {
                self.browser_state.toolbar_state.show_properties_panel = true;
                for entry in &mut self.browser_state.entries {
                    entry.selected = entry.name == *file;
                }
                true
            }
            Action::CopyPath(file) => {
                let nav = self.shared.signals().navigation.get();
                let full_path = if nav.current_path.is_empty() {
                    file.clone()
                } else {
                    format!("{}/{}", nav.current_path, file)
                };
                self.egui_ctx.copy_text(full_path);
                true
            }
            _ => false, // Not a navigation action
        }
    }

    /// Handle simple actions that only need SharedState access
    pub fn handle_simple(&mut self, action: &Action) -> bool {
        match action {
            Action::Metadata(json) => {
                // Parse metadata JSON and store in state
                match serde_json::from_str::<arclain_core::features::organization::GameMetadata>(
                    json,
                ) {
                    Ok(metadata) => {
                        tracing::info!("Received metadata from plugin: {:?}", metadata.title);
                        self.shared.signals().game_metadata.set(Some(metadata));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse metadata JSON: {}", e);
                    }
                }
                true
            }
            Action::DeleteFile(file) => {
                // Get archive path from signals
                let archive_path = self.shared.signals().archive_path.get();
                if let Some(archive) = archive_path {
                    let state = self.shared.app_state.lock();
                    // Select backend for this archive
                    match state.backend_selector.select(&archive) {
                        Ok(backend) => {
                            drop(state); // Release lock before operation
                                         // Attempt to delete the file
                            match backend.delete_files(&archive, &[file.clone()]) {
                                Ok(()) => {
                                    tracing::info!("Deleted file from archive: {}", file);
                                    self.status_info.message = format!("Deleted: {}", file);
                                    // Refresh entries after deletion
                                    if let Ok(info) = backend.list(&archive, None) {
                                        self.shared
                                            .signals()
                                            .entries
                                            .set(std::sync::Arc::new(info.entries));
                                        // Update browser state entries
                                        self.browser_state.entries = self
                                            .shared
                                            .signals()
                                            .entries
                                            .get()
                                            .iter()
                                            .map(crate::core::utils::convert_to_file_entry)
                                            .collect();
                                    }
                                }
                                Err(e) => {
                                    let msg = format!("Failed to delete file: {}", e);
                                    tracing::error!("{}", msg);
                                    self.status_info.message = msg;
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("No backend for archive: {}", e);
                            tracing::error!("{}", msg);
                            self.status_info.message = msg;
                        }
                    }
                } else {
                    self.status_info.message = "No archive loaded".to_string();
                }
                true
            }
            _ => false, // Not a simple action
        }
    }

    /// Handle complex actions requiring access to other subsystems
    pub fn handle_complex(&mut self, action: &Action) -> bool {
        match action {
            Action::OpenFile(file) => {
                self.archive_ops_state.pending_open_file = Some(file.clone());
                true
            }
            Action::Extract(file) => {
                // Extract single file to default location
                // Create a temporary selection for just this file
                let entries = vec![FileEntry {
                    name: file.clone(),
                    selected: true,
                    size: String::new(),
                    compressed: String::new(),
                    ratio: String::new(),
                    modified: String::new(),
                    crc32: String::new(),
                    encrypted: false,
                    is_folder: false,
                }];
                operations::extraction::extract_selected(
                    &self.shared.app_state,
                    &entries,
                    &mut self.archive_ops_state.extraction_dialog,
                    &mut self.archive_ops_state.extraction_rx,
                    &mut self.archive_ops_state.extraction_child,
                    &mut self.archive_ops_state.extraction_minimized,
                    &mut self.archive_ops_state.extraction_started,
                    self.status_info,
                );
                true
            }
            Action::OpenArchiveInTab(archive_path) => {
                // Extract nested archive to temp and open as current archive
                if let Some(extracted_path) =
                    crate::features::archive_operations::open_file_from_archive(
                        &self.shared.app_state,
                        archive_path,
                        self.status_info,
                    )
                {
                    // Open the extracted archive as the current archive
                    let mut archive_info = crate::core::operations::archive::ArchiveInfo::default();
                    crate::core::operations::archive::open_archive_by_path(
                        &self.shared.app_state,
                        &extracted_path,
                        &mut self.browser_state.current_path,
                        self.password_dialog,
                        self.status_info,
                        &mut self.browser_state.entries,
                        &mut archive_info,
                    );
                }
                true
            }
            Action::EditFile(file) => {
                self.edit_dialog.show = true;
                self.edit_dialog.full_path_in_archive = file.clone();
                self.edit_dialog.name_input = file.clone();

                if let Some(archive) = self.shared.signals().archive_path.get() {
                    let state = self.shared.app_state.lock();
                    match state.read_text_file(&archive, file) {
                        Ok(content) => {
                            self.edit_dialog.content = content.clone();
                            self.edit_dialog.original_content = content;
                        }
                        Err(e) => {
                            let msg = format!("Failed to read file: {}", e);
                            crate::core::utils::log_failure("FileEdit", &msg);
                            self.status_info.message = msg;
                        }
                    }
                }
                true
            }
            Action::Organize => {
                // Trigger organization flow
                if let Some(archive) = self.shared.signals().archive_path.get() {
                    let archive_name = archive
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();

                    // Load rules directly from DB and filter by enabled plugins
                    let mut rules = Vec::new(); // Default empty
                    {
                        // Check enabled plugins (specifically DLsite) from services
                        let dlsite_enabled =
                            if let Some(manager) = &self.shared.services.plugin_manager {
                                let mgr = manager.lock();
                                mgr.list_plugins()
                                    .iter()
                                    .any(|p| p.id.eq_ignore_ascii_case("dlsite") && p.enabled)
                            } else {
                                false
                            };

                        let state = self.shared.app_state.lock();

                        if let Some(dbs) = &state.dbs {
                            let pool = &dbs.config_pool;
                            if let Ok(loaded) = arclain_core::config::database::list_org_rules(pool)
                            {
                                rules = loaded
                                    .into_iter()
                                    .filter(|r| {
                                        if r.trigger
                                            .metadata_source
                                            .as_deref()
                                            .map(|s| s.eq_ignore_ascii_case("dlsite"))
                                            .unwrap_or(false)
                                        {
                                            dlsite_enabled
                                        } else {
                                            true
                                        }
                                    })
                                    .collect();
                            }
                        }
                    }

                    let entries = self.shared.signals().entries.get().as_ref().clone();
                    let metadata = self.shared.signals().game_metadata.get();

                    self.organization_feature.organizer_page =
                        Some(crate::features::organization::OrganizerPage::new(
                            crate::features::organization::OrganizePanel::new(
                                archive_name.clone(),
                                entries,
                                rules,
                                metadata,
                            ),
                        ));

                    self.page_navigator
                        .navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                }
                true
            }
            Action::ExtractTo(file) => {
                self.status_info.message =
                    format!("Extract to... for '{}' - not yet implemented", file);
                true
            }
            _ => false, // Not a complex action handled here
        }
    }
}

// Re-export for backwards compatibility (type alias)
pub type ArchiveBrowserAction = Action;
