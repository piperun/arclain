//! Toolbar handler for ArclainApp

use super::ArclainApp;
use crate::core::{operations, signals::ToolbarContext};
use crate::shared::components;
use eframe::egui;

pub fn render_toolbar(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Toolbar (only on Main page AND when Archive context is active)
    let should_show_archive_toolbar = if app.page_navigator.is_on_main() {
        matches!(
            app.shared_state.signals().tabs.get().active().active_toolbar.get(),
            ToolbarContext::Archive
        )
    } else {
        false
    };

    if should_show_archive_toolbar {
        egui::TopBottomPanel::top("toolbar_panel")
            .frame(egui::Frame::NONE.fill(app.shared_state.theme.colors.surface_variant))
            .show(ctx, |ui| {
                let tab = app.shared_state.signals().tabs.get().active().clone();
                let nav = tab.navigation.get();
                let can_go_back = nav.can_go_back();
                let can_go_forward = nav.can_go_forward();
                let can_go_up = nav.can_go_up();
                let archive_loaded = tab.archive_path.read().is_some();
                // Use selection_count signal for decoupled toolbar state
                let has_selection = tab.selection_count.get() > 0;
                let has_metadata = tab.metadata.read().is_some();
                let toolbar_config = components::toolbar::ToolbarConfig::new(
                    app.shared_state.signals().toolbar_items.get(),
                );
                let plugin_manager = app.shared_state.services.plugin_manager.clone();

                let mut view_state = tab.browser_view_state.get();
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
                tab.browser_view_state.set(view_state);

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
                    let open_tab = shared_state.signals().tabs.get().active().clone();
                    let mut view_state = open_tab.browser_view_state.get();
                    // password_dialog is per-tab now (post 2026-05-20 B3 reframed slice)
                    let mut password_dialog = open_tab.password_dialog.get();
                    let mut status_info = shared_state.signals().status_bar.get();
                    // merge_dialog is per-tab now (post 2026-05-20 audit B2 follow-up)
                    let mut merge_dialog = open_tab.merge_dialog.get();
                    // nav removed

                    operations::archive::open_archive(
                        &app.shared_state.app_state,
                        // current_path removed
                        &mut password_dialog,
                        &mut app._pending_archive_path,
                        &mut status_info,
                        &mut view_state.view_entries,
                        &mut archive_info,
                        Some(&mut merge_dialog),
                    );

                    // Sync back to signals
                    // navigation set removed
                    open_tab.browser_view_state.set(view_state);
                    open_tab.archive_info.set(archive_info);
                    open_tab.password_dialog.set(password_dialog);
                    shared_state.signals().status_bar.set(status_info);
                    open_tab.merge_dialog.set(merge_dialog);
                }
                if actions.extract {
                    let view_state = shared_state.signals().tabs.get().active().browser_view_state.get();
                    let ops_state = app.archive_operations.state_mut();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let mut dialog = shared_state.signals().extraction_dialog().get();

                    operations::extraction::extract_selected(
                        &app.shared_state.app_state,
                        &view_state.view_entries,
                        &mut dialog,
                        &mut ops_state.extraction_rx,
                        &mut ops_state.extraction_child,
                        &mut ops_state.extraction_minimized,
                        &mut ops_state.extraction_started,
                        &mut ops_state.extraction_op_guard,
                        &mut ops_state.extraction_origin_tab,
                        &mut status_info,
                    );
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().extraction_dialog().set(dialog);
                }
                if actions.extract_all {
                    let ops_state = app.archive_operations.state_mut();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let mut dialog = shared_state.signals().extraction_dialog().get();

                    operations::extraction::extract_all(
                        &app.shared_state.app_state,
                        &mut dialog,
                        &mut ops_state.extraction_rx,
                        &mut ops_state.extraction_child,
                        &mut ops_state.extraction_minimized,
                        &mut ops_state.extraction_started,
                        &mut ops_state.extraction_op_guard,
                        &mut ops_state.extraction_origin_tab,
                        &mut status_info,
                    );
                    shared_state.signals().status_bar.set(status_info);
                    shared_state.signals().extraction_dialog().set(dialog);
                }
                if actions.add {
                    let mut status_info = shared_state.signals().status_bar.get();
                    operations::file::add_files(&app.shared_state.app_state, &mut status_info);
                    shared_state.signals().status_bar.set(status_info);
                }
                if actions.delete_selected {
                    let mut archive_info = operations::archive::ArchiveInfo::default();
                    let t = shared_state.signals().tabs.get().active().clone();
                    let mut view_state = t.browser_view_state.get();
                    let mut status_info = shared_state.signals().status_bar.get();
                    let entries_clone = view_state.view_entries.clone();

                    operations::file::delete_selected(
                        &app.shared_state.app_state,
                        &entries_clone,
                        &mut status_info,
                        &mut view_state.view_entries,
                        &mut archive_info,
                    );

                    t.browser_view_state.set(view_state);
                    t.archive_info.set(archive_info);
                    shared_state.signals().status_bar.set(status_info);
                }
                if actions.convert_to_7z {
                    // Navigate to Process page pre-populated with a Convert step.
                    use arclain_core::{
                        CompressionLevel, ConvertFormat, Pipeline, PipelineInput, PipelineStep,
                    };
                    app.process_state.pipeline = Pipeline::default();
                    app.process_state.pipeline.steps.push(PipelineStep::Convert {
                        format: ConvertFormat::Zip,
                        compression: CompressionLevel::Normal,
                        password: None,
                    });
                    if let Some(ap) = shared_state.signals().tabs.get().active().archive_path.get() {
                        app.process_state.pipeline.input =
                            Some(PipelineInput::Files(vec![ap]));
                    }
                    app.process_state.mark_dirty();
                    app.page_navigator
                        .navigate_to(crate::core::navigation::AppPage::Process);
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

                if actions.batch_convert {
                    if let Some(folder) = rfd::FileDialog::new()
                        .set_title("Select folder of archives")
                        .pick_folder()
                    {
                        use arclain_core::{
                            CompressionLevel, ConvertFormat, Pipeline, PipelineInput, PipelineStep,
                        };
                        app.process_state.pipeline = Pipeline::default();
                        app.process_state.pipeline.input = Some(PipelineInput::Folder(folder));
                        app.process_state.pipeline.steps.push(PipelineStep::Convert {
                            format: ConvertFormat::Zip,
                            compression: CompressionLevel::Normal,
                            password: None,
                        });
                        app.process_state.mark_dirty();
                        app.page_navigator
                            .navigate_to(crate::core::navigation::AppPage::Process);
                    }
                }
            });
    }
}
