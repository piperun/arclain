//! Browser controller for archive browser feature
//! Coordinates between UI events and application services.

use crate::core::navigation::PageNavigator;
use crate::features::archive_browser::application::{
    DragDropService, FileOpsService, NavigationService,
};
use crate::features::archive_browser::domain::Action;
use crate::features::archive_operations::ArchiveOperationsState;
use crate::features::organization::OrganizationFeature;
use crate::shared::SharedState;
use eframe::egui;

pub struct BrowserController {
    nav_service: NavigationService,
    file_ops: FileOpsService,
    drag_drop: DragDropService,
}

impl BrowserController {
    pub fn new() -> Self {
        Self {
            nav_service: NavigationService,
            file_ops: FileOpsService,
            drag_drop: DragDropService,
        }
    }

    pub fn handle_action(
        &self,
        action: Action,
        shared: &SharedState,
        archive_ops_state: &mut ArchiveOperationsState,
        organization_feature: &mut OrganizationFeature,
        page_navigator: &mut PageNavigator,
        egui_ctx: &egui::Context,
    ) {
        match action {
            Action::NavigateToFolder(folder) => {
                self.nav_service
                    .navigate_to_folder(shared.signals(), &folder);
            }
            Action::NavigateToPath(path) => {
                self.nav_service.navigate_to_path(shared.signals(), &path);
            }
            Action::OpenFile(file) => {
                shared.signals().pending_open_file.set(Some(file));
            }
            Action::OpenArchiveInTab(archive_path) => {
                self.handle_open_archive_in_tab(shared, archive_path);
            }
            Action::EditFile(file) => {
                self.file_ops.edit_file(shared, &file);
            }
            Action::DeleteFile(file) => {
                self.file_ops.delete_file(shared, &file);
            }
            Action::Organize => {
                self.handle_organize(shared, organization_feature, page_navigator);
            }
            Action::Metadata(json) => {
                self.handle_metadata(shared, json);
            }
            Action::Extract(file) => {
                self.file_ops.extract(shared, archive_ops_state, &file);
            }
            Action::ExtractTo(file) => {
                shared.signals().status_bar.update(|s| {
                    s.message = format!("Extract to... for '{}' - not yet implemented", file);
                });
            }
            Action::CopyPath(file) => {
                self.file_ops.copy_path(egui_ctx, shared.signals(), &file);
            }
            Action::ShowProperties(file) => {
                shared.signals().browser_view_state.update(|s| {
                    s.toolbar_state.show_properties_panel = true;
                    for entry in &mut s.view_entries {
                        entry.selected = entry.name == file;
                    }
                });
            }
            Action::DragExtract(files) => {
                self.drag_drop
                    .drag_extract(shared, archive_ops_state, files);
            }
            Action::NavigateBack => {
                self.nav_service.navigate_back(shared.signals());
            }
            Action::NavigateForward => {
                self.nav_service.navigate_forward(shared.signals());
            }
            Action::NavigateUp => {
                self.nav_service.navigate_up(shared.signals());
            }
            Action::None => {}
        }
    }

    fn handle_open_archive_in_tab(&self, shared: &SharedState, archive_path: String) {
        // Use a local StatusBarInfo for the extraction call, then sync to signal
        let mut status_info = shared.signals().status_bar.get();

        if let Some(extracted_path) = crate::features::archive_operations::open_file_from_archive(
            &shared.app_state,
            &archive_path,
            &mut status_info,
        ) {
            shared.signals().status_bar.set(status_info);
            let mut archive_info = crate::core::operations::archive::ArchiveInfo::default();

            // Password dialog and other state also in signals now
            let mut password_dialog = shared.signals().password_dialog.get();
            let mut status_info = shared.signals().status_bar.get();
            let mut view_state = shared.signals().browser_view_state.get();
            // nav removed

            crate::core::operations::archive::open_archive_by_path(
                &shared.app_state,
                &extracted_path,
                // current_path removed
                &mut password_dialog,
                &mut status_info,
                &mut view_state.view_entries,
                &mut archive_info,
            );

            // navigation set removed
            shared.signals().password_dialog.set(password_dialog);
            shared.signals().status_bar.set(status_info);
            shared.signals().browser_view_state.set(view_state);
            shared.signals().archive_info.set(archive_info);
        } else {
            shared.signals().status_bar.set(status_info);
        }
    }

    fn handle_organize(
        &self,
        shared: &SharedState,
        organization_feature: &mut OrganizationFeature,
        page_navigator: &mut PageNavigator,
    ) {
        if let Some(archive) = shared.signals().archive_path.get() {
            let archive_name = archive
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let rules = self.load_org_rules(shared);
            let profiles = self.load_profiles(shared);
            let entries = shared.signals().entries.get().as_ref().clone();
            let metadata = shared.signals().game_metadata.get();

            organization_feature.organizer_page =
                Some(crate::features::organization::OrganizerPage::new(
                    crate::features::organization::OrganizePanel::new(
                        archive_name.clone(),
                        entries,
                        rules,
                        profiles,
                        metadata,
                    ),
                ));

            page_navigator.navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
        }
    }

    fn load_org_rules(
        &self,
        shared: &SharedState,
    ) -> Vec<arclain_core::features::organization::OrganizationRule> {
        let mut rules = Vec::new();
        let dlsite_enabled = if let Some(manager) = &shared.services.plugin_manager {
            let mgr = manager.lock();
            mgr.list_plugins().iter().any(|p| {
                (p.id.eq_ignore_ascii_case("dlsite")
                    || p.id.eq_ignore_ascii_case("dlsite-metadata"))
                    && p.enabled
            })
        } else {
            false
        };

        let state = shared.app_state.lock();
        if let Some(dbs) = &state.dbs {
            let pool = &dbs.config_pool;
            if let Ok(loaded) = arclain_core::config::database::list_org_rules(pool) {
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
        rules
    }

    fn load_profiles(
        &self,
        shared: &SharedState,
    ) -> Vec<arclain_core::features::organization::ArchiveProfile> {
        let state = shared.app_state.lock();
        if let Some(dbs) = &state.dbs {
            let pool = &dbs.config_pool;
            if let Ok(mut conn) = pool.get() {
                if let Ok(db_profiles) = arclain_db::list_profiles_diesel(&mut conn) {
                    return db_profiles
                        .iter()
                        .map(arclain_core::features::organization::ArchiveProfile::from_db)
                        .collect();
                }
            }
        }
        // Return default profile if database not available
        vec![arclain_core::features::organization::ArchiveProfile::default()]
    }

    fn handle_metadata(&self, shared: &SharedState, json: String) {
        match serde_json::from_str::<arclain_core::features::organization::GameMetadata>(&json) {
            Ok(metadata) => {
                tracing::info!("Received metadata from plugin: {:?}", metadata.title);
                shared.signals().game_metadata.set(Some(metadata));
            }
            Err(e) => {
                tracing::warn!("Failed to parse metadata JSON: {}", e);
            }
        }
    }
}
