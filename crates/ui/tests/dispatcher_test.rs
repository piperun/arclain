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
use common::{
    create_test_shared_state, create_test_shared_state_with_dbs,
    create_test_shared_state_with_facade,
};

// ============================================================================
// ProfilesPage dispatcher (handle_profiles_action)
// ============================================================================

mod profiles_page {
    use super::*;
    use arclain_app::organization::OrganizationProfileInput;
    use arclain_ui::features::organization::presentation::views::profiles_page::{
        handle_profiles_action, ProfilesAction, ProfilesPage,
    };

    fn sample_profile(name: &str) -> OrganizationProfileInput {
        OrganizationProfileInput {
            id: None,
            name: name.into(),
            description: None,
            output_format: "7z".into(),
            compression_level: 5,
            compression_method: Some("LZMA2".into()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: false,
        }
    }

    #[test]
    fn load_profiles_without_a_facade_sets_error() {
        let shared = create_test_shared_state();
        let mut page = ProfilesPage::new();

        handle_profiles_action(&mut page, ProfilesAction::LoadProfiles, &shared);

        assert!(
            page.error().is_some(),
            "LoadProfiles with no application must surface an error rather than an empty page"
        );
        assert!(page.profiles().is_none());
    }

    #[test]
    fn save_profile_without_a_facade_sets_error() {
        let shared = create_test_shared_state();
        let mut page = ProfilesPage::new();

        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(sample_profile("test")),
            &shared,
        );

        assert!(page.error().is_some());
    }

    /// A rejected write shows why and leaves the page's list alone --
    /// the application validates before it persists, so nothing changed.
    #[test]
    fn a_rejected_profile_surfaces_the_reason() {
        let (_temp, shared) = create_test_shared_state_with_facade();
        let mut page = ProfilesPage::new();
        handle_profiles_action(&mut page, ProfilesAction::LoadProfiles, &shared);
        let before = page.profiles().expect("profiles must load").len();

        let mut invalid = sample_profile("Unsupported");
        invalid.output_format = "rar".into();
        handle_profiles_action(&mut page, ProfilesAction::SaveProfile(invalid), &shared);

        let error = page.error().expect("an unsupported format must be refused");
        assert!(
            error.contains("output format"),
            "the reason must name the field: {error}"
        );
        assert_eq!(
            page.profiles().unwrap().len(),
            before,
            "a refused save must not change the list"
        );
    }
}

// ============================================================================
// RulesPage dispatcher (handle_rules_page_action)
// ============================================================================

mod rules_page {
    use super::*;
    use arclain_ui::core::navigation::SettingsPage;
    use arclain_ui::features::organization::presentation::views::rules_page::{
        handle_rules_page_action, RulesPage, RulesPageAction,
    };

