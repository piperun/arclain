//! Toolbar handler for ArclainApp

use super::ArclainApp;
use crate::core::{operations, signals::ToolbarContext};
use crate::shared::components;
use eframe::egui;

pub fn render_toolbar(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Toolbar (only on Main page AND when Archive context is active)
    let should_show_archive_toolbar = if app.page_navigator.is_on_main() {
        matches!(
            app.shared_state.signals().active_toolbar.get(),
            ToolbarContext::Archive
        )
    } else {
        false
    };

    if should_show_archive_toolbar {
        egui::TopBottomPanel::top("toolbar_panel")
            .frame(egui::Frame::NONE.fill(app.shared_state.theme.colors.surface_variant))
            .show(ctx, |ui| {
                let nav = app.shared_state.signals().navigation.get();
                let can_go_back = nav.can_go_back();
                let can_go_forward = nav.can_go_forward();
                let can_go_up = nav.can_go_up();
                let archive_loaded = app.shared_state.signals().archive_path.get().is_some();
                // Use selection_count signal for decoupled toolbar state
                let has_selection = app.shared_state.signals().selection_count.get() > 0;
                let has_metadata = app.shared_state.signals().metadata.get().is_some();
                let toolbar_config = components::toolbar::ToolbarConfig::new(
                    app.shared_state.signals().toolbar_items.get(),
                );
                let plugin_manager = app.shared_state.services.plugin_manager.clone();

                let mut view_state = app.shared_state.signals().browser_view_state.get();
                let actions = components::toolbar::render(
                    ui,
                    &app.shared_state.theme,
                    &mut view_state.toolbar_state,
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
                app.shared_state
                    .signals()
                    .browser_view_state
                    .set(view_state);

                // Handle toolbar actions
                let shared_state = app.shared_state.clone();
                use crate::features::archive_browser::Action;

                if actions.go_back {
                    app.archive_browser.controller.handle_action(
                        Action::NavigateBack,
                        &shared_state,
                        app.archive_operations.state_mut(),
                        &mut app.organization_feature,
                        &mut app.page_navigator,
                        ctx,
                    );
                }
                if actions.go_forward {
                    app.archive_browser.controller.handle_action(
                        Action::NavigateForward,
                        &shared_state,
                        app.archive_operations.state_mut(),
                        &mut app.organization_feature,
                        &mut app.page_navigator,
                        ctx,
                    );
                }
                if actions.go_up {
                    app.archive_browser.controller.handle_action(
                        Action::NavigateUp,
                        &shared_state,
                        app.archive_operations.state_mut(),
                        &mut app.organization_feature,
                        &mut app.page_navigator,
                        ctx,
                    );
                }
                if actions.open {
                    let mut archive_info = operations::archive::ArchiveInfo::default();
                    // Sync from signals
                    let mut view_state = shared_state.signals().browser_view_state.get();
                    let mut password_dialog = shared_state.signals().password_dialog.get();
                    let mut status_info = shared_state.signals().status_bar.get();

                    operations::archive::open_archive(
                        &app.shared_state.app_state,
                        &mut view_state.current_path,
                        &mut password_dialog,
                        &mut app._pending_archive_path,
                        &mut status_info,
                        &mut view_state.view_entries,
                        &mut archive_info,
                    );

                    // Sync back to signals
                    shared_state.signals().browser_view_state.set(view_state);
                    shared_state.signals().password_dialog.set(password_dialog);
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().archive_info.set(archive_info);
                }
                if actions.extract {
                    let view_state = shared_state.signals().browser_view_state.get();
                    let ops_state = app.archive_operations.state_mut();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let mut dialog = shared_state.signals().extraction_dialog.get();

                    operations::extraction::extract_selected(
                        &app.shared_state.app_state,
                        &view_state.view_entries,
                        &mut dialog,
                        &mut ops_state.extraction_rx,
                        &mut ops_state.extraction_child,
                        &mut ops_state.extraction_minimized,
                        &mut ops_state.extraction_started,
                        &mut status_info,
                    );
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().extraction_dialog.set(dialog);
                }
                if actions.extract_all {
                    let ops_state = app.archive_operations.state_mut();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let mut dialog = shared_state.signals().extraction_dialog.get();

                    operations::extraction::extract_all(
                        &app.shared_state.app_state,
                        &mut dialog,
                        &mut ops_state.extraction_rx,
                        &mut ops_state.extraction_child,
                        &mut ops_state.extraction_minimized,
                        &mut ops_state.extraction_started,
                        &mut status_info,
                    );
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().extraction_dialog.set(dialog);
                }
                if actions.add {
                    let mut status_info = shared_state.signals().status_bar.get();
                    operations::file::add_files(&app.shared_state.app_state, &mut status_info);
                    shared_state.signals().status_bar.set(status_info);
                }
                if actions.delete_selected {
                    let mut archive_info = operations::archive::ArchiveInfo::default();
                    let mut view_state = shared_state.signals().browser_view_state.get();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let entries_clone = view_state.view_entries.clone();

                    operations::file::delete_selected(
                        &app.shared_state.app_state,
                        &entries_clone,
                        &mut status_info,
                        &mut view_state.view_entries,
                        &mut archive_info,
                    );

                    shared_state.signals().browser_view_state.set(view_state);
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().archive_info.set(archive_info);
                }
                if actions.convert_to_7z {
                    let ops_state = app.archive_operations.state_mut();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let mut dialog = shared_state.signals().conversion_dialog.get();
                    operations::archive::convert_archive(
                        &app.shared_state.app_state,
                        &mut status_info,
                        &mut dialog,
                        &mut ops_state.conversion_rx,
                        &mut ops_state.conversion_child,
                        &mut ops_state.conversion_started,
                    );
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().conversion_dialog.set(dialog);
                }
                if actions.organize_archive {
                    app.archive_browser.controller.handle_action(
                        Action::Organize,
                        &shared_state,
                        app.archive_operations.state_mut(),
                        &mut app.organization_feature,
                        &mut app.page_navigator,
                        ctx,
                    );
                }
            });
    }
}
