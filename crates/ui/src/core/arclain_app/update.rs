//! Update handler for ArclainApp

use super::ArclainApp;
use crate::core::navigation::{AppPage, SettingsPage};
use crate::core::{app_lifecycle, app_rendering, operations};
use eframe::egui;

pub fn update_app(app: &mut ArclainApp, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    // DEBUG: Track update frequency
    static LAST_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let last = LAST_LOG.load(std::sync::atomic::Ordering::Relaxed);
    FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if now != last {
        let count = FRAME_COUNT.swap(0, std::sync::atomic::Ordering::Relaxed);
        if count > 10 {
            tracing::warn!("High frame rate detected: {} fps", count);
        }
        LAST_LOG.store(now, std::sync::atomic::Ordering::Relaxed);
    }
    // === Handle files dropped from Explorer ===
    crate::core::arclain_app::drop_handler::handle_drop_events(app, ctx);

    // === Lifecycle: Refresh requests, signals, theme ===
    app_lifecycle::process_refresh_requests(&app.shared_state, ctx);
    app_lifecycle::bind_signals_once(&app.shared_state.app_state, ctx, &mut app._signals_bound);

    // Apply theme only once on init or when dark_mode changes (prevent continuous repaint loop)
    let current_dark_mode = app.shared_state.theme.dark_mode;
    if !app._theme_applied || app._last_dark_mode != current_dark_mode {
        app_lifecycle::apply_theme(&app.shared_state, ctx);
        app._theme_applied = true;
        app._last_dark_mode = current_dark_mode;
    }

    // === Process Hotkey Input ===
    // Check if hotkeys need reloading
    if app.shared_state.signals().hotkeys_updated.get() {
        // Reset signal
        app.shared_state.signals().hotkeys_updated.set(false);

        // Reload manager from current user config
        let config = app.shared_state.signals().user_config.get();
        let bindings_map = if let Some(json) = &config.hotkey_bindings {
            serde_json::from_str::<std::collections::HashMap<String, String>>(json)
                .unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };

        app.hotkey_manager = crate::features::hotkeys::HotkeyManager::from_config(&bindings_map);
        tracing::info!("Reloaded hotkey manager from updated settings");
    }

    let triggered_actions = app_lifecycle::process_hotkey_input(&app.hotkey_manager, ctx);
    for action in triggered_actions {
        use crate::features::hotkeys::HotkeyAction;
        match action {
            HotkeyAction::NavigateBack => {
                // Context-aware back navigation:
                // 1. If on main page with archive loaded, try archive back navigation first
                // 2. If that fails (e.g. at root), navigate UI page back
                let is_on_main = app.page_navigator.is_on_main();
                let archive_loaded = app.shared_state.signals().archive_path.get().is_some();
                let signals = app.shared_state.signals();

                let mut handled = false;
                if is_on_main && archive_loaded {
                    if operations::navigation_signals::navigate_back(signals) {
                        operations::navigation_view::refresh_view_entries(signals);
                        tracing::info!("Archive folder back navigation");
                        handled = true;
                    }
                }

                if !handled {
                    // Navigate back in UI pages
                    app.page_navigator.navigate_back();
                    tracing::info!("UI page back navigation");
                }
            }
            HotkeyAction::NavigateForward => {
                // Context-aware forward navigation:
                // 1. If on main page with archive loaded, try archive forward navigation first
                // 2. If that fails, navigate UI page forward
                let is_on_main = app.page_navigator.is_on_main();
                let archive_loaded = app.shared_state.signals().archive_path.get().is_some();
                let signals = app.shared_state.signals();

                let mut handled = false;
                if is_on_main && archive_loaded {
                    if operations::navigation_signals::navigate_forward(signals) {
                        operations::navigation_view::refresh_view_entries(signals);
                        tracing::info!("Archive folder forward navigation");
                        handled = true;
                    }
                }

                if !handled {
                    // Navigate forward in UI pages
                    app.page_navigator.navigate_forward();
                    tracing::info!("UI page forward navigation");
                }
            }
            HotkeyAction::NavigateUp => {
                // Navigate up one level in archive folder structure
                let mut nav = app.shared_state.signals().navigation.get();
                if !nav.current_path.is_empty() {
                    if let Some(parent) = std::path::Path::new(&nav.current_path)
                        .parent()
                        .and_then(|p| p.to_str())
                    {
                        nav.path_stack.push(nav.current_path.clone());
                        nav.current_path = parent.to_string();
                        nav.forward_stack.clear();
                        app.shared_state.signals().navigation.set(nav);
                        // Re-filter view entries to match new path
                        operations::navigation_view::refresh_view_entries(
                            app.shared_state.signals(),
                        );
                    }
                }
            }
            HotkeyAction::NavigateToRoot => {
                // Navigate to archive root
                let mut nav = app.shared_state.signals().navigation.get();
                if !nav.current_path.is_empty() {
                    nav.path_stack.push(nav.current_path.clone());
                    nav.current_path = String::new();
                    nav.forward_stack.clear();
                    app.shared_state.signals().navigation.set(nav);
                    // Re-filter view entries to match new path
                    operations::navigation_view::refresh_view_entries(app.shared_state.signals());
                }
            }
            HotkeyAction::OpenSettings => {
                app.page_navigator
                    .navigate_to(AppPage::Settings(SettingsPage::Overview));
            }
            HotkeyAction::OpenArchive => {
                // Open file dialog to select archive
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("Archives", &["zip", "7z", "rar"])
                    .pick_file()
                {
                    tracing::info!("Opening archive via hotkey: {}", file.display());
                    // Open the archive directly
                    let mut password_dialog = app.shared_state.signals().password_dialog.get();
                    let mut status_info = app.shared_state.signals().status_bar.get();
                    let mut view_state = app.shared_state.signals().browser_view_state.get();
                    // nav removed
                    let mut archive_info = operations::archive::ArchiveInfo::default();

                    operations::archive::open_archive_by_path(
                        &app.shared_state.app_state,
                        &file,
                        // current_path removed
                        &mut password_dialog,
                        &mut status_info,
                        &mut view_state.view_entries,
                        &mut archive_info,
                    );

                    // navigation set removed
                    app.shared_state
                        .signals()
                        .password_dialog
                        .set(password_dialog);
                    app.shared_state.signals().status_bar.set(status_info);
                    app.shared_state
                        .signals()
                        .browser_view_state
                        .set(view_state);
                    app.shared_state.signals().archive_info.set(archive_info);
                }
            }
            HotkeyAction::Search => {
                tracing::info!("Search hotkey triggered - signaling focus request");
                app.shared_state.signals().search_focus_requested.set(true);
            }
            HotkeyAction::SelectAll => {
                // Select all entries in the file list
                let mut view_state = app.shared_state.signals().browser_view_state.get();
                for entry in &mut view_state.view_entries {
                    entry.selected = true;
                }
                app.shared_state
                    .signals()
                    .browser_view_state
                    .set(view_state);
            }
            HotkeyAction::DeleteSelected => {
                tracing::debug!(
                    "Delete hotkey - not yet implemented (needs archive modification support)"
                );
            }
            HotkeyAction::ExtractSelected | HotkeyAction::ExtractAll => {
                // Log - extract requires proper state wiring
                tracing::debug!("Extract hotkey triggered: {:?}", action);
            }
        }
    }

    // === Lifecycle: Process metadata signal updates from plugins ===
    app_lifecycle::process_metadata_signal(&app.shared_state, &mut app.organization_feature);

    // === Lifecycle: Handle extraction progress from native backends ===
    {
        // Use a block to limit scope of extracted signals
        let mut status = app.shared_state.signals().status_bar.get();
        let mut dialog = app.shared_state.signals().extraction_dialog.get();
        // Skip processing if minimized to avoid UI updates when invisible?
        // Actually, process_extraction_progress updates the signal state from the channel,
        // so it should run regardless of visibility, but maybe minimize update frequency?
        // The original code passed `&mut ops_state.extraction_dialog`.
        app_lifecycle::process_extraction_progress(
            &app.shared_state,
            &mut dialog,
            &mut status.message,
            ctx,
        );
        // app.shared_state.signals().status_bar.set_if_changed(status); // REMOVED: prevents infinite loop
        app.shared_state
            .signals()
            .extraction_dialog
            .set_if_changed(dialog);
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
    app.archive_operations.update_drag_progress(ctx);

    // Process pending file opens (double-click on file in archive)
    if let Some(file_path) = app.shared_state.signals().pending_open_file.get() {
        app.shared_state.signals().pending_open_file.set(None);

        // Use a local StatusBarInfo for the extraction call, then sync to signal
        let mut status_info = app.shared_state.signals().status_bar.get();

        if let Some(nested_archive_path) =
            crate::features::archive_operations::open_file_from_archive(
                &app.shared_state.app_state,
                &file_path,
                &mut status_info,
            )
        {
            app.shared_state.signals().status_bar.set(status_info);
            // It's a nested archive - open it as the current archive
            let mut archive_info = operations::archive::ArchiveInfo::default();

            let mut password_dialog = app.shared_state.signals().password_dialog.get();
            let mut status_info = app.shared_state.signals().status_bar.get();
            let mut view_state = app.shared_state.signals().browser_view_state.get();
            // nav removed

            operations::archive::open_archive_by_path(
                &app.shared_state.app_state,
                &nested_archive_path,
                // current_path removed
                &mut password_dialog,
                &mut status_info,
                &mut view_state.view_entries,
                &mut archive_info,
            );

            // navigation set removed
            app.shared_state
                .signals()
                .password_dialog
                .set(password_dialog);
            app.shared_state.signals().status_bar.set(status_info);
            app.shared_state
                .signals()
                .browser_view_state
                .set(view_state);
            app.shared_state.signals().archive_info.set(archive_info);
        } else {
            app.shared_state.signals().status_bar.set(status_info);
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
                let mut dialog_state = app.shared_state.signals().plugin_dialog_state.get();
                dialog_state.page_stack.clear();
                app.shared_state
                    .signals()
                    .plugin_dialog_state
                    .set(dialog_state);
            }
            app.page_navigator.navigate_to_main();
        }
        app_rendering::TabBarAction::SelectPluginTab { plugin_id, tab_id } => {
            // Set toolbar context to Plugin
            app.shared_state.signals().active_toolbar.set(
                crate::core::signals::ToolbarContext::Plugin(plugin_id.clone()),
            );
            // Open plugin page
            let mut dialog_state = app.shared_state.signals().plugin_dialog_state.get();
            dialog_state.page_stack.clear();
            dialog_state.open_page(&plugin_id, &tab_id);
            app.shared_state
                .signals()
                .plugin_dialog_state
                .set(dialog_state);
        }
        app_rendering::TabBarAction::None => {}
    }

    // Render Toolbar (only on Main page AND when Archive context is active)
    crate::core::arclain_app::toolbar_handler::render_toolbar(app, ctx);

    // === Render Path Bar (Archive context only) ===
    // === Render Path Bar (Archive context only) ===
    let path_bar_action =
        app_rendering::render_path_bar_panel(ctx, &app.shared_state, &app.page_navigator);
    if let app_rendering::PathBarAction::NavigateToPath(path) = path_bar_action {
        app.archive_browser.controller.handle_action(
            crate::features::archive_browser::Action::NavigateToPath(path),
            &app.shared_state,
            app.archive_operations.state_mut(),
            &mut app.organization_feature,
            &mut app.page_navigator,
            ctx,
        );
    }

    // === Render Status Bar ===
    // === Render Status Bar ===
    let mut status_info = app.shared_state.signals().status_bar.get();
    app_rendering::render_status_bar_panel(ctx, &app.shared_state, &mut status_info);
    // Note: We do NOT save status_info back to the signal here using set_if_changed.
    // render_status_bar_panel updates the struct with archive_info stats for display,
    // but persisting it causes an infinite repaint loop if the signal notifies.

    // Render Password Dialog & Rules & Extraction & Edit
    crate::core::arclain_app::dialog_handler::render_dialogs(app, ctx);

    // Render Main Content
    crate::core::arclain_app::content_handler::render_content(app, ctx);

    // Render toast notifications (always on top) & Plugin Dialog & Logs
    crate::core::arclain_app::dialog_handler::render_overlays(app, ctx);
}