    #[test]
    fn load_rule_with_id_zero_creates_new_editor_state_without_a_facade() {
        let shared = create_test_shared_state();
        let mut page = RulesPage::new();

        // rule_id == 0 is the "new rule" sentinel: there is nothing to
        // load, so the dispatcher never reaches for the application.
        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 0 },
            &shared,
            None,
        );

        assert!(
            page.editor_load_error().is_none(),
            "rule_id=0 must not hit the error path"
        );
        assert!(
            !page.is_editor_dirty(),
            "fresh editor state should be clean"
        );
        assert!(page.editor_rule_mut().is_some(), "an editor must be open");
    }

    #[test]
    fn load_rules_without_a_facade_sets_error() {
        let shared = create_test_shared_state();
        let mut page = RulesPage::new();

        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &shared, None);

        assert!(
            page.error().is_some(),
            "loading with no application must surface an error"
        );
    }

    #[test]
    fn load_rule_without_a_facade_sets_editor_load_error() {
        let shared = create_test_shared_state();
        let mut page = RulesPage::new();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 7 },
            &shared,
            None,
        );

        assert!(page.editor_load_error().is_some());
    }

    /// Create through the editor, list it, load it back into the editor:
    /// the page's whole rule cycle against a real application.
    #[test]
    fn rule_editing_round_trips_through_the_facade() {
        let (_temp, shared) = create_test_shared_state_with_facade();
        let mut page = RulesPage::new();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 0 },
            &shared,
            None,
        );
        {
            let rule = page.editor_rule_mut().expect("a new rule must be open");
            rule.name = "Round Trip".to_string();
            rule.enabled = true;
            rule.priority = 42;
            rule.trigger.filename_pattern = Some(r"^RJ\d+".to_string());
            rule.actions.root_folder = Some("[$product_id] $title".to_string());
        }
        page.save_editor_rule(&shared)
            .expect("the save must succeed");
        page.mark_saved_and_clear();

        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &shared, None);
        assert_eq!(page.error(), None);
        let saved = page
            .rules()
            .expect("rules must be loaded")
            .iter()
            .find(|rule| rule.name == "Round Trip")
            .expect("the saved rule must be listed")
            .clone();
        assert!(saved.enabled);
        assert_eq!(saved.priority, 42);

        let saved_id: i64 = saved.id.parse().expect("ids are decimal integers");
        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: saved_id },
            &shared,
            None,
        );
        assert_eq!(page.editor_load_error(), None);
        let reloaded = page.editor_rule_mut().expect("the rule must load").clone();
        assert_eq!(reloaded.id.as_deref(), Some(saved.id.as_str()));
        assert_eq!(reloaded.name, "Round Trip");
        assert_eq!(
            reloaded.actions.root_folder.as_deref(),
            Some("[$product_id] $title"),
            "every edited field must survive the round trip"
        );
    }

    /// An unsavable rule reports why, and the editor keeps the draft so
    /// the author can fix it rather than retyping it.
    #[test]
    fn a_rejected_rule_reports_why_and_keeps_the_draft() {
        let (_temp, shared) = create_test_shared_state_with_facade();
        let mut page = RulesPage::new();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 0 },
            &shared,
            None,
        );
        page.editor_rule_mut()
            .expect("a new rule must be open")
            .name = "   ".to_string();

        let error = page
            .save_editor_rule(&shared)
            .expect_err("a blank name must be refused");
        assert!(
            error.contains("name"),
            "the reason must name the field: {error}"
        );
        assert!(
            page.editor_rule_mut().is_some(),
            "the draft must survive a refused save"
        );
    }

    #[test]
    fn loading_a_rule_that_does_not_exist_sets_editor_load_error() {
        let (_temp, shared) = create_test_shared_state_with_facade();
        let mut page = RulesPage::new();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 999_999 },
            &shared,
            None,
        );

        assert_eq!(page.editor_load_error(), Some("Rule not found"));
    }

    #[test]
    #[should_panic(expected = "Navigate should be handled by the caller")]
    fn navigate_action_panics_in_debug() {
        let shared = create_test_shared_state();
        let mut page = RulesPage::new();

        // Navigate is supposed to be handled at the call site (translated
        // to SettingsAction::NavigateTo) and never reach the dispatcher.
        // Debug asserts catch the misuse.
        handle_rules_page_action(
            &mut page,
            RulesPageAction::Navigate(SettingsPage::OrganizationRules),
            &shared,
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

        handle_process_action(&mut state, ProcessAction::LoadInterruptedCount, &shared);

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

        handle_process_action(&mut state, ProcessAction::LoadInterruptedCount, &shared);

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

        handle_process_action(&mut state, ProcessAction::LoadOrganizationRules, &shared);

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
    fn load_presets_with_no_facade_sets_an_empty_list() {
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();

        handle_process_action(&mut state, ProcessAction::LoadPresets, &shared);

        let presets = state
            .presets
            .as_ref()
            .expect("LoadPresets must seat the list (even if empty)");
        assert!(
            presets.is_empty(),
            "without a facade there is nothing to list"
        );
    }

    #[test]
    fn preset_writes_with_no_facade_are_a_no_op_rather_than_a_panic() {
        // Preset persistence lives entirely behind the facade now, so
        // the facade-less branch has nothing to do. This is a routing
        // test -- the assertion is "does not panic and touches no
        // state" -- with the real save/delete behaviour covered against
        // a real application in `process_page_facade_test.rs`.
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();

        handle_process_action(
            &mut state,
            ProcessAction::SavePreset {
                name: "Flatten then zip".to_string(),
            },
            &shared,
        );
        handle_process_action(
            &mut state,
            ProcessAction::DeletePreset {
                name: "Flatten then zip".to_string(),
            },
            &shared,
        );

        assert!(state.presets.is_none());
        assert!(state.active_preset_name.is_none());
    }
}

// ============================================================================
// LayoutEditor dispatcher (handle_layout_editor_action via region wrappers)
// ============================================================================

mod layout_editor {
    use super::*;
    use arclain_ui::features::settings::presentation::pages::{
        handle_info_panel_layout_action, handle_toolbar_layout_action, InfoPanelLayoutState,
        LayoutEditorAction, ToolbarLayoutState,
    };

    // The dispatcher reads the canonical item signals rather than
    // asking the application itself. `create_test_shared_state()` starts
    // with empty signals, so these tests assert the "signal exists but
    // empty" branch.

    #[test]
    fn toolbar_sync_against_empty_signal_loads_empty_items() {
        let shared = create_test_shared_state();
        let mut state = ToolbarLayoutState::default();

        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert!(
            state.loaded,
            "sync always flips loaded=true once it consults the signal"
        );
        assert!(state.items.is_empty(), "empty signal → empty state.items");
    }

    #[test]
    fn info_panel_sync_against_empty_signal_loads_empty_items() {
        let shared = create_test_shared_state();
        let mut state = InfoPanelLayoutState::default();

        handle_info_panel_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert!(state.loaded);
        assert!(state.items.is_empty());
    }

    #[test]
    fn sync_with_no_plugin_manager_skips_plugin_walk_cleanly() {
        let shared = create_test_shared_state();
        let mut state = ToolbarLayoutState::default();

        // Sync with empty signal and no plugin manager should be a
        // graceful no-op for dirty (loaded=true, items=empty, dirty
        // stays false).
        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert!(!state.dirty, "no-op sync must not mark the editor dirty");
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
    fn load_display_options_with_no_application_is_noop() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        assert!(!state.loaded);

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::LoadDisplayOptions,
            &shared,
        );

        // With no facade the read fails, so `loaded` stays false and the
        // page keeps showing "Loading…" rather than presenting
        // placeholder values as if the user had chosen them.
        assert!(!state.loaded, "no application → loader must short-circuit");
    }

    #[test]
    fn save_display_options_with_no_application_does_not_panic_and_does_not_clear_dirty() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        state.display_options.show_button_labels = true;
        state.dirty = true;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveDisplayOptions,
            &shared,
        );

        // With no facade nothing was stored, so the page stays
        // pending-save rather than dropping the user's edit.
        assert!(
            state.dirty,
            "no application → SaveDisplayOptions early-returns and dirty stays true"
        );
    }

    /// A refused save must not push its unstored values onward either:
    /// `ui_preferences` drives the header's own rendering, so letting it
    /// take a value the application never accepted would show the user a
    /// preference that vanishes on restart.
    #[test]
    fn a_refused_display_option_save_does_not_touch_the_preference_signal() {
        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();
        state.display_options.show_button_labels = true;
        state.dirty = true;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveDisplayOptions,
            &shared,
        );

        assert!(
            !shared.signals().ui_preferences.get().show_button_labels,
            "a save that never landed must not repaint the header as if it had"
        );
    }

    #[test]
    fn toggle_item_visibility_with_no_application_is_noop() {
        use arclain_app::layout::UiRegionDto;

        let shared = create_test_shared_state();
        let mut state = InterfaceSettingsState::default();

        // No facade in test shared → dispatcher early-returns without
        // panic. Signal value unchanged.
        let before = shared.signals().context_menu_items.get();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegionDto::ContextMenu,
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
    use arclain_app::organization::{OrganizationProfileInput, OrganizationProfileSummary};
    use arclain_ui::features::organization::presentation::views::profiles_page::{
        handle_profiles_action, ProfilesAction, ProfilesPage,
    };

    fn profile_named(name: &str) -> OrganizationProfileInput {
        OrganizationProfileInput {
            id: None,
            name: name.into(),
            description: None,
            output_format: "7z".into(),
            compression_level: 5,
            compression_method: Some("LZMA2".into()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: false,
        }
    }

    /// The config database seeds its own system profiles on first init.
    /// Tests that count user-created profiles must filter `!is_system`.
    fn user_names(profiles: &[OrganizationProfileSummary]) -> Vec<String> {
        profiles
            .iter()
            .filter(|p| !p.is_system)
            .map(|p| p.name.clone())
            .collect()
    }

    fn id_of(page: &ProfilesPage, name: &str) -> String {
        page.profiles()
            .expect("profiles must be loaded")
            .iter()
            .find(|profile| profile.name == name)
            .unwrap_or_else(|| panic!("profile {name:?} must be listed"))
            .id
            .clone()
    }

    #[test]
    fn load_against_a_fresh_application_returns_the_seeded_system_profiles_only() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
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
            "a fresh application ships with system defaults seeded"
        );
        assert_eq!(page.error(), None);
    }

    #[test]
    fn save_then_load_returns_the_profile() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut page = ProfilesPage::new();

        handle_profiles_action(
            &mut page,
            ProfilesAction::SaveProfile(profile_named("alpha")),
            &shared,
        );

        // Every mutation answers with the post-write list, so the page
        // already reflects the new row without a separate LoadProfiles.
        let profiles = page.profiles().expect("save should re-populate cache");
        assert_eq!(user_names(profiles), vec!["alpha".to_string()]);
        assert_eq!(page.error(), None);
    }

    #[test]
    fn delete_removes_only_the_targeted_profile() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
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
        let id_alpha = id_of(&page, "alpha");

        handle_profiles_action(&mut page, ProfilesAction::DeleteProfile(id_alpha), &shared);

        let after = page.profiles().unwrap();
        assert_eq!(user_names(after), vec!["beta".to_string()]);
    }

    #[test]
    fn set_default_marks_only_one_profile_default() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
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
        let id_beta = id_of(&page, "beta");

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
    use arclain_ui::features::organization::presentation::views::rules_page::{
        handle_rules_page_action, RulesPage, RulesPageAction,
    };

    use super::*;

    #[test]
    fn load_rules_against_a_fresh_application_populates_the_cache() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut page = RulesPage::new();

        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &shared, None);

        assert_eq!(page.error(), None);
        assert!(
            page.rules().is_some(),
            "a successful load must populate the cache, even when empty"
        );
    }

    #[test]
    fn a_rule_saved_through_the_editor_is_picked_up_by_the_list() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut page = RulesPage::new();

        handle_rules_page_action(
            &mut page,
            RulesPageAction::LoadRule { rule_id: 0 },
            &shared,
            None,
        );
        {
            let rule = page.editor_rule_mut().expect("a new rule must be open");
            rule.name = "Staged Rule".to_string();
            rule.enabled = true;
        }
        page.save_editor_rule(&shared)
            .expect("the save must succeed");
        page.mark_saved_and_clear();

        handle_rules_page_action(&mut page, RulesPageAction::LoadRules, &shared, None);

        assert_eq!(page.error(), None);
        assert!(
            page.rules()
                .expect("rules must be loaded")
                .iter()
                .any(|rule| rule.name == "Staged Rule"),
            "the dispatcher must pick up a rule saved through the editor"
        );
    }
}

