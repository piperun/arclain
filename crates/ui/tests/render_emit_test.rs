//! Render-emit tests for the MVU-converted features.
//!
//! Complements `dispatcher_test.rs`: dispatcher tests verify
//! "given action X, the right side effects happen"; these tests
//! verify "given state Y, render emits the right action." Together
//! they pin both halves of the MVU loop.
//!
//! Uses `egui_kittest::Harness::new_ui_state` so each render runs
//! against a real (headless) egui Ui — same as the existing widget
//! tests in `widget_tests.rs` and `close_tab_confirm_test.rs`. The
//! pattern: render emits the action, the closure latches it on the
//! first non-None frame, the test asserts on the latched value.
//!
//! Skipped here (ctx-based page renderers using egui::SidePanel +
//! CentralPanel — `ProcessPage::render`): those need
//! `Harness::new` not `new_ui_state`, plus a more involved
//! `SharedState` setup. Worth doing in a follow-up.

mod common;

use arclain_ui::shared::theme::AppTheme;
use egui_kittest::Harness;

// ============================================================================
// ProfilesPage
// ============================================================================

mod profiles_page {
    use super::*;
    use arclain_ui::features::organization::presentation::views::profiles_page::{
        handle_profiles_action, ProfilesAction, ProfilesPage,
    };
    use arclain_ui::shared::SharedState;
    use egui_kittest::kittest::Queryable as _;

    struct Stage {
        page: ProfilesPage,
        theme: AppTheme,
        last_action: Option<ProfilesAction>,
    }

    #[test]
    fn first_render_with_empty_cache_emits_load_profiles() {
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut Stage| {
                if let Some(a) = s.page.render(ui, &s.theme) {
                    s.last_action = Some(a);
                }
            },
            Stage {
                page: ProfilesPage::new(),
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            matches!(
                harness.state().last_action,
                Some(ProfilesAction::LoadProfiles)
            ),
            "first render with profiles=None must auto-emit LoadProfiles"
        );
    }

    struct StormStage {
        page: ProfilesPage,
        shared: SharedState,
        theme: AppTheme,
        load_count: usize,
    }

    /// A `LoadProfiles` dispatch that fails (here: no facade) leaves the
    /// cache empty. The auto-fire must not depend on the dispatcher
    /// succeeding in order to quench: it fires once, holds the failure,
    /// and only the Retry affordance arms exactly one more attempt.
    /// Without that, a failed load re-fires a blocking database call
    /// every frame, forever.
    #[test]
    fn a_failed_load_fires_once_and_retry_arms_exactly_one_more() {
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut StormStage| {
                if let Some(action) = s.page.render(ui, &s.theme) {
                    if matches!(action, ProfilesAction::LoadProfiles) {
                        s.load_count += 1;
                    }
                    // Dispatch synchronously after render, exactly as
                    // `settings_content.rs` does. No facade, so the
                    // load fails and the cache stays empty.
                    handle_profiles_action(&mut s.page, action, &s.shared);
                }
            },
            StormStage {
                page: ProfilesPage::new(),
                shared: common::create_test_shared_state(),
                theme: AppTheme::new(false),
                load_count: 0,
            },
        );

        for _ in 0..4 {
            harness.step();
        }
        assert_eq!(
            harness.state().load_count,
            1,
            "a failed load must fire exactly once, not once per frame"
        );
        assert!(
            harness.state().page.error().is_some(),
            "the failure must be surfaced on the page"
        );

        // Retry is a user action: it arms exactly one further attempt,
        // which also fails, and the page holds again.
        harness.get_by_label("Retry").click();
        for _ in 0..4 {
            harness.step();
        }
        assert_eq!(
            harness.state().load_count,
            2,
            "Retry must arm exactly one more load"
        );
    }
}

// ============================================================================
// RulesPage
// ============================================================================

mod rules_page {
    use super::*;
    use arclain_ui::features::organization::presentation::views::rules_page::{
        handle_rules_page_action, RulesPage, RulesPageAction,
    };
    use arclain_ui::shared::SharedState;
    use egui_kittest::kittest::Queryable as _;

    struct ListStage {
        page: RulesPage,
        theme: AppTheme,
        last_action: Option<RulesPageAction>,
    }

