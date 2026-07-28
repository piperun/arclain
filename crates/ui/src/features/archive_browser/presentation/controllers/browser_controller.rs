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
                // pending_open_file lives on the active tab now (post 2026-05-19 audit)
                shared
                    .signals()
                    .tabs
                    .get()
                    .active()
                    .pending_open_file
                    .set(Some(file));
            }
            Action::OpenArchiveInTab(archive_path) => {
                self.handle_open_archive_in_tab(shared, archive_path);
            }
            Action::EditFile(file) => {
                let origin = shared.signals().tabs.get().active().clone();
                self.file_ops.read_text(shared, origin, file);
            }
            Action::DeleteFile(file) => {
                let origin = shared.signals().tabs.get().active().clone();
                self.file_ops.delete_files(shared, origin, vec![file]);
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
                self.file_ops.copy_path(egui_ctx, &file);
            }
            Action::ShowProperties(file) => {
                // Set selection to just this archive entry. Display-relative
                // paths are not unique across folders, so action payloads and
                // selection both use the stable archive-root path.
                let tab = shared.signals().tabs.get().active().clone();
                let entries = tab.browser_entries.get();
                tab.browser_view_state.update(|s| {
                    s.toolbar_state.show_properties_panel = true;
                    s.selection.clear();
                    if let Some(entry) = entries.entries.iter().find(|e| e.archive_path == file) {
                        s.selection.insert(entry.archive_path.clone());
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
            let active_id = shared.signals().tabs.get().active_id();
            crate::core::operations::archive::start_archive_open(
                shared,
                active_id,
                extracted_path,
                None,
            );
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
        let org_tab = shared.signals().tabs.get().active().clone();
        if let Some(archive) = org_tab.archive_path.get() {
            let archive_name = archive
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let rules = self.load_org_rules(shared);
            let profiles = self.load_profiles(shared);
            let entries = org_tab.entries.get().as_ref().clone();
            let metadata = org_tab.game_metadata.get();

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
        let user_config = shared.signals().user_config.get();
        let dlsite_enabled = shared
            .plugin_ui_jobs
            .plugin_snapshot(&user_config)
            .and_then(Result::ok)
            .map(|plugins| {
                plugins.iter().any(|p| {
                    (p.id.eq_ignore_ascii_case("dlsite")
                        || p.id.eq_ignore_ascii_case("dlsite-metadata"))
                        && p.enabled
                })
            })
            .unwrap_or(false);

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
                if let Ok(db_profiles) = arclain_core::list_profiles(&mut conn) {
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
                tracing::info!("Received metadata from plugin: {}", metadata.title);
                shared
                    .signals()
                    .tabs
                    .get()
                    .active()
                    .game_metadata
                    .set(Some(metadata));
            }
            Err(e) => {
                tracing::warn!("Failed to parse metadata JSON: {}", e);
            }
        }
    }
}
