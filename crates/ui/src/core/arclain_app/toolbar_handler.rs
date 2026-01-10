//! Toolbar handler for ArclainApp

use super::ArclainApp;
use crate::core::{operations, signals::ToolbarContext};
use crate::shared::components;
use eframe::egui;

pub fn render_toolbar(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Toolbar (only on Main page AND when Archive context is active)
    let should_show_archive_toolbar = if app.page_navigator.is_on_main() {
        let state = app.shared_state.app_state.lock();
        matches!(state.signals.active_toolbar.get(), ToolbarContext::Archive)
    } else {
        false
    };

    if should_show_archive_toolbar {
        egui::TopBottomPanel::top("toolbar_panel")
            .frame(egui::Frame::NONE.fill(app.shared_state.theme.colors.surface_variant))
            .show(ctx, |ui| {
                let state = app.shared_state.app_state.lock();
                let nav = state.signals.navigation.get();
                let can_go_back = nav.can_go_back();
                let can_go_forward = nav.can_go_forward();
                let can_go_up = nav.can_go_up();
                let archive_loaded = state.signals.archive_path.get().is_some();
                drop(state);
                let has_selection = app
                    .archive_browser
                    .state_mut()
                    .entries
                    .iter()
                    .any(|e| e.selected);
                let state = app.shared_state.app_state.lock();
                let has_metadata = state.signals.metadata.get().is_some();
                let toolbar_config =
                    components::toolbar::ToolbarConfig::new(state.signals.toolbar_items.get());
                drop(state);
                let plugin_manager = app.shared_state.services.plugin_manager.clone();

                let actions = components::toolbar::render(
                    ui,
                    &app.shared_state.theme,
                    &mut app.archive_browser.state_mut().toolbar_state,
                    can_go_back,
                    can_go_forward,
                    can_go_up,
                    archive_loaded,
                    has_selection,
                    has_metadata,
                    Some(&toolbar_config),
                    plugin_manager.as_ref(),
                    Some(&app.shared_state),
                );

                // Handle toolbar actions
                let shared_state = app.shared_state.clone();

                if actions.go_back {
                    crate::features::archive_browser::navigation::navigate_back(
                        app.archive_browser.state_mut(),
                        &shared_state,
                    );
                }
                if actions.go_forward {
                    crate::features::archive_browser::navigation::navigate_forward(
                        app.archive_browser.state_mut(),
                        &shared_state,
                    );
                }
                if actions.go_up {
                    crate::features::archive_browser::navigation::navigate_up(
                        app.archive_browser.state_mut(),
                        &shared_state,
                    );
                }
                if actions.open {
                    let mut archive_info = operations::archive::ArchiveInfo::default();
                    let browser_state = app.archive_browser.state_mut();
                    operations::archive::open_archive(
                        &app.shared_state.app_state,
                        &mut browser_state.current_path,
                        &mut app.password_feature.password_dialog,
                        &mut app._pending_archive_path,
                        &mut app.status_info,
                        &mut browser_state.entries,
                        &mut archive_info,
                    );
                }
                if actions.extract {
                    let browser_state = app.archive_browser.state_mut();
                    let ops_state = app.archive_operations.state_mut();
                    operations::extraction::extract_selected(
                        &app.shared_state.app_state,
                        &browser_state.entries,
                        &mut ops_state.extraction_dialog,
                        &mut ops_state.extraction_rx,
                        &mut ops_state.extraction_child,
                        &mut ops_state.extraction_minimized,
                        &mut ops_state.extraction_started,
                        &mut app.status_info,
                    );
                }
                if actions.extract_all {
                    let ops_state = app.archive_operations.state_mut();
                    operations::extraction::extract_all(
                        &app.shared_state.app_state,
                        &mut ops_state.extraction_dialog,
                        &mut ops_state.extraction_rx,
                        &mut ops_state.extraction_child,
                        &mut ops_state.extraction_minimized,
                        &mut ops_state.extraction_started,
                        &mut app.status_info,
                    );
                }
                if actions.add {
                    operations::file::add_files(&app.shared_state.app_state, &mut app.status_info);
                }
                if actions.delete_selected {
                    let mut archive_info = operations::archive::ArchiveInfo::default();
                    let browser_state = app.archive_browser.state_mut();
                    let entries_clone = browser_state.entries.clone();
                    operations::file::delete_selected(
                        &app.shared_state.app_state,
                        &entries_clone,
                        &mut app.status_info,
                        &mut browser_state.entries,
                        &mut archive_info,
                    );
                }
                if actions.convert_to_7z {
                    let ops_state = app.archive_operations.state_mut();
                    operations::archive::convert_archive(
                        &app.shared_state.app_state,
                        &mut app.status_info,
                        &mut ops_state.conversion_dialog,
                        &mut ops_state.conversion_rx,
                        &mut ops_state.conversion_child,
                        &mut ops_state.conversion_started,
                    );
                }
                if actions.organize_archive {
                    let state = app.shared_state.app_state.lock();
                    if let Some(archive) = state.signals.archive_path.get() {
                        let archive_name = archive
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        drop(state);

                        // Load rules directly from DB and filter by enabled plugins
                        let mut rules = Vec::new(); // Default empty
                        {
                            // Check enabled plugins (specifically DLsite) from services
                            let dlsite_enabled =
                                if let Some(manager) = &app.shared_state.services.plugin_manager {
                                    let mgr = manager.lock();
                                    mgr.list_plugins().iter().any(|p| {
                                        p.id.eq_ignore_ascii_case("dlsite-metadata") && p.enabled
                                    })
                                } else {
                                    false
                                };

                            let state = app.shared_state.app_state.lock();

                            if let Some(dbs) = &state.dbs {
                                let pool = &dbs.config_pool;
                                if let Ok(loaded) =
                                    arclain_core::config::database::list_org_rules(pool)
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

                        // Initialize panel
                        let state = app.shared_state.app_state.lock();
                        let entries = state.signals.entries.get().as_ref().clone();
                        let metadata = state.signals.game_metadata.get();
                        drop(state);

                        app.organization_feature.organizer_page =
                            Some(crate::features::organization::OrganizerPage::new(
                                crate::features::organization::OrganizePanel::new(
                                    archive_name.clone(),
                                    entries,
                                    rules,
                                    metadata,
                                ),
                            ));

                        app.page_navigator
                            .navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                    }
                }
            });
    }
}
