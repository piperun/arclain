//! Integration tests for the chrome-layout facade surface:
//! `ArclainApp::list_ui_items`/`save_ui_items` and
//! `ArclainApp::ui_display_options`/`save_ui_display_options`.
//!
//! `crates/app/src/layout.rs`'s own unit tests cover the DTO mirrors and
//! the write-path validation in isolation (pure functions, no I/O); this
//! file's job is proving those pieces are wired together correctly behind
//! the public API against a real bootstrap -- a real SQLite config
//! database in a temp profile, seeded with the application's own default
//! layout -- the same way `organization_facade.rs`/`settings_facade.rs`
//! already do for their own surfaces.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! following this crate's established convention: `ArclainApp` owns its
//! own Tokio runtime, and dropping it must not happen from inside an
//! async context (see `archive_sessions.rs`'s own module doc comment).

mod support;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::layout::{
    UiActionTypeDto, UiDisplayModeDto, UiDisplayOptionsDto, UiItemDto, UiRegionDto, UiViewModeDto,
    MAX_UI_PANEL_WIDTH_PX,
};
use arclain_app::{ArclainApp, BootstrapConfig};

// ============================================================================
// Harness.
// ============================================================================

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn worker_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

/// Bootstraps an `ArclainApp` against an isolated temp profile -- see
/// `organization_facade.rs::bootstrap_app`'s identical doc comment for why
/// the dummy 7-Zip seeding is required even for tests that never touch an
/// archive backend.
fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    let sevenzip = support::create_dummy_executable(temp.path(), sevenzip_exe_name());
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .expect("bootstrap the application")
}

const EVERY_REGION: [UiRegionDto; 4] = [
    UiRegionDto::Toolbar,
    UiRegionDto::ContextMenu,
    UiRegionDto::ToolsDialog,
    UiRegionDto::InfoPanel,
];

fn ids(items: &[UiItemDto]) -> Vec<&str> {
    items.iter().map(|item| item.id.as_str()).collect()
}

// ============================================================================
// Fresh-profile defaults.
// ============================================================================

/// The facade's answer must be the storage service's answer, region for
/// region and field for field -- otherwise the mirror is lying about
/// something and every "identical layout before and after" claim below is
/// only checking the facade against itself.
#[test]
fn a_fresh_profile_reports_exactly_what_the_storage_service_reports() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let legacy = app
        .take_legacy_composition()
        .expect("take the legacy composition");
    let service = legacy
        .core_services
        .ui_service
        .clone()
        .expect("a bootstrapped application has a ui service");

    for region in EVERY_REGION {
        let through_facade = runtime
            .block_on(app.list_ui_items(region))
            .expect("list through the facade");
        let through_service = service
            .list_items(region.into())
            .expect("list through the service");

        assert_eq!(
            through_facade.len(),
            through_service.len(),
            "{region:?} must report the same number of items either way"
        );
        for (mirrored, stored) in through_facade.iter().zip(through_service.iter()) {
            assert_eq!(mirrored, &UiItemDto::from(stored.clone()));
        }
    }
}

/// A concrete pin on the seeded toolbar so the comparison above cannot
/// pass by both sides being empty, and so a change to the shipped defaults
/// is a deliberate edit here rather than a silent one.
#[test]
fn a_fresh_profile_reports_the_seeded_toolbar_layout() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let items = runtime
        .block_on(app.list_ui_items(UiRegionDto::Toolbar))
        .expect("list the toolbar");

    assert_eq!(
        ids(&items),
        vec![
            "toolbar.back",
            "toolbar.forward",
            "toolbar.up",
            "toolbar.open",
            "toolbar.extract",
            "toolbar.extract_all",
            "toolbar.add",
            "toolbar.delete",
            "toolbar.convert",
            "toolbar.batch_convert",
            "toolbar.organize",
            "toolbar.list_view",
            "toolbar.grid_view",
            "toolbar.column_lock",
            "toolbar.tree_panel",
            "toolbar.properties_panel",
        ],
        "the seeded toolbar comes back in sort order, navigation first"
    );

    let back = &items[0];
    assert_eq!(back.region, UiRegionDto::Toolbar);
    assert_eq!(back.group_id.as_deref(), Some("navigation"));
    assert_eq!(back.label, "Back");
    assert_eq!(back.icon.as_deref(), Some("ARROW_LEFT"));
    assert!(back.visible);
    assert_eq!(back.sort_order, 0);
    assert_eq!(back.display_mode, UiDisplayModeDto::IconOnly);
    assert_eq!(back.action_type, UiActionTypeDto::Builtin);
    assert_eq!(back.action_data, None);

    // Icon-only navigation and icon+text file actions is the whole point
    // of storing a display mode per item, so the two must not have
    // collapsed onto one value.
    let extract = items
        .iter()
        .find(|item| item.id == "toolbar.extract")
        .expect("the seeded extract button");
    assert_eq!(extract.group_id.as_deref(), Some("file_actions"));
    assert_eq!(extract.display_mode, UiDisplayModeDto::IconAndText);
}

