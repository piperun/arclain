//! Update handler for ArclainApp

use super::ArclainApp;
use crate::core::navigation::{AppPage, SettingsPage};
use crate::core::{app_lifecycle, app_rendering, operations};
use eframe::egui;

pub fn update_app(app: &mut ArclainApp, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    // === Lifecycle: Refresh requests, signals, theme ===
    app_lifecycle::process_refresh_requests(&app.shared_state, ctx);
    app_lifecycle::bind_signals_once(&app.shared_state.app_state, ctx, &mut app._signals_bound);
    app_lifecycle::apply_theme(&app.shared_state, ctx);

    // === Lifecycle: Process metadata signal updates from plugins ===
    app_lifecycle::process_metadata_signal(&app.shared_state, &mut app.organization_feature);

    // === Lifecycle: Handle extraction progress from native backends ===
    {
        let ops_state = app.archive_operations.state_mut();
        app_lifecycle::process_extraction_progress(
            &app.shared_state,
            &mut ops_state.extraction_dialog,
            &mut app.status_info.message,
            ctx,
        );
    }

    // === Lifecycle: Update window title ===
    app_lifecycle::update_window_title(
        &app.shared_state,
        &app.page_navigator,
        &mut app._last_window_title,
        ctx,
    );

    // Handle extraction/conversion progress from CLI backends
    app.archive_operations.update_extraction_progress(ctx);
    app.archive_operations.update_conversion_progress(ctx);

    // Process pending file opens (double-click on file in archive)
    if let Some(file_path) = app.archive_operations.state_mut().pending_open_file.take() {
        if let Some(nested_archive_path) =
            crate::features::archive_operations::open_file_from_archive(
                &app.shared_state.app_state,
                &file_path,
                &mut app.status_info,
            )
        {
            // It's a nested archive - open it as the current archive
            let browser_state = app.archive_browser.state_mut();
            let mut archive_info = operations::archive::ArchiveInfo::default();
            operations::archive::open_archive_by_path(
                &app.shared_state.app_state,
                &nested_archive_path,
                &mut browser_state.current_path,
                &mut app.password_feature.password_dialog,
                &mut app.status_info,
                &mut browser_state.entries,
                &mut archive_info,
            );
        }
    }

    // === Render Header Panel ===
    let header_actions = app_rendering::render_header_panel(
        ctx,
        &app.shared_state,
        &app.page_navigator,
        &mut app.header_state,
    );

    // Handle header actions
    if header_actions.theme_toggle {
        app.shared_state.theme.toggle();
    }
    if header_actions.navigate_home {
        app.page_navigator.navigate_to_main();
    }
    if header_actions.navigate_back {
        app.page_navigator.navigate_back();
    }
    if header_actions.navigate_plugins {
        app.page_navigator.navigate_to(AppPage::Plugins);
    }
    if header_actions.navigate_settings {
        app.page_navigator
            .navigate_to(AppPage::Settings(SettingsPage::Overview));
    }
    if header_actions.show_logs {
        app.show_log_viewer = true;
    }

    // === Render Tab Bar Panel ===
    let tab_action =
        app_rendering::render_tab_bar_panel(ctx, &app.shared_state, &mut app.top_tab_bar_state);

    // Handle tab bar actions
    match tab_action {
        app_rendering::TabBarAction::SelectArchiveTab => {
            // Set toolbar context to Archive
            app.shared_state
                .signals()
                .active_toolbar
                .set(crate::core::signals::ToolbarContext::Archive);
            app.shared_state.signals().status_message.set(None);
            // Close any open plugin pages
            {
                let mut dialog_state = app.shared_state.plugin_dialog_state.lock();
                dialog_state.page_stack.clear();
            }
            app.page_navigator.navigate_to_main();
        }
        app_rendering::TabBarAction::SelectPluginTab { plugin_id, tab_id } => {
            // Set toolbar context to Plugin
            app.shared_state.signals().active_toolbar.set(
                crate::core::signals::ToolbarContext::Plugin(plugin_id.clone()),
            );
            // Open plugin page
            let mut dialog_state = app.shared_state.plugin_dialog_state.lock();
            dialog_state.page_stack.clear();
            dialog_state.open_page(&plugin_id, &tab_id);
        }
        app_rendering::TabBarAction::None => {}
    }

    // Render Toolbar (only on Main page AND when Archive context is active)
    crate::core::arclain_app::toolbar_handler::render_toolbar(app, ctx);

    // === Render Path Bar (Archive context only) ===
    let path_bar_action = app_rendering::render_path_bar_panel(ctx, &app.shared_state);
    if let app_rendering::PathBarAction::NavigateToPath(path) = path_bar_action {
        crate::features::archive_browser::navigation::navigate_to_path(
            app.archive_browser.state_mut(),
            &app.shared_state,
            &path,
        );
    }

    // === Render Status Bar ===
    app_rendering::render_status_bar_panel(ctx, &app.shared_state, &mut app.status_info);

    // Render Password Dialog & Rules & Extraction & Edit
    crate::core::arclain_app::dialog_handler::render_dialogs(app, ctx);

    // Render Main Content
    crate::core::arclain_app::content_handler::render_content(app, ctx);

    // Render toast notifications (always on top) & Plugin Dialog & Logs
    crate::core::arclain_app::dialog_handler::render_overlays(app, ctx);
}
