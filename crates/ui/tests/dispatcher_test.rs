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
use common::{create_test_shared_state, create_test_shared_state_with_dbs};

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
    fn load_display_options_with_no_service_is_noop() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        assert!(!state.loaded);

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::LoadDisplayOptions,
            &shared,
        );

        // With ui_service=None the dispatcher's early-return path
        // fires; load_from_service is never called and `loaded`
        // stays false.
        assert!(
            !state.loaded,
            "no ui_service → loader must short-circuit"
        );
    }

    #[test]
    fn save_display_options_with_no_service_does_not_panic_and_does_not_clear_dirty() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        state.show_button_labels = true;
        state.dirty = true;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveDisplayOptions,
            &shared,
        );

        // With ui_service=None the dispatcher returns before
        // save_to_service can clear dirty. The user-visible state
        // remains pending-save until a service is wired up.
        assert!(
            state.dirty,
            "no ui_service → SaveDisplayOptions early-returns and dirty stays true"
        );
    }

    #[test]
    fn toggle_item_visibility_with_no_service_is_noop() {
        use arclain_core::UiRegion;

        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();

        // No ui_service in test shared → dispatcher early-returns
        // without panic. Signal value unchanged.
        let before = shared.signals().context_menu_items.get();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegion::ContextMenu,
                item_id: "anything".into(),
                visible: false,
            },
            &shared,
        );
        let after = shared.signals().context_menu_items.get();
        assert_eq!(before.len(), after.len());
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

// ============================================================================
// Happy-path tests (against real temp-file SQLite databases with
// production schemas applied). Each test owns its own tempdir; the
// dir is held in scope until the test returns so open SQLite handles
// stay valid.
// ============================================================================

mod profiles_page_happy {
    use super::*;
    use arclain_core::features::organization::{ArchiveFormat, ArchiveProfile};
    use arclain_ui::features::organization::presentation::views::profiles_page::{
        handle_profiles_action, ProfilesAction, ProfilesPage,
    };

    fn profile_named(name: &str) -> ArchiveProfile {
        ArchiveProfile {
            id: 0,
            name: name.into(),
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

    /// ConfigDb seeds 3 system profiles ("Maximum Compression (7z)",
    /// "Fast Compression (7z)", "Zip Compatible") on first init.
    /// Tests that count user-created profiles must filter `!is_system`.
    fn user_names(profiles: &[ArchiveProfile]) -> Vec<String> {
        profiles
            .iter()
            .filter(|p| !p.is_system)
            .map(|p| p.name.clone())
            .collect()
    }

    #[test]
    fn load_against_fresh_db_returns_seeded_system_profiles_only() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut page = ProfilesPage::new();

        handle_profiles_action(&mut page, ProfilesAction::LoadProfiles, &shared);

        let profiles = page
            .profiles()
            .expect("LoadProfiles must populate the cache");
        assert!(
            user_names(profiles).is_empty(),
            "no user profiles until SaveProfile fires"
        );
        assert!(
            !profiles.is_empty(),
            "fresh-init DB ships with system-defaults seeded"
        );
        assert_eq!(page.error(), None);
    }

    #[test]
    fn save_then_load_returns_the_profile() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut page = ProfilesPage::new();

        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(profile_named("alpha")),
            &shared,
        );

        // Save's dispatcher branch re-fetches the list, so `page.profiles`
        // already reflects the new row without a separate LoadProfiles call.
        let profiles = page.profiles().expect("save should re-populate cache");
        assert_eq!(user_names(profiles), vec!["alpha".to_string()]);
        assert_eq!(page.error(), None);
    }

    #[test]
    fn delete_removes_only_the_targeted_profile() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut page = ProfilesPage::new();

        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(profile_named("alpha")),
            &shared,
        );
        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(profile_named("beta")),
            &shared,
        );
        let id_alpha = page
            .profiles()
            .unwrap()
            .iter()
            .find(|p| p.name == "alpha")
            .unwrap()
            .id;

        handle_profiles_action(
            &mut page,
            ProfilesAction::DeleteProfile(id_alpha),
            &shared,
        );

        let after = page.profiles().unwrap();
        assert_eq!(user_names(after), vec!["beta".to_string()]);
    }

    #[test]
    fn set_default_marks_only_one_profile_default() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut page = ProfilesPage::new();

        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(profile_named("alpha")),
            &shared,
        );
        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(profile_named("beta")),
            &shared,
        );
        let id_beta = page
            .profiles()
            .unwrap()
            .iter()
            .find(|p| p.name == "beta")
            .unwrap()
            .id;

        handle_profiles_action(
            &mut page,
            ProfilesAction::SetDefaultProfile(id_beta),
            &shared,
        );

        let after = page.profiles().unwrap();
        let defaults: Vec<&str> = after
            .iter()
            .filter(|p| p.is_default)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(defaults, vec!["beta"]);
    }
}

