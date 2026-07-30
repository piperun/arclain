//! Toolbar handler for ArclainApp

use super::ArclainApp;
use crate::core::{operations, signals::ToolbarContext};
use crate::features::plugins::presentation::toolbar_item;
use crate::features::process::state::{PipelineDraft, ProcessPageState};
use crate::shared::components;
use arclain_app::operations::pipeline::{CompressionLevelDto, PipelineInputsDto, PipelineStepDto};
use eframe::egui;

/// Seeds the Process page with a fresh single-Convert-step draft over
/// `inputs`, replacing whatever the page held.
///
/// Both convert toolbar actions do exactly this and differ only in what
/// they point it at, so the shape lives in one place -- and it is one
/// place that has to agree with the application's step vocabulary
/// rather than two.
fn seed_convert_draft(state: &mut ProcessPageState, inputs: Option<PipelineInputsDto>) {
    let mut draft = PipelineDraft::default();
    if let Some(inputs) = inputs {
        draft.inputs = inputs;
    }
    draft.steps.push(PipelineStepDto::Convert {
        format: "zip".to_string(),
        compression: CompressionLevelDto::Normal,
    });
    state.draft = draft;
    state.active_preset_name = None;
    state.mark_dirty();
}

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
                let listing = tab.listing.get();
                let can_go_back = listing.can_go_back();
                let can_go_forward = listing.can_go_forward();
                let can_go_up = listing.can_go_up();
                let archive_loaded = tab.archive_loaded.get();
                // Use selection_count signal for decoupled toolbar state
                let has_selection = tab.selection_count.get() > 0;
                let has_metadata = tab.metadata.read().is_some();
                let toolbar_config = components::toolbar::ToolbarConfig::new(
                    app.shared_state.signals().toolbar_items.get(),
                );
                let mut view_state = tab.browser_view_state.get();

                // Plugin-rendering bridge: `shared/` doesn't know about
                // features/plugins, so it hands the plugin half of the
                // toolbar back to the feature that owns it, one item at a
                // time. Composition only -- the drawing, the session
                // lookup and the dispatch all live in
                // `features::plugins::presentation::toolbar_item`.
                let shared_ref = &app.shared_state;
                let mut plugin_renderer =
                    move |ui: &mut egui::Ui, plugin_id: &str, button_id: Option<&str>| {
                        toolbar_item::render_toolbar_item(ui, shared_ref, plugin_id, button_id);
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
                    // Add itself now goes through the application facade
                    // (`start_archive_mutation` with `AddFiles`), driven
                    // by `crate::core::operation_bridge` -- see that
                    // module for how the resulting `SnapshotChanged`/
                    // terminal events route back onto this tab's signals.
                    let active_tab_id = shared_state.signals().tabs.get().active_id();
                    operations::file::add_files(&shared_state, active_tab_id);
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
                    let inputs = shared_state
                        .signals()
                        .tabs
                        .get()
                        .active()
                        .archive_path
                        .get()
                        .map(|path| PipelineInputsDto::Files { paths: vec![path] });
                    seed_convert_draft(&mut app.process_state, inputs);
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
                        // Seeded as a folder, not as the archives in it
                        // right now: the run expands a folder itself, so
                        // anything added between here and Execute is
                        // still picked up.
                        seed_convert_draft(
                            &mut app.process_state,
                            Some(PipelineInputsDto::Folder { path: folder }),
                        );
                        app.page_navigator
                            .navigate_to(crate::core::navigation::AppPage::Process);
                    }
                }
            });
    }
}
