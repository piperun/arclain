//! Application rendering module
//!
//! Handles rendering of all panels (header, tab bar, toolbar, status bar)
//! and provides the AppRenderContext for passing state to rendering functions.

use crate::core::navigation::PageNavigator;
use crate::shared::components::search_palette::{self, SearchHit, SearchPaletteAction, TabSummary};
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
    /// A unified-search result was activated (switch tab / jump to file).
    pub search_action: Option<SearchPaletteAction>,
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
        search_action: None,
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

            // Build unified-search hits from live tab + entry signals. The
            // matching itself is pure (search_palette::build_hits); here we
            // just snapshot each tab's code/title/maker/file and the active
            // archive's entry paths.
            let col = shared_state.signals().tabs.get();
            let active_id = col.active_id();
            let tab_summaries: Vec<TabSummary> = col
                .tabs()
                .iter()
                .map(|t| {
                    let file = t
                        .archive_path
                        .get()
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let (code, title, maker) = match t.game_metadata.get() {
                        Some(m) => (
                            m.product_id,
                            m.title.unwrap_or_else(|| t.display_title()),
                            m.creator.unwrap_or_default(),
                        ),
                        None => (String::new(), t.display_title(), String::new()),
                    };
                    TabSummary {
                        id: t.id,
                        code,
                        title,
                        maker,
                        file,
                        // Files plus directories, synthesized ancestors
                        // included -- agrees with the session's own
                        // `ArchiveSnapshot::entry_count`. (The pre-facade
                        // count was the backend's raw row count, which
                        // omitted implied ancestor folders.)
                        entry_count: t.inventory.get().entry_count(),
                        active: t.id == active_id,
                    }
                })
                .collect();
            // File paths only, matching `ArclainApp::archive_file_paths`'s
            // scope for the same search -- the inventory held on the tab
            // IS the per-(session, revision) cache, so no per-frame
            // facade call happens here. (The pre-facade input was the
            // backend's raw listing, which also carried any explicitly
            // listed directory row; a *named but empty* directory is no
            // longer findable by name, while every non-empty one still
            // matches through the paths beneath it.)
            let active_inventory = col.active().inventory.get();
            let active_paths: Vec<&str> = active_inventory
                .entries()
                .iter()
                .filter(|entry| entry.kind != arclain_app::archive::EntryKind::Directory)
                .map(|entry| entry.path.as_str())
                .collect();
            let active_code = tab_summaries
                .iter()
                .find(|s| s.active)
                .map(|s| s.code.clone())
                .unwrap_or_default();
            let search_hits: Vec<SearchHit> = search_palette::build_hits(
                &header_state.search_text,
                &tab_summaries,
                &active_paths,
            );

            let header_inputs = components::header::HeaderInputs {
                show_nav_buttons: true, // Always show nav buttons
                can_go_back,
                is_on_settings,
                server_status: &server_status,
                search_hits: &search_hits,
                active_code: &active_code,
            };
            let actions = components::header::render(
                ui,
                &shared_state.theme,
                header_state,
                &mut result.theme_toggle,
                &mut search_focus_requested,
                &header_inputs,
            );

            result.search_action = actions.search_action;

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

            if let Some(Ok((_, top_tabs))) = shared_state.plugin_ui_jobs.chrome_snapshot() {
                for (plugin_id, tab_config) in top_tabs.iter() {
                    tabs.push(components::top_tab_bar::TopTab {
                        id: tab_config.id.clone(),
                        label: tab_config.label.clone(),
                        icon: tab_config.icon.clone(),
                        badge: tab_config.badge.clone(),
                        source: Some(plugin_id.clone()),
                    });
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

/// Render the status bar panel
///
/// Post 2026-05-20 Tier 2 item 7 this no longer mutates `status_info`
/// — the archive count/size/format fields it used to copy into the
/// struct now live on the per-tab `Computed<ArchiveInfo>` and the
/// child render() reads them directly. The signature stays `&` (no
/// mut) so callers can drop their pre-render-clone / post-render-set
/// dance.
pub fn render_status_bar_panel(
    ctx: &egui::Context,
    shared_state: &SharedState,
    status_info: &components::StatusBarInfo,
) {
    egui::TopBottomPanel::bottom("status_bar")
        // Tall enough to fit a pill-style chip (Chips widget renders
        // as Frame + Label, ~22px content height); previous 28px
        // squeezed the chip and made centering unreliable.
        .exact_height(36.0)
        .frame(
            egui::Frame::NONE
                .fill(shared_state.theme.colors.surface_variant)
                .inner_margin(egui::Margin::symmetric(0, 6)),
        )
        .show(ctx, |ui| {
            // Audit P9/P10: prefer `read()` (RwLock guard, zero-copy)
            // over `get()` (full clone) when we only need a flag —
            // metadata in particular is `Option<serde_json::Value>`
            // and can be KBs.
            let tab = shared_state.signals().tabs.get().active().clone();
            let archive_loaded = tab.archive_loaded.get();

            // Post 2026-05-20 Tier 2 item 7 the count/size/format
            // fields live on the per-tab Computed<ArchiveInfo>, no
            // longer mirrored into status_info. Pull a fresh derived
            // value only when an archive is loaded.
            let archive_info = if archive_loaded {
                Some(tab.archive_info.get())
            } else {
                None
            };
            let has_metadata = tab.metadata.read().is_some();

            // Status bar only needs counts. Use the cheap status_summary
            // path so we don't clone every plugin's manifest per frame
            // (audit finding P5).
            let plugin_info = shared_state
                .plugin_ui_jobs
                .chrome_snapshot()
                .and_then(Result::ok)
                .map(|(summary, _)| components::status_bar::PluginStatusInfo {
                    total_plugins: summary.total,
                    enabled_plugins: summary.enabled,
                    has_metadata,
                });

            // The current "selected" item is whatever metadata the
            // host has stored from the most recent emit_metadata call.
            // The chip in the status bar surfaces it to the user so
            // they know both Organizer and Process will use this entry.
            let selected_item = tab.game_metadata.get();

            components::status_bar::render(
                ui,
                &shared_state.theme,
                status_info,
                archive_info.as_ref(),
                plugin_info.as_ref(),
                selected_item.as_ref(),
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
    let tab = shared_state.signals().tabs.get().active().clone();
    let archive_loaded = tab.archive_loaded.get();
    let is_archive_context = matches!(
        tab.active_toolbar.get(),
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
            let tab = shared_state.signals().tabs.get().active().clone();
            let archive_name = tab
                .archive_path
                .get()
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let current_path = tab.listing.get().current_path().to_string();

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