#[test]
fn a_fresh_profile_reports_the_seeded_context_menu_and_info_panel() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let context = runtime
        .block_on(app.list_ui_items(UiRegionDto::ContextMenu))
        .expect("list the context menu");
    assert_eq!(
        ids(&context),
        vec![
            "context.open",
            "context.extract",
            "context.extract_to",
            "context.copy_path",
            "context.delete",
            "context.properties",
        ]
    );
    assert!(context
        .iter()
        .all(|item| item.region == UiRegionDto::ContextMenu && item.group_id.is_none()));

    let info = runtime
        .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
        .expect("list the info panel");
    assert_eq!(
        ids(&info),
        vec!["info.archive", "info.file", "info.attributes"]
    );
    assert!(info
        .iter()
        .all(|item| item.display_mode == UiDisplayModeDto::TextOnly));

    // Nothing seeds the tools dialog, so an unpopulated region reports an
    // empty list rather than failing.
    assert!(runtime
        .block_on(app.list_ui_items(UiRegionDto::ToolsDialog))
        .expect("list the tools dialog")
        .is_empty());
}

// ============================================================================
// Item round trip.
// ============================================================================

/// The editor contract: edit what was read, save it, read it back, get
/// exactly what was saved -- including a reorder, a hide, a relabel and a
/// display-mode change in one batch.
#[test]
fn a_saved_arrangement_reads_back_identically() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let mut items = runtime
        .block_on(app.list_ui_items(UiRegionDto::Toolbar))
        .expect("list the toolbar");

    // Swap the first two items' order the way the editor's move buttons
    // do (exchange sort_order, not position), hide one, and retitle
    // another with a different display mode.
    let first_order = items[0].sort_order;
    items[0].sort_order = items[1].sort_order;
    items[1].sort_order = first_order;
    items[2].visible = false;
    items[3].label = "Open an archive".to_string();
    items[3].display_mode = UiDisplayModeDto::TextOnly;

    let mut expected = items.clone();
    expected.sort_by_key(|item| item.sort_order);

    runtime
        .block_on(app.save_ui_items(UiRegionDto::Toolbar, items))
        .expect("save the arrangement");

    let reloaded = runtime
        .block_on(app.list_ui_items(UiRegionDto::Toolbar))
        .expect("re-list the toolbar");

    assert_eq!(reloaded, expected);
    // Saving twice from the reloaded list is a no-op, which is what makes
    // the editor's "save, navigate away, come back" cycle stable.
    runtime
        .block_on(app.save_ui_items(UiRegionDto::Toolbar, reloaded.clone()))
        .expect("re-save the same arrangement");
    assert_eq!(
        runtime
            .block_on(app.list_ui_items(UiRegionDto::Toolbar))
            .expect("re-list again"),
        reloaded
    );
}

