//! Application rendering module
//!
//! Handles rendering of all panels (header, tab bar, toolbar, status bar)
//! and provides the AppRenderContext for passing state to rendering functions.

use crate::core::navigation::PageNavigator;
use crate::shared::{components, SharedState};
use eframe::egui;

/// Actions returned from header panel rendering
pub struct HeaderActions {
    pub navigate_home: bool,
    pub navigate_back: bool,
    pub navigate_plugins: bool,
    pub navigate_settings: bool,
    pub theme_toggle: bool,
    pub show_logs: bool,
}

/// Render the header panel
pub fn render_header_panel(
    ctx: &egui::Context,
    shared_state: &SharedState,
    page_navigator: &PageNavigator,
    header_state: &mut components::HeaderState,
) -> HeaderActions {
    let mut result = HeaderActions {
        navigate_home: false,
        navigate_back: false,
        navigate_plugins: false,
        navigate_settings: false,
        theme_toggle: false,
        show_logs: false,
    };

    egui::TopBottomPanel::top("header_panel")
        .frame(egui::Frame::NONE.fill(shared_state.theme.colors.surface_variant))
        .show(ctx, |ui| {
            let can_go_back = page_navigator.can_go_back();
            let is_on_settings = page_navigator.is_on_settings();

            // Sync UI preferences from AppState (read-only, use signals() helper)
            header_state.show_button_labels = shared_state
                .signals()
                .ui_preferences
                .get()
                .show_button_labels;

            // Sync search_text from signal to HeaderState before render
            let search_before = shared_state.signals().search_text.get();
            header_state.search_text = search_before;

            // Sync search focus request from signal
            let mut search_focus_requested = shared_state.signals().search_focus_requested.get();

            let server_status = shared_state.signals().server_status.get();

            let actions = components::header::render(
                ui,
                &shared_state.theme,
                header_state,
                &mut result.theme_toggle,
                true, // Always show nav buttons
                can_go_back,
                is_on_settings,
                &mut search_focus_requested,
                &server_status,
            );

            // Sync search focus request back to signal (if consumed)
            if search_focus_requested != shared_state.signals().search_focus_requested.get() {
                shared_state
                    .signals()
                    .search_focus_requested
                    .set(search_focus_requested);
            }

            // Sync search_text back to signal if changed
            let current = shared_state.signals().search_text.get();
            if current != header_state.search_text {
                shared_state
                    .signals()
                    .search_text
                    .set(header_state.search_text.clone());
            }

            result.navigate_home = actions.navigate_home;
            result.navigate_back = actions.navigate_back;
            result.navigate_plugins = actions.navigate_plugins;
            result.navigate_settings = actions.navigate_settings;
            result.show_logs = actions.show_logs;
        });

    result
}

/// Actions returned from tab bar rendering
pub enum TabBarAction {
    None,
    SelectArchiveTab,
    SelectPluginTab { plugin_id: String, tab_id: String },
}

/// Render the top tab bar panel
pub fn render_tab_bar_panel(
    ctx: &egui::Context,
    shared_state: &SharedState,
    tab_bar_state: &mut components::top_tab_bar::TopTabBarState,
) -> TabBarAction {
    let mut result = TabBarAction::None;

    egui::TopBottomPanel::top("top_tab_bar_panel")
        .frame(egui::Frame::NONE.fill(shared_state.theme.colors.surface))
        .show(ctx, |ui| {
            // Build combined tabs list: host tabs + plugin tabs
            let mut tabs = vec![components::top_tab_bar::TopTab {
                id: "archive".to_string(),
                label: "Archive".to_string(),
                icon: egui_phosphor::regular::FOLDER_OPEN.to_string(),
                badge: None,
                source: None,
            }];

            // Collect plugin tabs from services (no lock needed)
            {
                if let Some(plugin_manager) = &shared_state.services.plugin_manager {
                    if let Some(pm) = plugin_manager.try_lock() {
                        for (plugin_id, tab_config) in pm.get_all_top_tabs() {
                            tabs.push(components::top_tab_bar::TopTab {
                                id: tab_config.id.clone(),
                                label: tab_config.label,
                                icon: tab_config.icon,
                                badge: tab_config.badge,
                                source: Some(plugin_id),
                            });
                        }
                    }
                }
            }

            // Render tab bar and handle actions
            if let Some(action) = components::top_tab_bar::render(
                ui,
                &shared_state.theme.colors,
                tab_bar_state,
                &tabs,
            ) {
                match action {
                    components::top_tab_bar::TopTabAction::SelectHostTab(id) => {
                        if id == "archive" {
                            result = TabBarAction::SelectArchiveTab;
                        }
                    }
                    components::top_tab_bar::TopTabAction::SelectPluginTab {
                        plugin_id,
                        tab_id,
                    } => {
                        result = TabBarAction::SelectPluginTab { plugin_id, tab_id };
                    }
                }
            }
        });

    result
}

/// Actions returned from toolbar rendering  
/// Note: Not yet wired into arclain_app.rs - kept inline due to complex action handling
#[allow(dead_code)]
#[derive(Default)]
pub struct ToolbarActions {
    pub go_back: bool,
    pub go_forward: bool,
    pub go_up: bool,
    pub open: bool,
    pub extract: bool,
    pub extract_all: bool,
    pub add: bool,
    pub delete_selected: bool,
    pub convert_to_7z: bool,
    pub organize_archive: bool,
}

