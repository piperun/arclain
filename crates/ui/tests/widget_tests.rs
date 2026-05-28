//! Widget render and interaction tests using egui_kittest
//!
//! These tests verify that UI components render without panicking
//! and that interactive elements respond to user input correctly.

use egui_kittest::Harness;

// =============================================================================
// Switch
// =============================================================================

#[test]
fn switch_renders_without_panic() {
    let mut harness = Harness::new_ui_state(
        |ui, value: &mut bool| {
            ui.add(arclain_widgets::ToggleSwitch::new(value));
        },
        false,
    );

    harness.run();
    assert!(!*harness.state());
}

// =============================================================================
// SearchBar
// =============================================================================

#[test]
fn search_bar_renders_with_hint() {
    let mut harness = Harness::new_ui_state(
        |ui, query: &mut String| {
            ui.add(
                arclain_ui::shared::components::search_bar::SearchBar::new(query)
                    .hint("Search files..."),
            );
        },
        String::new(),
    );

    harness.run();
    assert!(harness.state().is_empty());
}

#[test]
fn search_bar_renders_with_custom_width() {
    let mut harness = Harness::new_ui_state(
        |ui, query: &mut String| {
            ui.add(
                arclain_ui::shared::components::search_bar::SearchBar::new(query).width(400.0),
            );
        },
        String::new(),
    );

    harness.run();
}

// =============================================================================
// TopTabBar
// =============================================================================

#[test]
fn top_tab_bar_renders_with_tabs() {
    use arclain_ui::shared::components::top_tab_bar::*;

    let tabs = vec![
        TopTab {
            id: "browser".into(),
            label: "Browser".into(),
            icon: "FOLDER_OPEN".into(),
            badge: None,
            source: None,
        },
        TopTab {
            id: "search".into(),
            label: "Search".into(),
            icon: "MAGNIFYING_GLASS".into(),
            badge: None,
            source: None,
        },
    ];

    let mut harness = Harness::new_ui_state(
        move |ui, state: &mut TopTabBarState| {
            let colors = arclain_theme::ThemeColors::dark();
            render(ui, &colors, state, &tabs);
        },
        TopTabBarState::new("browser"),
    );

    harness.run();
    assert_eq!(harness.state().selected_tab, "browser");
}

#[test]
fn top_tab_bar_renders_empty() {
    use arclain_ui::shared::components::top_tab_bar::*;

    let tabs: Vec<TopTab> = vec![];

    let mut harness = Harness::new_ui_state(
        move |ui, state: &mut TopTabBarState| {
            let colors = arclain_theme::ThemeColors::dark();
            render(ui, &colors, state, &tabs);
        },
        TopTabBarState::default(),
    );

    harness.run();
}

#[test]
fn top_tab_bar_renders_with_badge() {
    use arclain_plugins::BadgeConfig;
    use arclain_ui::shared::components::top_tab_bar::*;

    let tabs = vec![TopTab {
        id: "alerts".into(),
        label: "Alerts".into(),
        icon: "INFO".into(),
        badge: Some(BadgeConfig {
            count: Some(5),
            dot: false,
            color: "red".into(),
        }),
        source: None,
    }];

    let mut harness = Harness::new_ui_state(
        move |ui, state: &mut TopTabBarState| {
            let colors = arclain_theme::ThemeColors::dark();
            render(ui, &colors, state, &tabs);
        },
        TopTabBarState::new("alerts"),
    );

    harness.run();
}

// =============================================================================
// Header
// =============================================================================

#[test]
fn header_renders_with_nav_buttons() {
    use arclain_ui::shared::components::header::*;

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut HeaderState| {
            let theme = arclain_theme::AppTheme::new(true);
            let mut toggle = false;
            let mut focus = false;
            render(
                ui,
                &theme,
                state,
                &mut toggle,
                true,  // show_nav_buttons
                true,  // can_go_back
                false, // is_on_settings
                &mut focus,
                &arclain_ui::core::signals::ServerConnectionStatus::Offline,
                &[],
                "",
            );
        },
        HeaderState::default(),
    );

    harness.run();
}

#[test]
fn header_renders_without_nav_buttons() {
    use arclain_ui::shared::components::header::*;

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut HeaderState| {
            let theme = arclain_theme::AppTheme::new(false);
            let mut toggle = false;
            let mut focus = false;
            render(
                ui,
                &theme,
                state,
                &mut toggle,
                false, // show_nav_buttons
                false, // can_go_back
                false, // is_on_settings
                &mut focus,
                &arclain_ui::core::signals::ServerConnectionStatus::Offline,
                &[],
                "",
            );
        },
        HeaderState::default(),
    );

    harness.run();
}

#[test]
fn header_renders_on_settings_page() {
    use arclain_ui::shared::components::header::*;

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut HeaderState| {
            let theme = arclain_theme::AppTheme::new(true);
            let mut toggle = false;
            let mut focus = false;
            render(
                ui,
                &theme,
                state,
                &mut toggle,
                true, // show_nav_buttons
                true, // can_go_back
                true, // is_on_settings
                &mut focus,
                &arclain_ui::core::signals::ServerConnectionStatus::Offline,
                &[],
                "",
            );
        },
        HeaderState::default(),
    );

    harness.run();
}

// =============================================================================
// Search palette
// =============================================================================

#[test]
fn search_palette_renders_tab_and_file_results_without_panic() {
    use arclain_ui::core::tabs::TabId;
    use arclain_ui::shared::components::search_palette::{self, SearchHit, TabSummary};

    // A query that hits both a tab (by code/title) and a file path
    // exercises group labels, highlight slicing, and both row layouts —
    // none of which the empty-hits header tests reach.
    let tabs = vec![TabSummary {
        id: TabId(1),
        code: "RJ000222".into(),
        title: "Scene Pack".into(),
        maker: "Coralt".into(),
        file: "scene.rar".into(),
        entry_count: 3,
        active: true,
    }];
    let paths = ["scene_01.txt", "img_main.jpg"];
    let hits: Vec<SearchHit> = search_palette::build_hits("sc", &tabs, &paths);
    assert!(hits.len() >= 2, "query should match the tab and a file");

    let mut harness = Harness::new_ui_state(
        move |ui, selected: &mut usize| {
            let theme = arclain_theme::AppTheme::new(true);
            let anchor = ui.max_rect();
            let _ = search_palette::view::render_area(
                ui, &theme, anchor, "sc", &hits, "RJ000222", selected,
            );
        },
        0usize,
    );
    harness.run();
}

// =============================================================================
// Carousel (empty)
// =============================================================================

#[test]
fn carousel_renders_empty_images() {
    use arclain_ui::shared::components::carousel::Carousel;

    let images: Vec<(String, Option<String>)> = vec![];

    let mut harness = Harness::new_ui(move |ui| {
        Carousel::new("test_carousel", &images, 0).show(ui);
    });

    harness.run();
}