#[test]
fn a_new_item_id_is_created_rather_than_refused() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let plugin_item = UiItemDto {
        id: "plugin_example_fetch".to_string(),
        region: UiRegionDto::Toolbar,
        group_id: Some("plugins".to_string()),
        label: "Example - Fetch".to_string(),
        icon: Some("PUZZLE_PIECE".to_string()),
        visible: true,
        sort_order: 999,
        display_mode: UiDisplayModeDto::IconAndText,
        action_type: UiActionTypeDto::Plugin,
        action_data: Some("example:fetch".to_string()),
    };

    runtime
        .block_on(app.save_ui_items(UiRegionDto::Toolbar, vec![plugin_item.clone()]))
        .expect("a plugin-contributed item is a create");

    let reloaded = runtime
        .block_on(app.list_ui_items(UiRegionDto::Toolbar))
        .expect("re-list the toolbar");
    assert_eq!(
        reloaded.last(),
        Some(&plugin_item),
        "the new item lands last, by its own sort order"
    );
}

/// Upsert, not replace. A frontend that saves a *subset* of a region must
/// not thereby delete the rows it left out -- that is what lets a layout
/// editor filter host-managed items out of the list it shows without
/// destroying them.
#[test]
fn a_save_leaves_the_rows_it_did_not_mention_untouched() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let before = runtime
        .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
        .expect("list the info panel");
    let omitted = before
        .iter()
        .find(|item| item.id == "info.attributes")
        .expect("the seeded attributes section")
        .clone();

    let mut subset: Vec<UiItemDto> = before
        .iter()
        .filter(|item| item.id != omitted.id)
        .cloned()
        .collect();
    subset[0].visible = false;

    runtime
        .block_on(app.save_ui_items(UiRegionDto::InfoPanel, subset))
        .expect("save a filtered subset");

    let after = runtime
        .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
        .expect("re-list the info panel");
    assert_eq!(
        ids(&after),
        ids(&before),
        "an omitted row survives the save"
    );
    assert_eq!(
        after
            .iter()
            .find(|item| item.id == omitted.id)
            .expect("the omitted row is still there"),
        &omitted,
        "and survives it unchanged"
    );
    assert!(
        !after
            .iter()
            .find(|item| item.id == before[0].id)
            .expect("the saved row")
            .visible,
        "the rows that were mentioned still took the edit"
    );
}

// ============================================================================
// Refusals never write.
// ============================================================================

#[test]
fn an_item_named_for_another_region_is_refused_and_nothing_is_written() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let before = runtime
        .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
        .expect("list the info panel");

    let smuggled = UiItemDto {
        id: "smuggled.item".to_string(),
        region: UiRegionDto::Toolbar,
        group_id: None,
        label: "Smuggled".to_string(),
        icon: None,
        visible: true,
        sort_order: 0,
        display_mode: UiDisplayModeDto::TextOnly,
        action_type: UiActionTypeDto::Builtin,
        action_data: None,
    };

    let error = runtime
        .block_on(app.save_ui_items(UiRegionDto::InfoPanel, vec![smuggled]))
        .expect_err("a toolbar item must not be written into the info panel");
    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("items[0].region"));

    assert_eq!(
        runtime
            .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
            .expect("re-list the info panel"),
        before
    );
    assert!(
        !runtime
            .block_on(app.list_ui_items(UiRegionDto::Toolbar))
            .expect("re-list the toolbar")
            .iter()
            .any(|item| item.id == "smuggled.item"),
        "and it must not have landed in the region it named either"
    );
}

/// Validation covers the whole batch before the first write, so one bad
/// item at the end cannot leave the good ones ahead of it persisted.
#[test]
fn a_refused_batch_writes_none_of_its_items() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let good = UiItemDto {
        id: "toolbar.brand_new".to_string(),
        region: UiRegionDto::Toolbar,
        group_id: Some("plugins".to_string()),
        label: "Brand New".to_string(),
        icon: None,
        visible: true,
        sort_order: 500,
        display_mode: UiDisplayModeDto::IconAndText,
        action_type: UiActionTypeDto::Builtin,
        action_data: None,
    };
    let mut bad = good.clone();
    bad.id = String::new();

    let error = runtime
        .block_on(app.save_ui_items(UiRegionDto::Toolbar, vec![good, bad]))
        .expect_err("an empty id must be refused");
    assert_eq!(error.field.as_deref(), Some("items[1].id"));

    assert!(
        !runtime
            .block_on(app.list_ui_items(UiRegionDto::Toolbar))
            .expect("re-list the toolbar")
            .iter()
            .any(|item| item.id == "toolbar.brand_new"),
        "the valid item ahead of the invalid one must not have been written"
    );
}