/// Render the toolbar panel (only when on main page with Archive context)
/// Note: Not yet wired into arclain_app.rs - kept inline due to complex action handling
#[allow(dead_code)]
pub fn render_toolbar_panel(
    ctx: &egui::Context,
    shared_state: &SharedState,
    page_navigator: &PageNavigator,
    toolbar_state: &mut components::toolbar::ToolbarState,
) -> Option<ToolbarActions> {
    // Check if we should show the archive toolbar
    let should_show = if page_navigator.is_on_main() {
        matches!(
            shared_state.signals().active_toolbar.get(),
            crate::core::signals::ToolbarContext::Archive
        )
    } else {
        false
    };

    if !should_show {
        return None;
    }

    let mut result = ToolbarActions::default();

    egui::TopBottomPanel::top("toolbar_panel")
        .frame(egui::Frame::NONE.fill(shared_state.theme.colors.surface_variant))
        .show(ctx, |ui| {
            let nav = shared_state.signals().navigation.get();
            let can_go_back = nav.can_go_back();
            let can_go_forward = nav.can_go_forward();
            let can_go_up = nav.can_go_up();
            let archive_loaded = shared_state.signals().archive_path.get().is_some();
            let has_selection = shared_state.signals().selection_count.get() > 0;
            let has_metadata = shared_state.signals().metadata.get().is_some();
            let toolbar_config =
                components::toolbar::ToolbarConfig::new(shared_state.signals().toolbar_items.get());
            let plugin_manager = shared_state.services.plugin_manager.clone();

            let actions = components::toolbar::render(
                ui,
                &shared_state.theme,
                toolbar_state,
                can_go_back,
                can_go_forward,
                can_go_up,
                archive_loaded,
                has_selection,
                has_metadata,
                Some(&toolbar_config),
                plugin_manager.as_ref(),
                Some(shared_state),
            );

            result.go_back = actions.go_back;
            result.go_forward = actions.go_forward;
            result.go_up = actions.go_up;
            result.open = actions.open;
            result.extract = actions.extract;
            result.extract_all = actions.extract_all;
            result.add = actions.add;
            result.delete_selected = actions.delete_selected;
            result.convert_to_7z = actions.convert_to_7z;
            result.organize_archive = actions.organize_archive;
        });

    Some(result)
}

/// Render the status bar panel
pub fn render_status_bar_panel(
    ctx: &egui::Context,
    shared_state: &SharedState,
    status_info: &mut components::StatusBarInfo,
) {
    egui::TopBottomPanel::bottom("status_bar")
        .exact_height(28.0)
        .frame(
            egui::Frame::NONE
                .fill(shared_state.theme.colors.surface_variant)
                .inner_margin(egui::Margin::symmetric(0, 6)),
        )
        .show(ctx, |ui| {
            let archive_loaded = shared_state.signals().archive_path.get().is_some();

            // Update status info from state
            if archive_loaded {
                let archive_info = shared_state.signals().archive_info.get();
                status_info.file_count = archive_info.file_count;
                status_info.total_size = crate::core::utils::format_size(archive_info.total_size);
                status_info.compressed_size =
                    crate::core::utils::format_size(archive_info.compressed_size);
                status_info.archive_format = archive_info.archive_format;
            }
            let has_metadata = shared_state.signals().metadata.get().is_some();

            let plugin_info = if let Some(manager) = &shared_state.services.plugin_manager {
                let mgr = manager.lock();
                let list = mgr.list_plugins();
                Some(components::status_bar::PluginStatusInfo {
                    total_plugins: list.len(),
                    enabled_plugins: list.iter().filter(|p| p.enabled).count(),
                    has_metadata,
                })
            } else {
                None
            };

            components::status_bar::render(
                ui,
                &shared_state.theme,
                status_info,
                archive_loaded,
                plugin_info.as_ref(),
            );
        });
}

/// Actions returned from path bar rendering
pub enum PathBarAction {
    None,
    NavigateToPath(String),
}

/// Render the path bar panel (Archive context only)
/// Shows breadcrumb navigation between toolbar and content area
pub fn render_path_bar_panel(
    ctx: &egui::Context,
    shared_state: &SharedState,
    page_navigator: &PageNavigator,
) -> PathBarAction {
    // Only show when Archive tab is active and archive is loaded
    let archive_loaded = shared_state.signals().archive_path.get().is_some();
    let is_archive_context = matches!(
        shared_state.signals().active_toolbar.get(),
        crate::core::signals::ToolbarContext::Archive
    );

    // Don't show path bar if on settings page
    if !archive_loaded || !is_archive_context || page_navigator.is_on_settings() {
        return PathBarAction::None;
    }

    let mut result = PathBarAction::None;

    egui::TopBottomPanel::top("path_bar_panel")
        .exact_height(36.0)
        .frame(
            egui::Frame::NONE
                .fill(shared_state.theme.colors.surface_variant)
                .inner_margin(egui::Margin::symmetric(16, 8))
                .stroke(egui::Stroke::new(1.0, shared_state.theme.colors.outline)),
        )
        .show(ctx, |ui| {
            let archive_name = shared_state
                .signals()
                .archive_path
                .get()
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let current_path = shared_state.signals().navigation.get().current_path.clone();

            if let Some(path) = crate::features::archive_browser::presentation::components::file_list::render_breadcrumb(
                ui,
                &shared_state.theme,
                &current_path,
                &archive_name,
            ) {
                result = PathBarAction::NavigateToPath(path);
            }
        });

    result
}