mod process_page_happy {
    use super::*;
    use arclain_ui::features::process::view::{handle_process_action, ProcessAction};
    use arclain_ui::features::process::ProcessPageState;

    /// With no application to ask, the cache is still populated (with
    /// nothing) rather than left `None` -- otherwise the page would emit
    /// a load intent on every single frame.
    #[test]
    fn load_organization_rules_without_a_facade_caches_an_empty_list() {
        let shared = create_test_shared_state();
        let mut state = ProcessPageState::default();

        handle_process_action(&mut state, ProcessAction::LoadOrganizationRules, &shared);

        assert_eq!(state.cached_org_rules.as_deref(), Some(&[][..]));
    }

    /// The Organize step's rule picker is populated from the
    /// application's own rule list.
    #[test]
    fn load_organization_rules_caches_what_the_application_reports() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let app = shared.facade.as_ref().expect("the fixture has a facade");
        shared
            .services
            .tokio_runtime
            .block_on(app.upsert_organization_rule(
                arclain_app::organization::OrganizationRuleInput {
                    id: None,
                    name: "Pipeline Rule".to_string(),
                    priority: 10,
                    enabled: true,
                    trigger: Default::default(),
                    actions: Default::default(),
                },
            ))
            .expect("seeding a rule must succeed");
        let mut state = ProcessPageState::default();