    #[test]
    fn first_render_with_empty_cache_emits_load_rules() {
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut ListStage| {
                if let Some(a) = s.page.render(ui, &s.theme) {
                    s.last_action = Some(a);
                }
            },
            ListStage {
                page: RulesPage::new(),
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            matches!(
                harness.state().last_action,
                Some(RulesPageAction::LoadRules)
            ),
            "first render with rules=None must auto-emit LoadRules"
        );
    }

    struct EditStage {
        page: RulesPage,
        theme: AppTheme,
        rule_id: i64,
        latched_data: Option<RulesPageAction>,
    }

    #[test]
    fn first_render_edit_rule_emits_load_rule() {
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut EditStage| {
                let out = s.page.render_edit_rule(ui, &s.theme, s.rule_id);
                if let Some(a) = out.data_action {
                    s.latched_data = Some(a);
                }
            },
            EditStage {
                page: RulesPage::new(),
                theme: AppTheme::new(false),
                rule_id: 42,
                latched_data: None,
            },
        );
        harness.run();
        assert!(
            matches!(
                harness.state().latched_data,
                Some(RulesPageAction::LoadRule { rule_id: 42 })
            ),
            "render_edit_rule with no editor state must auto-emit LoadRule"
        );
    }

    struct StormStage {
        page: RulesPage,
        shared: SharedState,
        theme: AppTheme,
        load_count: usize,
    }

    /// A `LoadRules` dispatch that fails (here: no facade) leaves the
    /// cache empty. The auto-fire must not depend on the dispatcher
    /// succeeding in order to quench: it fires once, holds the failure,
    /// and only the Retry affordance arms exactly one more attempt.
    /// Without that, a failed load re-fires a blocking database call
    /// every frame, forever.
    #[test]
    fn a_failed_load_fires_once_and_retry_arms_exactly_one_more() {
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut StormStage| {
                if let Some(action) = s.page.render(ui, &s.theme) {
                    if matches!(action, RulesPageAction::LoadRules) {
                        s.load_count += 1;
                    }
                    // Dispatch synchronously after render, exactly as
                    // `settings_content.rs` does. No facade, so the
                    // load fails and the cache stays empty.
                    handle_rules_page_action(&mut s.page, action, &s.shared, None);
                }
            },
            StormStage {
                page: RulesPage::new(),
                shared: common::create_test_shared_state(),
                theme: AppTheme::new(false),
                load_count: 0,
            },
        );

        for _ in 0..4 {
            harness.step();
        }
        assert_eq!(
            harness.state().load_count,
            1,
            "a failed load must fire exactly once, not once per frame"
        );
        assert!(
            harness.state().page.error().is_some(),
            "the failure must be surfaced on the page"
        );

        // Retry is a user action: it arms exactly one further attempt,
        // which also fails, and the page holds again.
        harness.get_by_label("Retry").click();
        for _ in 0..4 {
            harness.step();
        }
        assert_eq!(
            harness.state().load_count,
            2,
            "Retry must arm exactly one more load"
        );

        // A cache invalidation (the user saved or cancelled an edit) is
        // also a user action, so it arms the reload the same way.
        harness.state_mut().page.mark_saved_and_clear();
        for _ in 0..4 {
            harness.step();
        }
        assert_eq!(
            harness.state().load_count,
            3,
            "invalidating the cache must arm exactly one reload"
        );
    }
}

// ============================================================================
// InterfaceSettings
// ============================================================================

mod interface_settings {
    use super::*;
    use arclain_ui::features::settings::presentation::pages::{
        render_interface_settings, InterfaceSettingsAction, InterfaceSettingsState,
    };
    use arclain_ui::shared::SharedState;

    struct Stage {
        shared: SharedState,
        state: InterfaceSettingsState,
        theme: AppTheme,
        last_action: Option<InterfaceSettingsAction>,
    }

    #[test]
    fn first_render_with_unloaded_state_emits_load_display_options() {
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut Stage| {
                if let Some(a) = render_interface_settings(ui, &s.theme, &s.shared, &mut s.state) {
                    s.last_action = Some(a);
                }
            },
            Stage {
                shared: common::create_test_shared_state(),
                state: InterfaceSettingsState::default(),
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            matches!(
                harness.state().last_action,
                Some(InterfaceSettingsAction::LoadDisplayOptions)
            ),
            "first render with loaded=false must auto-emit LoadDisplayOptions"
        );
    }

