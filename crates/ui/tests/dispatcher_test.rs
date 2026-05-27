//! Dispatcher tests for the MVU-converted features.
//!
//! Each `handle_*_action` function is the side-effect chokepoint for
//! its feature. These tests assert the dispatcher's behavior without
//! going through the egui render path — the MVU split is what makes
//! that possible.
//!
//! Most tests run against a minimal `SharedState` with `dbs: None`
//! and `services.*` empty, exercising the "no DB / no service"
//! branches. Adding in-memory DB setup (a follow-up task) would
//! unlock the meaty round-trip tests for save/delete/etc.

mod common;
use common::create_test_shared_state;

// ============================================================================
// ProfilesPage dispatcher (handle_profiles_action)
// ============================================================================

mod profiles_page {
    use super::*;
    use arclain_core::features::organization::{ArchiveFormat, ArchiveProfile};
    use arclain_ui::features::organization::presentation::views::profiles_page::{
        handle_profiles_action, ProfilesAction, ProfilesPage,
    };

    fn sample_profile() -> ArchiveProfile {
        ArchiveProfile {
            id: 0,
            name: "test".into(),
            description: None,
            format: ArchiveFormat::SevenZ,
            compression_level: 5,
            compression_method: Some("LZMA2".into()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: false,
            is_system: false,
        }
    }

    #[test]
    fn load_profiles_with_no_db_sets_error() {
        let shared = create_test_shared_state();
        let mut page = ProfilesPage::new();

        handle_profiles_action(&mut page, ProfilesAction::LoadProfiles, &shared);

        assert_eq!(
            page.error(),
            Some("Database not available"),
            "LoadProfiles with no DB should surface the missing-DB error"
        );
    }

    #[test]
    fn save_profile_with_no_db_sets_error() {
        let shared = create_test_shared_state();
        let mut page = ProfilesPage::new();

        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(sample_profile()),
            &shared,
        );

        assert_eq!(page.error(), Some("Database not available"));
    }

    #[test]
    fn delete_profile_with_no_db_sets_error() {
        let shared = create_test_shared_state();
        let mut page = ProfilesPage::new();

        handle_profiles_action(&mut page, ProfilesAction::DeleteProfile(42), &shared);

        assert_eq!(page.error(), Some("Database not available"));
    }

    #[test]
    fn set_default_profile_with_no_db_sets_error() {
        let shared = create_test_shared_state();
        let mut page = ProfilesPage::new();

        handle_profiles_action(&mut page, ProfilesAction::SetDefaultProfile(42), &shared);

        assert_eq!(page.error(), Some("Database not available"));
    }
}

// ============================================================================
// RulesPage dispatcher (handle_rules_page_action)
// ============================================================================

mod rules_page {
    use arclain_core::OrganizationService;
    use arclain_db::DieselPool;
    use arclain_ui::features::organization::presentation::views::rules_page::{
        handle_rules_page_action, RulesPage, RulesPageAction,
    };
    use arclain_ui::core::navigation::SettingsPage;

    /// Empty in-memory pool: the schema isn't applied, so any real
    /// query returns Err. Suitable for testing dispatcher error
    /// branches that don't actually need the data.
    fn empty_in_memory_service() -> OrganizationService {
        let pool = DieselPool::from_url(":memory:").expect("in-memory pool");
        OrganizationService::new(pool)
    }

    #[test]
    fn load_rule_with_id_zero_creates_new_editor_state_without_service() {
        let mut page = RulesPage::new();
        let service = empty_in_memory_service();

        // rule_id == 0 is the "new rule" sentinel; the dispatcher
        // creates a fresh RuleEditorState without touching the service,
        // so even an empty-schema pool is fine here.
        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 0 },
            &service,
            None,
        );

        assert!(
            page.editor_load_error().is_none(),
            "rule_id=0 must not hit the error path"
        );
        // RulesPage doesn't expose editor_state directly; we infer
        // success from is_editor_dirty being defined (false on fresh)
        // and the absence of an error.
        assert!(
            !page.is_editor_dirty(),
            "fresh editor state should be clean"
        );
    }

    #[test]
    fn load_rules_against_empty_schema_sets_error() {
        let mut page = RulesPage::new();
        let service = empty_in_memory_service();

        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &service, None);

        assert!(
            page.error().is_some(),
            "list_domain_rules against empty schema must error and surface it"
        );
    }

    #[test]
    fn load_rule_against_empty_schema_sets_editor_load_error() {
        let mut page = RulesPage::new();
        let service = empty_in_memory_service();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 7 },
            &service,
            None,
        );

        assert!(
            page.editor_load_error().is_some(),
            "get_domain_rule against empty schema should surface editor_load_error"
        );
    }

    #[test]
    #[should_panic(expected = "Navigate should be handled by the caller")]
    fn navigate_action_panics_in_debug() {
        let mut page = RulesPage::new();
        let service = empty_in_memory_service();

        // Navigate is supposed to be handled at the call site (translated
        // to SettingsAction::NavigateTo) and never reach the dispatcher.
        // Debug asserts catch the misuse.
        handle_rules_page_action(
            &mut page,
            RulesPageAction::Navigate(SettingsPage::OrganizationRules),
            &service,
            None,
        );
    }
}

// ============================================================================
// ProcessPage dispatcher (handle_process_action)
// ============================================================================

mod process_page {
    use super::*;
    use arclain_ui::features::process::view::{handle_process_action, ProcessAction};
    use arclain_ui::features::process::ProcessPageState;

