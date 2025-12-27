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
    };

    egui::TopBottomPanel::top("header_panel")
        .frame(egui::Frame::NONE.fill(shared_state.theme.colors.surface_variant))
        .show(ctx, |ui| {
            let can_go_back = page_navigator.can_go_back();
            let is_on_settings = page_navigator.is_on_settings();

            // Sync UI preferences from AppState
            {
                let state = shared_state.app_state.lock();
                header_state.show_button_labels =
                    state.signals.ui_preferences.get().show_button_labels;
            }

            // Sync search_text from signal to HeaderState before render
            let search_before = shared_state.app_state.lock().signals.search_text.get();
            header_state.search_text = search_before;

            let actions = components::header::render(
                ui,
                &shared_state.theme,
                header_state,
                &mut result.theme_toggle,
                true, // Always show nav buttons
                can_go_back,
                is_on_settings,
            );

            // Sync search_text back to signal if changed
            let state = shared_state.app_state.lock();
            let current = state.signals.search_text.get();
            if current != header_state.search_text {
                state
                    .signals
                    .search_text
                    .set(header_state.search_text.clone());
            }
            drop(state);

            result.navigate_home = actions.navigate_home;
            result.navigate_back = actions.navigate_back;
            result.navigate_plugins = actions.navigate_plugins;
            result.navigate_settings = actions.navigate_settings;
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
        let state = shared_state.app_state.lock();
        matches!(
            state.signals.active_toolbar.get(),
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
            let state = shared_state.app_state.lock();
            let nav = state.signals.navigation.get();
            let can_go_back = nav.can_go_back();
            let can_go_forward = nav.can_go_forward();
            let can_go_up = nav.can_go_up();
            let archive_loaded = state.signals.archive_path.get().is_some();
            let has_selection = false; // TODO: Implement selection tracking
            let has_metadata = state.signals.metadata.get().is_some();
            let toolbar_config =
                components::toolbar::ToolbarConfig::new(state.signals.toolbar_items.get());
            drop(state);
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
            let state = shared_state.app_state.lock();
            let archive_loaded = state.signals.archive_path.get().is_some();

            // Update status info from state
            if archive_loaded {
                let archive_info = state.signals.archive_info.get();
                status_info.file_count = archive_info.file_count;
                status_info.total_size = crate::core::utils::format_size(archive_info.total_size);
                status_info.compressed_size =
                    crate::core::utils::format_size(archive_info.compressed_size);
                status_info.archive_format = archive_info.archive_format;
            }
            let has_metadata = state.signals.metadata.get().is_some();
            drop(state);

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