    #[test]
    fn render_with_dirty_state_emits_save_display_options() {
        // Pretend the page already loaded (so the LoadDisplayOptions
        // auto-fire path is skipped) and a display-option has been
        // toggled (dirty=true). The render path's bottom-of-frame
        // auto-save check should emit SaveDisplayOptions.
        let mut s = InterfaceSettingsState::default();
        s.loaded = true;
        s.dirty = true;

        let mut harness = Harness::new_ui_state(
            |ui, st: &mut Stage| {
                if let Some(a) = render_interface_settings(ui, &st.theme, &st.shared, &mut st.state)
                {
                    st.last_action = Some(a);
                }
            },
            Stage {
                shared: common::create_test_shared_state(),
                state: s,
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            matches!(
                harness.state().last_action,
                Some(InterfaceSettingsAction::SaveDisplayOptions)
            ),
            "dirty state with no higher-priority action must auto-emit SaveDisplayOptions"
        );
    }

    /// Both intents auto-fire every frame, so a failure that leaves
    /// `loaded` false or `dirty` true would retry sixty times a second
    /// and report itself each time. Holding the failure is what stops
    /// that, and it is the whole reason `error` is on the page state.
    #[test]
    fn a_held_failure_stops_the_load_from_re_firing_every_frame() {
        let mut state = InterfaceSettingsState::default();
        state.error = Some("Failed to load interface settings: no backend".to_string());

        let mut harness = Harness::new_ui_state(
            |ui, s: &mut Stage| {
                if let Some(a) = render_interface_settings(ui, &s.theme, &s.shared, &mut s.state) {
                    s.last_action = Some(a);
                }
            },
            Stage {
                shared: common::create_test_shared_state(),
                state,
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            harness.state().last_action.is_none(),
            "an unloaded page holding a failure must not keep asking"
        );
    }

    #[test]
    fn a_held_failure_stops_the_save_from_re_firing_every_frame() {
        let mut state = InterfaceSettingsState::default();
        state.loaded = true;
        state.dirty = true;
        state.error = Some("Failed to save interface settings: no backend".to_string());

        let mut harness = Harness::new_ui_state(
            |ui, s: &mut Stage| {
                if let Some(a) = render_interface_settings(ui, &s.theme, &s.shared, &mut s.state) {
                    s.last_action = Some(a);
                }
            },
            Stage {
                shared: common::create_test_shared_state(),
                state,
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            harness.state().last_action.is_none(),
            "a dirty page holding a failure must wait for the next edit, not retry every frame"
        );
        assert!(
            harness.state().state.dirty,
            "and must still be dirty -- the edit was never stored"
        );
    }
}

// ============================================================================
// LayoutEditor (toolbar variant — InfoPanel is structurally identical)
// ============================================================================

mod layout_editor {
    use super::*;
    use arclain_ui::features::settings::presentation::pages::{
        render_toolbar_layout, LayoutEditorAction, ToolbarLayoutState,
    };

    struct Stage {
        state: ToolbarLayoutState,
        theme: AppTheme,
        last_action: Option<LayoutEditorAction>,
    }

    #[test]
    fn render_always_emits_sync_items() {
        // SyncItems is an auto-fire-every-frame action — it covers both
        // initial load and per-frame plugin reconciliation. The render
        // path returns it unconditionally so the parent dispatcher gets
        // a chance to refresh state.items from the canonical signal.
        let mut harness = Harness::new_ui_state(
            |ui, s: &mut Stage| {
                if let Some(a) = render_toolbar_layout(ui, &s.theme, &mut s.state) {
                    s.last_action = Some(a);
                }
            },
            Stage {
                state: ToolbarLayoutState::default(),
                theme: AppTheme::new(false),
                last_action: None,
            },
        );
        harness.run();
        assert!(
            matches!(
                harness.state().last_action,
                Some(LayoutEditorAction::SyncItems)
            ),
            "render must auto-emit SyncItems every frame"
        );
    }
}
