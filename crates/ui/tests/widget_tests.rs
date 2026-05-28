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
            let inputs = HeaderInputs {
                show_nav_buttons: true,
                can_go_back: true,
                is_on_settings: false,
                server_status: &arclain_ui::core::signals::ServerConnectionStatus::Offline,
                search_hits: &[],
                active_code: "",
            };
            render(ui, &theme, state, &mut toggle, &mut focus, &inputs);
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
            let inputs = HeaderInputs {
                show_nav_buttons: false,
                can_go_back: false,
                is_on_settings: false,
                server_status: &arclain_ui::core::signals::ServerConnectionStatus::Offline,
                search_hits: &[],
                active_code: "",
            };
            render(ui, &theme, state, &mut toggle, &mut focus, &inputs);
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
            let inputs = HeaderInputs {
                show_nav_buttons: true,
                can_go_back: true,
                is_on_settings: true,
                server_status: &arclain_ui::core::signals::ServerConnectionStatus::Offline,
                search_hits: &[],
                active_code: "",
            };
            render(ui, &theme, state, &mut toggle, &mut focus, &inputs);
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
            let view = search_palette::PaletteView {
                anchor_rect: anchor,
                query: "sc",
                hits: &hits,
                active_code: "RJ000222",
                scroll_to_selected: false,
            };
            let _ = search_palette::view::render_area(ui, &theme, &view, selected);
        },
        0usize,
    );
    harness.run();
}

#[test]
fn handle_keys_arrows_move_selection_and_flag_navigation() {
    // Regression: arrowing the palette must actually move the selection AND
    // flag `navigated` (so the dropdown scrolls the new row into view).
    // The headless render test above never pressed a key, so it couldn't
    // catch nav being broken — this drives real ArrowUp/Down events.
    use arclain_ui::shared::components::search_palette::view::handle_keys;

    // state.0 = selected index, state.1 = "navigated fired at least once".
    // key_press emits down+up, so step() runs two frames; navigated is only
    // true on the down frame — accumulate it instead of reading last frame.
    let mut harness = Harness::new_ui_state(
        |ui, st: &mut (usize, bool)| {
            let intent = handle_keys(ui, 3, &mut st.0);
            st.1 |= intent.navigated;
        },
        (0usize, false),
    );

    harness.step();
    assert_eq!(harness.state().0, 0, "starts at the top");

    harness.key_press(egui::Key::ArrowDown);
    harness.step();
    assert_eq!(harness.state().0, 1, "ArrowDown advances");
    assert!(harness.state().1, "ArrowDown flags navigated for scroll-follow");

    harness.key_press(egui::Key::ArrowDown);
    harness.step();
    assert_eq!(harness.state().0, 2, "ArrowDown advances again");

    harness.key_press(egui::Key::ArrowDown);
    harness.step();
    assert_eq!(harness.state().0, 0, "ArrowDown wraps past the end");

    harness.key_press(egui::Key::ArrowUp);
    harness.step();
    assert_eq!(harness.state().0, 2, "ArrowUp wraps backwards");
}

// =============================================================================
// Hotkey focus arbitration (end-to-end through a live egui Context)
// =============================================================================

#[test]
fn check_input_suppresses_contextual_hotkey_while_text_field_focused() {
    // Drive the real arbitration: a focused TextEdit must keep Ctrl+A for
    // "select all text", so the SelectAll app hotkey ("select all files")
    // must NOT fire. Pairs with the control test below.
    use arclain_ui::features::hotkeys::{HotkeyAction, HotkeyManager};

    let manager = HotkeyManager::new();
    // state: (text buffer, accumulated triggered actions, frame counter)
    let mut harness = Harness::new_ui_state(
        move |ui, st: &mut (String, Vec<HotkeyAction>, u32)| {
            st.2 += 1;
            // check_input runs at frame-top in the app (before widgets), so
            // call it before the TextEdit: it reads last frame's focus and
            // this frame's still-unconsumed key events.
            st.1.extend(manager.check_input(ui.ctx()));
            let resp = ui.text_edit_singleline(&mut st.0);
            if st.2 == 1 {
                resp.request_focus();
            }
        },
        (String::new(), Vec::new(), 0u32),
    );

    harness.step(); // frame 1: focus requested
    harness.step(); // frame 2: focus now active
    harness.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::A);
    harness.step(); // frame 3: Ctrl+A arrives while focused

    assert!(
        !harness.state().1.contains(&HotkeyAction::SelectAll),
        "SelectAll must not fire while a text field is focused; got {:?}",
        harness.state().1
    );
}

#[test]
fn check_input_fires_contextual_hotkey_when_nothing_focused() {
    // Control for the test above: the same Ctrl+A, with no widget holding
    // focus, DOES fire SelectAll — proving the suppression there is the focus
    // guard, not a missing binding.
    use arclain_ui::features::hotkeys::{HotkeyAction, HotkeyManager};

    let manager = HotkeyManager::new();
    let mut harness = Harness::new_ui_state(
        move |ui, st: &mut Vec<HotkeyAction>| {
            st.extend(manager.check_input(ui.ctx()));
            ui.label("no focusable widget here"); // takes no keyboard focus
        },
        Vec::new(),
    );

    harness.step();
    harness.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::A);
    harness.step();

    assert!(
        harness.state().contains(&HotkeyAction::SelectAll),
        "SelectAll should fire when no widget owns the keyboard; got {:?}",
        harness.state()
    );
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