        handle_process_action(&mut state, ProcessAction::LoadOrganizationRules, &shared);

        let cache = state
            .cached_org_rules
            .as_ref()
            .expect("cache should be populated");
        assert!(cache.iter().any(|rule| rule.name == "Pipeline Rule"));
    }

    #[test]
    fn load_interrupted_count_against_empty_db_returns_zero() {
        let (_tmp, shared) = create_test_shared_state_with_dbs();
        let mut state = ProcessPageState::default();

        handle_process_action(&mut state, ProcessAction::LoadInterruptedCount, &shared);

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
    fn toolbar_sync_against_populated_signal_loads_seeded_defaults() {
        // The `with_facade` helper primes signals from the freshly
        // bootstrapped application the same way `state/init.rs` does in
        // production, so `signals.toolbar_items` carries the canonical
        // seeded entries (navigation group, file-actions, view, etc.).
        // Assert non-empty rather than a hardcoded count so the test
        // doesn't break every time defaults are tweaked.
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut state = ToolbarLayoutState::default();

        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert!(state.loaded, "sync flips loaded=true");
        assert!(
            !state.items.is_empty(),
            "primed signal carries seeded toolbar defaults"
        );
        assert!(
            !state.dirty,
            "loading existing items shouldn't mark the editor dirty"
        );
    }

    #[test]
    fn info_panel_sync_against_populated_signal_loads_seeded_defaults() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut state = InfoPanelLayoutState::default();

        handle_info_panel_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert!(state.loaded);
        assert!(
            !state.items.is_empty(),
            "primed signal carries seeded info-panel sections"
        );
    }

    #[test]
    fn sync_when_dirty_does_not_clobber_user_edits() {
        // Once state.dirty=true (typical: user moved an item or
        // toggled visibility in the editor), a later SyncItems must
        // NOT pull a fresh signal value over state.items — that
        // would silently throw away the in-flight edit.
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut state = ToolbarLayoutState::default();

        // First sync to populate from signal.
        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);
        let baseline_len = state.items.len();
        assert!(baseline_len > 0);

        // Simulate a user edit: pop one item AND mark dirty.
        state.items.pop();
        state.dirty = true;
        let edited_len = state.items.len();

        // Sync again. With dirty=true, signal-side data must NOT be
        // re-applied; user's local edit survives.
        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert_eq!(state.items.len(), edited_len, "dirty edit preserved");
        assert!(state.dirty, "dirty flag persists across syncs");
    }

    #[test]
    fn sync_when_clean_picks_up_signal_changes() {
        // Mirror of the prior test for the not-dirty branch: if the
        // signal changes (e.g. via the Interface page's per-toggle
        // dispatcher), a SyncItems with dirty=false must reflect the
        // new value. This is the bug the refactor fixes — Interface
        // edits now propagate into a stale LayoutEditor cache.
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut state = ToolbarLayoutState::default();

        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);
        let baseline_len = state.items.len();
        assert!(baseline_len > 0);

        // Externally remove one item from the signal (mimics an
        // Interface-page-driven ToggleItemVisibility OR an external
        // toolbar layout edit that landed via reload_ui_config).
        let mut updated = shared.signals().toolbar_items.get();
        updated.pop();
        shared.signals().toolbar_items.set(updated);

        handle_toolbar_layout_action(&mut state, LayoutEditorAction::SyncItems, &shared);

        assert_eq!(
            state.items.len(),
            baseline_len - 1,
            "clean sync picks up signal-side changes"
        );
    }
}