// ============================================================================
// Display options.
// ============================================================================

#[test]
fn a_fresh_profile_reports_the_default_display_options() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    assert_eq!(
        runtime
            .block_on(app.ui_display_options())
            .expect("read the display options"),
        UiDisplayOptionsDto::default(),
        "the shipped seed and the DTO's own default must agree, or a fresh \
         profile and an unconfigured one would disagree"
    );
}

#[test]
fn saved_display_options_read_back_identically() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let edited = UiDisplayOptionsDto {
        default_view_mode: UiViewModeDto::Grid,
        tree_panel_visible: false,
        tree_panel_width: 321.5,
        properties_panel_visible: false,
        properties_panel_width: 456.25,
        show_button_labels: true,
    };

    runtime
        .block_on(app.save_ui_display_options(edited))
        .expect("save the display options");

    assert_eq!(
        runtime
            .block_on(app.ui_display_options())
            .expect("re-read the display options"),
        edited,
        "every field, including the fractional widths, survives storage"
    );
}

#[test]
fn a_refused_display_option_write_leaves_the_stored_options_alone() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    let stored = UiDisplayOptionsDto {
        show_button_labels: true,
        ..UiDisplayOptionsDto::default()
    };
    runtime
        .block_on(app.save_ui_display_options(stored))
        .expect("save a valid baseline");

    let mut absurd = stored;
    absurd.show_button_labels = false;
    absurd.tree_panel_width = MAX_UI_PANEL_WIDTH_PX * 2.0;

    let error = runtime
        .block_on(app.save_ui_display_options(absurd))
        .expect_err("an absurd width must be refused");
    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("tree_panel_width"));

    assert_eq!(
        runtime
            .block_on(app.ui_display_options())
            .expect("re-read the display options"),
        stored,
        "no field of a refused write may have landed"
    );
}

/// The six display-option keys are one logical value, so a save must
/// land all of them or none of them. Induce a failure partway through
/// the batch -- ABORT triggers on the last-written key
/// (`show_button_labels`), installed on the same database file through
/// a second connection -- and assert the refused save left every key
/// at its stored value, including the ones written before the failure.
#[test]
fn a_display_option_save_that_fails_midway_lands_none_of_its_keys() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let paths = support::temp_paths(temp.path());
    let sevenzip = support::create_dummy_executable(temp.path(), sevenzip_exe_name());
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        ..Default::default()
    })
    .expect("bootstrap the application");
    let runtime = foreign_runtime();

    // A stored baseline that is not the defaults, so "rolled back to
    // the baseline" and "reset to a fresh profile" are distinguishable.
    let baseline = UiDisplayOptionsDto {
        tree_panel_visible: false,
        properties_panel_width: 333.0,
        ..UiDisplayOptionsDto::default()
    };
    runtime
        .block_on(app.save_ui_display_options(baseline))
        .expect("save the baseline");

    // Make any further write of the batch's *last* key fail, whichever
    // upsert branch it takes.
    let config_db = support::databases_dir(&paths).join("config.sqlite");
    arclain_core::config::ConfigDb::open(&config_db)
        .expect("open the config database on a second connection")
        .into_sqlite_db()
        .with_connection(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER induced_display_option_insert_failure
                 BEFORE INSERT ON ui_display_options
                 WHEN NEW.key = 'show_button_labels'
                 BEGIN SELECT RAISE(ABORT, 'induced display-option failure'); END;
                 CREATE TRIGGER induced_display_option_update_failure
                 BEFORE UPDATE ON ui_display_options
                 WHEN NEW.key = 'show_button_labels'
                 BEGIN SELECT RAISE(ABORT, 'induced display-option failure'); END;",
            )?;
            Ok(())
        })
        .expect("install the poison triggers");

    // The refused save edits the first-written key (the view mode), a
    // middle one (the tree width), and the poisoned last one.
    let edited = UiDisplayOptionsDto {
        default_view_mode: UiViewModeDto::Grid,
        tree_panel_width: 321.5,
        show_button_labels: true,
        ..baseline
    };
    let error = runtime
        .block_on(app.save_ui_display_options(edited))
        .expect_err("the poisoned key must fail the save");
    assert_eq!(error.kind, ApplicationErrorKind::Backend);

    assert_eq!(
        runtime
            .block_on(app.ui_display_options())
            .expect("re-read the display options"),
        baseline,
        "a save that failed partway must land none of its keys"
    );
}