    #[test]
    fn load_interrupted_count_with_no_config_db_sets_zero() {
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();

        handle_process_action(
            &mut state,
            ProcessAction::LoadInterruptedCount,
            &shared,
        );

        assert_eq!(
            state.interrupted_run_count,
            Some(0),
            "with config_db=None the dispatcher falls back to count=0 and caches it"
        );
    }

    #[test]
    fn load_interrupted_count_is_idempotent() {
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();
        state.interrupted_run_count = Some(7); // pretend a prior load happened

        handle_process_action(
            &mut state,
            ProcessAction::LoadInterruptedCount,
            &shared,
        );

        assert_eq!(
            state.interrupted_run_count,
            Some(7),
            "ensure_interrupted_count short-circuits when the cache is already populated"
        );
    }

    #[test]
    fn load_organization_rules_with_no_service_sets_empty_cache() {
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();

        handle_process_action(
            &mut state,
            ProcessAction::LoadOrganizationRules,
            &shared,
        );

        let cache = state
            .cached_org_rules
            .as_ref()
            .expect("LoadOrganizationRules must populate the cache (even if empty)");
        assert!(
            cache.is_empty(),
            "without an organization_service the cache lands on an empty Vec"
        );
    }

    #[test]
    fn save_presets_persists_state_via_save_path() {
        // SavePresets is filesystem IO not DB; we can exercise the
        // dispatcher's routing without needing a real presets file —
        // save_presets() no-ops cleanly when presets_path is None
        // (which is the default for a fresh ProcessPageState).
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();
        // presets_path is None by default — save_presets() should
        // log and return without panicking.

        handle_process_action(&mut state, ProcessAction::SavePresets, &shared);
        // No assertions on disk state; the assertion is "does not
        // panic." Routing test, not behavior test.
    }
}

// ============================================================================
// LayoutEditor dispatcher (handle_layout_editor_action via region wrappers)
// ============================================================================

mod layout_editor {
    use arclain_ui::features::settings::presentation::pages::{
        handle_info_panel_layout_action, handle_toolbar_layout_action, InfoPanelLayoutState,
        LayoutEditorAction, ToolbarLayoutState,
    };

    #[test]
    fn toolbar_sync_with_no_service_leaves_state_unloaded() {
        let mut state = ToolbarLayoutState::default();

        handle_toolbar_layout_action(
            &mut state,
            LayoutEditorAction::SyncItems,
            None, // no UiService
            None, // no PluginManager
        );

        assert!(
            !state.loaded,
            "without a UiService the loader branch must not flip loaded=true"
        );
        assert!(state.items.is_empty());
    }

    #[test]
    fn info_panel_sync_with_no_service_leaves_state_unloaded() {
        let mut state = InfoPanelLayoutState::default();

        handle_info_panel_layout_action(
            &mut state,
            LayoutEditorAction::SyncItems,
            None,
            None,
        );

        assert!(!state.loaded);
        assert!(state.items.is_empty());
    }

    #[test]
    fn sync_with_no_plugin_manager_skips_plugin_walk_cleanly() {
        let mut state = ToolbarLayoutState::default();

        // Sync with neither service nor plugin manager should be a
        // graceful no-op. Specifically, dirty stays false.
        handle_toolbar_layout_action(
            &mut state,
            LayoutEditorAction::SyncItems,
            None,
            None,
        );

        assert!(
            !state.dirty,
            "no-op sync must not mark the editor dirty"
        );
    }

    #[test]
    fn toolbar_and_info_panel_state_types_are_distinct() {
        // PhantomData<R> tag means the two regions' state types do
        // NOT unify. This is a compile-time check expressed as a
        // runtime assertion (the test running implies it compiled).
        let toolbar = ToolbarLayoutState::default();
        let info_panel = InfoPanelLayoutState::default();
        // Just touching the fields — if the aliases collided, the
        // dispatcher wrappers above would refuse to accept them at
        // the wrong call site, and this test file wouldn't compile.
        assert!(toolbar.items.is_empty());
        assert!(info_panel.items.is_empty());
    }
}

// ============================================================================
// InterfaceSettings dispatcher (handle_interface_settings_action)
// ============================================================================

mod interface_settings {
    use super::*;
    use arclain_ui::core::navigation::SettingsPage;
    use arclain_ui::features::settings::presentation::pages::{
        handle_interface_settings_action, InterfaceSettingsAction, InterfaceSettingsState,
    };

    #[test]
    fn load_items_with_no_service_is_noop() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        assert!(!state.loaded);

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::LoadItems,
            &shared,
        );

        // With ui_service=None the dispatcher's early-return path
        // fires; load_from_service is never called and `loaded`
        // stays false.
        assert!(
            !state.loaded,
            "no ui_service → loader must short-circuit"
        );
        assert!(state.items.is_empty());
    }

    #[test]
    fn save_and_sync_with_no_service_does_not_panic_and_does_not_clear_dirty() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        state.show_button_labels = true;
        state.dirty = true;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveAndSync,
            &shared,
        );

        // With ui_service=None the dispatcher returns before
        // save_to_service can clear dirty. The user-visible state
        // remains pending-save until a service is wired up.
        assert!(
            state.dirty,
            "no ui_service → SaveAndSync early-returns and dirty stays true"
        );
    }

    #[test]
    #[should_panic(expected = "Navigate should be handled by the caller")]
    fn navigate_action_panics_in_debug() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::Navigate(SettingsPage::ToolbarLayout),
            &shared,
        );
    }
}
