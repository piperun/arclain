//! Toolbar handler for ArclainApp

use super::ArclainApp;
use crate::core::{operations, signals::ToolbarContext};
use crate::shared::components;
use eframe::egui;

pub fn render_toolbar(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Toolbar (only on Main page AND when Archive context is active)
    let should_show_archive_toolbar = if app.page_navigator.is_on_main() {
        matches!(
            app.shared_state
                .signals()
                .tabs
                .get()
                .active()
                .active_toolbar
                .get(),
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
                let archive_loaded = tab.archive_loaded.get();
                // Use selection_count signal for decoupled toolbar state
                let has_selection = tab.selection_count.get() > 0;
                let has_metadata = tab.metadata.read().is_some();
                let toolbar_config = components::toolbar::ToolbarConfig::new(
                    app.shared_state.signals().toolbar_items.get(),
                );
                let mut view_state = tab.browser_view_state.get();

                // Plugin-rendering bridge: shared/ doesn't know about
                // features/plugins, so we hand it closures that do the
                // feature-layer work here in core/. Renderer returns
                // events; dispatcher routes them through the async
                // worker.
                let dispatch_shared = app.shared_state.clone();
                let theme_colors_ref = &app.shared_state.theme.colors;
                let shared_ref = &app.shared_state;
                let mut plugin_renderer = move |ui: &mut egui::Ui,
                                                pid: &str,
                                                elements: &[arclain_plugins::types::PluginUiElement]|
                      -> Vec<(String, String, Option<String>)> {
                    let collected: std::cell::RefCell<
                        Vec<(String, String, Option<String>)>,
                    > = std::cell::RefCell::new(Vec::new());
                    let pid_owned = pid.to_string();
                    {
                        let mut callback: crate::features::plugins::presentation::rendering::UiEventCallback =
                            Box::new(|element_id: &str, value: Option<String>| {
                                collected.borrow_mut().push((
                                    pid_owned.clone(),
                                    element_id.to_string(),
                                    value,
                                ));
                            });
                        crate::features::plugins::presentation::rendering::render_ui_elements(
                            ui,
                            elements,
                            &mut callback,
                            theme_colors_ref,
                            None,
                            Some(shared_ref),
                            Some(pid),
                        );
                    }
                    collected.into_inner()
                };
                let mut plugin_dispatcher = move |plugin_id: String,
                                                  event_id: String,
                                                  value: Option<String>| {
                    crate::features::plugins::presentation::dispatch::dispatch_plugin_event(
                        &dispatch_shared,
                        plugin_id,
                        event_id,
                        value,
                    );
                };

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
                    Some(&app.shared_state),
                    &mut plugin_renderer,
                    &mut plugin_dispatcher,
                );
                tab.browser_view_state.set_if_changed(view_state);

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
                    // merge_dialog is per-tab now (post 2026-05-20 audit B2 follow-up)
                    let open_tab = shared_state.signals().tabs.get().active().clone();
                    let mut merge_dialog = open_tab.merge_dialog.get();

                    // Opening itself now goes through the application
                    // facade (`start_archive_open`), driven by
                    // `crate::core::operation_bridge` -- see that
                    // module for how progress/challenges/completion
                    // route back onto this tab's signals.
                    operations::archive::open_archive_via_file_dialog(
                        &shared_state,
                        Some(&mut merge_dialog),
                    );

                    open_tab.merge_dialog.set(merge_dialog);
                }
                if actions.extract {
                    // extraction_dialog is per-tab now (post 2026-05-20 B3
                    // reframed slice 2). The button is on the active tab's
                    // toolbar, so the dialog lives on the active tab.
                    let active_tab = shared_state.signals().tabs.get().active().clone();
                    let view_state = active_tab.browser_view_state.get();
                    let entries = active_tab.browser_entries.get();

                    // Compute the archive-root paths to extract from the
                    // active tab's selection. Selection lives in the
                    // archive-path-keyed HashSet on BrowserViewState.
                    let selected_paths: Vec<String> = entries
                        .entries
                        .iter()
                        .filter(|e| view_state.selection.contains(&e.archive_path))
                        .map(|e| e.archive_path.clone())
                        .collect();
                    operations::extraction::extract_selected(
                        &shared_state,
                        &active_tab,
                        selected_paths,
                    );
                }
                if actions.extract_all {
                    let active_tab = shared_state.signals().tabs.get().active().clone();
                    operations::extraction::extract_all(&shared_state, &active_tab);
                }
                if actions.add {
                    let mut status_info = shared_state.signals().status_bar.get();
                    operations::file::add_files(&app.shared_state.app_state, &mut status_info);
                    shared_state.signals().status_bar.set(status_info);
                }
                if actions.delete_selected {
                    let t = shared_state.signals().tabs.get().active().clone();
                    let entries = t.browser_entries.get();
                    let view_state = t.browser_view_state.get();
                    let search_text = shared_state.signals().search_text.get();
                    let selected_paths = operations::file::selected_file_paths_for_search(
                        entries.entries.as_ref(),
                        &view_state.selection,
                        &search_text,
                    );

                    crate::features::archive_browser::application::FileOpsService.delete_files(
                        &shared_state,
                        t,
                        selected_paths,
                    );
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