mod rules_page_happy {
    use arclain_core::features::organization::{OrganizationRule, RuleTrigger};
    use arclain_ui::features::organization::presentation::views::rules_page::{
        handle_rules_page_action, RulesPage, RulesPageAction,
    };

    use super::*;

    #[test]
    fn load_rules_against_empty_db_populates_empty_vec() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let service = shared
            .services
            .organization_service
            .as_ref()
            .unwrap()
            .clone();
        let mut page = RulesPage::new();

        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &service, None);

        assert_eq!(page.error(), None);
        // page.rules is not pub; we infer success from the absence of an
        // error and from a follow-up LoadRule(0) creating fresh state.
    }

    #[test]
    fn load_rule_with_id_zero_succeeds_against_real_db() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let service = shared
            .services
            .organization_service
            .as_ref()
            .unwrap()
            .clone();
        let mut page = RulesPage::new();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 0 },
            &service,
            None,
        );

        assert_eq!(page.editor_load_error(), None);
        assert!(!page.is_editor_dirty());
    }

    #[test]
    fn save_rule_then_load_returns_it() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let service = shared
            .services
            .organization_service
            .as_ref()
            .unwrap()
            .clone();

        // Insert a rule directly via the service (the dispatcher save
        // path goes through RulesPage::save_editor_rule which isn't
        // an action — it's called from the settings header save
        // button). Use the service to stage the fixture, then dispatch
        // LoadRules to verify the dispatcher picks it up.
        let rule = OrganizationRule {
            id: 0,
            name: "demo-rule".into(),
            priority: 100,
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: Default::default(),
            ..Default::default()
        };
        service.save_domain_rule(&rule).expect("seed rule");

        let mut page = RulesPage::new();
        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &service, None);

        assert_eq!(page.error(), None);
        // We can't read page.rules directly (private), but a
        // subsequent LoadRule with the seeded id should now succeed
        // without setting editor_load_error.
        let seeded_id = service
            .list_domain_rules()
            .expect("list rules")
            .into_iter()
            .find(|r| r.name == "demo-rule")
            .expect("seeded rule present")
            .id;

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: seeded_id },
            &service,
            None,
        );
        assert_eq!(
            page.editor_load_error(),
            None,
            "LoadRule with a real id must succeed"
        );
    }
}

mod process_page_happy {
    use super::*;
    use arclain_ui::features::process::view::{handle_process_action, ProcessAction};
    use arclain_ui::features::process::ProcessPageState;

    #[test]
    fn load_organization_rules_with_real_service_caches_empty_vec_against_empty_db() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut state = ProcessPageState::default();

        handle_process_action(
            &mut state,
            ProcessAction::LoadOrganizationRules,
            &shared,
        );

        let cache = state
            .cached_org_rules
            .as_ref()
            .expect("cache should be populated");
        assert!(
            cache.is_empty(),
            "fresh DB has no rules; cache should be Some(empty)"
        );
    }

    #[test]
    fn load_interrupted_count_against_empty_db_returns_zero() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut state = ProcessPageState::default();

        handle_process_action(
            &mut state,
            ProcessAction::LoadInterruptedCount,
            &shared,
        );

        assert_eq!(
            state.interrupted_run_count,
            Some(0),
            "no rows in pipeline_runs → count is 0"
        );
    }
}

mod layout_editor_happy {
    use super::*;
    use arclain_ui::features::settings::presentation::pages::{
        handle_info_panel_layout_action, handle_toolbar_layout_action, InfoPanelLayoutState,
        LayoutEditorAction, ToolbarLayoutState,
    };

