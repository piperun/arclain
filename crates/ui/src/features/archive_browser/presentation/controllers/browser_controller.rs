//! Browser controller for archive browser feature
//! Coordinates between UI events and application services.

use crate::core::navigation::PageNavigator;
use crate::features::archive_browser::application::{
    DragDropService, FileOpsService, NavigationService,
};
use crate::features::archive_browser::domain::Action;
use crate::features::archive_operations::ArchiveOperationsState;
use crate::features::organization::application::facade as organization_facade;
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
        // Fire-and-forget: materialization, launching the external
        // application (or opening a nested archive), and lease lifetime
        // are all driven asynchronously by `crate::core::operation_bridge`
        // from here on -- see `file_opener`'s own module doc comment.
        crate::features::archive_operations::open_file_from_archive(shared, &archive_path);
    }

    /// Opens the organize panel for the active tab's archive.
    ///
    /// The panel is bound to that tab's archive *session*, not to its
    /// path: everything it then shows (the plan, the archive's own file
    /// list) and the organize it eventually runs all name that session,
    /// so they cannot describe different archives.
    fn handle_organize(
        &self,
        shared: &SharedState,
        organization_feature: &mut OrganizationFeature,
        page_navigator: &mut PageNavigator,
    ) {
        let org_tab = shared.signals().tabs.get().active().clone();
        let (Some(archive), Some(session_id)) =
            (org_tab.archive_path.get(), org_tab.archive_session_id.get())
        else {
            return;
        };
        let Some((app, runtime)) = organization_facade::handles(shared) else {
            return;
        };
        let archive_name = archive
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let rules = self.load_org_rules(shared, app, runtime);
        let profiles = runtime
            .block_on(app.organization_profiles())
            .unwrap_or_else(|error| {
                tracing::warn!("could not load archive profiles: {}", error.summary);
                Vec::new()
            });
        let matching_rule_ids = runtime
            .block_on(app.matching_organization_rule_ids(session_id))
            .unwrap_or_else(|error| {
                tracing::warn!("could not resolve matching rules: {}", error.summary);
                Vec::new()
            });
        let metadata = org_tab.game_metadata.get();

        organization_feature.organizer_page =
            Some(crate::features::organization::OrganizerPage::new(
                crate::features::organization::OrganizePanel::new(
                    session_id,
                    archive_name.clone(),
                    rules,
                    profiles,
                    metadata,
                    &matching_rule_ids,
                ),
            ));

        page_navigator.navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
    }

    /// The rules the panel offers: every saved rule, minus the ones that
    /// need a metadata plugin this installation does not have enabled
    /// (offering a DLsite rule with the DLsite plugin off would offer a
    /// rule that can never resolve its own variables).
    fn load_org_rules(
        &self,
        shared: &SharedState,
        app: &arclain_app::ArclainApp,
        runtime: &tokio::runtime::Runtime,
    ) -> Vec<arclain_app::organization::OrganizationRuleSummary> {
        let dlsite_enabled = shared
            .plugin_ui_jobs
            .plugin_snapshot(shared.signals().plugin_visibility.get())
            .and_then(Result::ok)
            .map(|plugins| {
                plugins.iter().any(|p| {
                    (p.id.eq_ignore_ascii_case("dlsite")
                        || p.id.eq_ignore_ascii_case("dlsite-metadata"))
                        && p.enabled
                })
            })
            .unwrap_or(false);

        runtime
            .block_on(app.organization_rules())
            .unwrap_or_else(|error| {
                tracing::warn!("could not load organization rules: {}", error.summary);
                Vec::new()
            })
            .into_iter()
            .filter(|rule| {
                if rule
                    .trigger
                    .metadata_source
                    .as_deref()
                    .map(|source| source.eq_ignore_ascii_case("dlsite"))
                    .unwrap_or(false)
                {
                    dlsite_enabled
                } else {
                    true
                }
            })
            .collect()
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