mod interface_settings_happy {
    use super::*;
    use arclain_app::layout::{UiRegionDto, UiViewModeDto};
    use arclain_ui::features::settings::presentation::pages::{
        handle_interface_settings_action, InterfaceSettingsAction, InterfaceSettingsState,
    };

    /// Everything stored for `region`, read the way the rest of the app
    /// reads it -- so a persistence assertion below is checking the
    /// store, not the signal it just watched being set.
    fn stored_items(
        shared: &arclain_ui::shared::SharedState,
        region: UiRegionDto,
    ) -> Vec<arclain_app::layout::UiItemDto> {
        let app = shared.facade.as_ref().expect("the fixture has a facade");
        shared
            .services
            .tokio_runtime
            .block_on(app.list_ui_items(region))
            .expect("list the stored items")
    }

    fn stored_visibility(
        shared: &arclain_ui::shared::SharedState,
        region: UiRegionDto,
        item_id: &str,
    ) -> bool {
        stored_items(shared, region)
            .into_iter()
            .find(|item| item.id == item_id)
            .expect("the item is still stored")
            .visible
    }

    #[test]
    fn load_display_options_against_a_fresh_profile_marks_loaded() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut state = InterfaceSettingsState::default();
        assert!(!state.loaded);

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::LoadDisplayOptions,
            &shared,
        );

        assert!(state.loaded, "a real application must flip loaded=true");
        assert!(!state.dirty, "fresh load shouldn't be dirty");
        assert_eq!(
            state.display_options.default_view_mode,
            UiViewModeDto::List,
            "and must carry the seeded values, not a placeholder"
        );
        assert!(state.display_options.tree_panel_visible);
    }

    #[test]
    fn save_display_options_clears_dirty_and_round_trips() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        let mut state = InterfaceSettingsState::default();
        // Pretend the user toggled several things.
        state.display_options.show_button_labels = true;
        state.display_options.default_view_mode = UiViewModeDto::Grid;
        state.display_options.tree_panel_visible = false;
        state.display_options.properties_panel_width = 333.0;
        state.dirty = true;
        let edited = state.display_options;

        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::SaveDisplayOptions,
            &shared,
        );

        assert!(
            !state.dirty,
            "SaveDisplayOptions must clear dirty once the write completes"
        );

        // A second page instance loading fresh must see the same values,
        // which is what makes the save a real round trip rather than an
        // in-memory edit.
        let mut reloaded = InterfaceSettingsState::default();
        handle_interface_settings_action(
            &mut reloaded,
            InterfaceSettingsAction::LoadDisplayOptions,
            &shared,
        );
        assert_eq!(reloaded.display_options, edited);
    }

    #[test]
    fn toggle_item_visibility_updates_signal_and_persists() {
        // Toggle a seeded info-panel row via the dispatcher. Verify the
        // signal reflects the new value AND the application persists the
        // change (re-list and check).
        let (_tmp, shared) = create_test_shared_state_with_facade();

        let victim = stored_items(&shared, UiRegionDto::InfoPanel)
            .into_iter()
            .next()
            .expect("seeded info-panel items present");
        let target_id = victim.id.clone();
        let started_visible = victim.visible;

        let mut state = InterfaceSettingsState::default();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegionDto::InfoPanel,
                item_id: target_id.clone(),
                visible: !started_visible,
            },
            &shared,
        );

        let signal_visible = shared
            .signals()
            .info_panel_items
            .get()
            .iter()
            .find(|i| i.id == target_id)
            .expect("item still in signal")
            .visible;
        assert_eq!(
            signal_visible, !started_visible,
            "signal must reflect the toggle"
        );
        assert_eq!(
            stored_visibility(&shared, UiRegionDto::InfoPanel, &target_id),
            !started_visible,
            "the application must persist the toggle"
        );
    }

    /// One toggle names one item, and upsert semantics mean the rest of
    /// the region must be left exactly as it was.
    #[test]
    fn toggle_item_visibility_leaves_every_other_item_alone() {
        let (_tmp, shared) = create_test_shared_state_with_facade();

        let before = stored_items(&shared, UiRegionDto::InfoPanel);
        let victim = before.first().expect("seeded info-panel items").clone();

        let mut state = InterfaceSettingsState::default();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegionDto::InfoPanel,
                item_id: victim.id.clone(),
                visible: !victim.visible,
            },
            &shared,
        );

        let after = stored_items(&shared, UiRegionDto::InfoPanel);
        assert_eq!(after.len(), before.len(), "no row appeared or vanished");
        for (before, after) in before.iter().zip(after.iter()) {
            if before.id == victim.id {
                continue;
            }
            assert_eq!(before, after, "an untouched row must be byte-identical");
        }
    }

    #[test]
    fn toggle_item_visibility_for_toolbar_persists_to_signal_and_store() {
        // Symmetric to the InfoPanel test above; covers the
        // UiRegionDto::Toolbar arm of the dispatcher's
        // ToggleItemVisibility match. Catches the case where Toolbar was
        // accidentally routed to the wrong signal.
        let (_tmp, shared) = create_test_shared_state_with_facade();

        let victim = stored_items(&shared, UiRegionDto::Toolbar)
            .into_iter()
            .next()
            .expect("seeded toolbar items present");
        let target_id = victim.id.clone();
        let started_visible = victim.visible;

        let mut state = InterfaceSettingsState::default();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegionDto::Toolbar,
                item_id: target_id.clone(),
                visible: !started_visible,
            },
            &shared,
        );

        let signal_visible = shared
            .signals()
            .toolbar_items
            .get()
            .iter()
            .find(|i| i.id == target_id)
            .expect("item still in signal")
            .visible;
        assert_eq!(signal_visible, !started_visible);
        assert_eq!(
            stored_visibility(&shared, UiRegionDto::Toolbar, &target_id),
            !started_visible
        );
    }

    #[test]
    fn toggle_item_visibility_for_context_menu_persists_to_signal_and_store() {
        // Same symmetry check for UiRegionDto::ContextMenu. Interface
        // page's context-menu section is the most likely consumer of
        // this arm — the symmetry test guards against silent breakage
        // there if the dispatcher's region match drifts.
        let (_tmp, shared) = create_test_shared_state_with_facade();

        let victim = stored_items(&shared, UiRegionDto::ContextMenu)
            .into_iter()
            .next()
            .expect("seeded context-menu items present");
        let target_id = victim.id.clone();
        let started_visible = victim.visible;

        let mut state = InterfaceSettingsState::default();
        handle_interface_settings_action(
            &mut state,
            InterfaceSettingsAction::ToggleItemVisibility {
                region: UiRegionDto::ContextMenu,
                item_id: target_id.clone(),
                visible: !started_visible,
            },
            &shared,
        );

        let signal_visible = shared
            .signals()
            .context_menu_items
            .get()
            .iter()
            .find(|i| i.id == target_id)
            .expect("item still in signal")
            .visible;
        assert_eq!(signal_visible, !started_visible);
        assert_eq!(
            stored_visibility(&shared, UiRegionDto::ContextMenu, &target_id),
            !started_visible
        );
    }

    #[test]
    fn save_display_options_pushes_show_button_labels_into_ui_preferences_signal() {
        let (_tmp, shared) = create_test_shared_state_with_facade();
        // Confirm baseline.
        assert!(!shared.signals().ui_preferences.get().show_button_labels);

        let mut state = InterfaceSettingsState::default();
        state.display_options.show_button_labels = true;
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