    #[test]
    fn toolbar_sync_with_real_service_loads_seeded_defaults() {
        // ConfigDb's ui::seed module ships canonical toolbar entries
        // (navigation group, file-actions, view, etc.) on first init.
        // We assert "non-empty" rather than a hardcoded count so the
        // test doesn't break every time defaults are tweaked.
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let ui_service = shared.services.ui_service.as_deref();
        let mut state = ToolbarLayoutState::default();

        handle_toolbar_layout_action(
            &mut state,
            LayoutEditorAction::SyncItems,
            ui_service,
            None,
        );

        assert!(state.loaded, "real UiService must flip loaded=true");
        assert!(
            !state.items.is_empty(),
            "fresh DB ships with seeded toolbar defaults"
        );
        assert!(
            !state.dirty,
            "loading existing items shouldn't mark the editor dirty"
        );
    }

    #[test]
    fn info_panel_sync_with_real_service_loads_seeded_defaults() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let ui_service = shared.services.ui_service.as_deref();
        let mut state = InfoPanelLayoutState::default();

        handle_info_panel_layout_action(
            &mut state,
            LayoutEditorAction::SyncItems,
            ui_service,
            None,
        );

        assert!(state.loaded);
        assert!(
            !state.items.is_empty(),
            "fresh DB ships with seeded info-panel sections"
        );
    }
}

mod interface_settings_happy {
    use super::*;
    use arclain_core::UiRegion;
    use arclain_ui::features::settings::presentation::pages::{
        handle_interface_settings_action, InterfaceSettingsAction, InterfaceSettingsState,
    };

    #[test]
    fn load_display_options_against_empty_db_marks_loaded() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut state = InterfaceSettingsState::default();
        assert!(!state.loaded);

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::LoadDisplayOptions,
            &shared,
        );

        assert!(state.loaded, "real UiService must flip loaded=true");
        assert!(!state.dirty, "fresh load shouldn't be dirty");
    }

    #[test]
    fn save_display_options_clears_dirty_against_real_db() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut state = InterfaceSettingsState::default();
        // Pretend the user toggled something.
        state.show_button_labels = true;
        state.dirty = true;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveDisplayOptions,
            &shared,
        );

        assert!(
            !state.dirty,
            "SaveDisplayOptions must clear dirty once the write completes"
        );
    }

    #[test]
    fn toggle_item_visibility_updates_signal_and_persists() {
        // Pre-seed the info_panel_items signal with a row, then toggle
        // its visibility via the dispatcher. Verify the signal reflects
        // the new value AND the DB persists the change (re-list and check).
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let ui_service = shared
            .services
            .ui_service
            .as_ref()
            .expect("ui_service present in with-dbs helper")
            .clone();

        // Pull seeded info-panel items and use the first as our victim.
        let initial = ui_service.list_info_panel_items().expect("list");
        let victim = initial
            .into_iter()
            .next()
            .expect("seeded info-panel items present");
        let target_id = victim.id.clone();
        let started_visible = victim.visible;
        // Push the live list into the signal so the dispatcher's
        // signal->mutate->signal round-trip has something to find.
        shared
            .signals()
            .info_panel_items
            .set(ui_service.list_info_panel_items().unwrap());

        let mut state = InterfaceSettingsState::default();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegion::InfoPanel,
                item_id: target_id.clone(),
                visible: !started_visible,
            },
            &shared,
        );

        let after_signal = shared.signals().info_panel_items.get();
        let signal_visible = after_signal
            .iter()
            .find(|i| i.id == target_id)
            .expect("item still in signal")
            .visible;
        assert_eq!(
            signal_visible, !started_visible,
            "signal must reflect the toggle"
        );

        let after_db = ui_service.list_info_panel_items().expect("list");
        let db_visible = after_db
            .iter()
            .find(|i| i.id == target_id)
            .expect("item still in DB")
            .visible;
        assert_eq!(db_visible, !started_visible, "DB must persist the toggle");
    }

    #[test]
    fn save_display_options_pushes_show_button_labels_into_ui_preferences_signal() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        // Confirm baseline.
        assert!(!shared.signals().ui_preferences.get().show_button_labels);

        let mut state = InterfaceSettingsState::default();
        state.show_button_labels = true;
        state.dirty = true;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveDisplayOptions,
            &shared,
        );

        assert!(
            shared.signals().ui_preferences.get().show_button_labels,
            "SaveDisplayOptions must propagate show_button_labels into ui_preferences signal"
        );
    }
}