// ============================================================================
// Serialization and concurrency.
// ============================================================================

/// Two full arrangements of one region saved concurrently must end as one
/// of the two, never a blend of both -- which is what
/// `settings_write_lock` buys a batch write that SQLite would otherwise
/// only serialize statement by statement.
#[test]
fn two_concurrent_saves_of_one_region_do_not_interleave() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = worker_runtime();

    let baseline = runtime
        .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
        .expect("list the info panel");
    assert!(baseline.len() >= 3, "the seed must give us several rows");

    // Two arrangements that disagree about *every* row, so any
    // interleaving is detectable rather than coincidentally identical.
    let all_labelled = |suffix: &str| -> Vec<UiItemDto> {
        baseline
            .iter()
            .map(|item| {
                let mut item = item.clone();
                item.label = format!("{}{suffix}", item.id);
                item
            })
            .collect()
    };
    let first = all_labelled("-first");
    let second = all_labelled("-second");

    let (left, right) = runtime.block_on(async {
        tokio::join!(
            app.save_ui_items(UiRegionDto::InfoPanel, first.clone()),
            app.save_ui_items(UiRegionDto::InfoPanel, second.clone()),
        )
    });
    left.expect("the first save must succeed");
    right.expect("the second save must succeed");

    let stored = runtime
        .block_on(app.list_ui_items(UiRegionDto::InfoPanel))
        .expect("re-list the info panel");
    assert!(
        stored == first || stored == second,
        "the stored layout must be one whole arrangement, not a mix: {:?}",
        ids(&stored)
    );
}

/// The whole surface is awaitable from a foreign runtime, per the crate's
/// executor-agnostic rule. Every other test here uses a `current_thread`
/// runtime; this one also drives it from a multi-thread one, since the two
/// poll a `spawn_blocking`-backed future differently.
#[test]
fn the_layout_surface_is_awaitable_from_a_multi_thread_runtime() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = worker_runtime();

    let items = runtime
        .block_on(app.list_ui_items(UiRegionDto::ContextMenu))
        .expect("list the context menu from a foreign multi-thread runtime");
    assert!(!items.is_empty());

    runtime
        .block_on(app.save_ui_items(UiRegionDto::ContextMenu, items))
        .expect("save from a foreign multi-thread runtime");
    runtime
        .block_on(app.save_ui_display_options(UiDisplayOptionsDto::default()))
        .expect("save display options from a foreign multi-thread runtime");
}

#[test]
fn the_layout_surface_reports_the_shutdown_error_after_shutdown() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let app = bootstrap_app(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.shutdown())
        .expect("shut the application down");

    for kind in [
        runtime
            .block_on(app.list_ui_items(UiRegionDto::Toolbar))
            .expect_err("a shut-down application serves no reads")
            .kind,
        runtime
            .block_on(app.save_ui_items(UiRegionDto::Toolbar, Vec::new()))
            .expect_err("a shut-down application serves no writes")
            .kind,
        runtime
            .block_on(app.ui_display_options())
            .expect_err("a shut-down application serves no option reads")
            .kind,
        runtime
            .block_on(app.save_ui_display_options(UiDisplayOptionsDto::default()))
            .expect_err("a shut-down application serves no option writes")
            .kind,
    ] {
        assert_eq!(kind, ApplicationErrorKind::Internal);
    }
}
