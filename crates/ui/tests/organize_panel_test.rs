//! The organize panel against a real application facade.
//!
//! The panel is a render function that emits intents plus a dispatcher
//! (`presentation::ui::refresh_preview`) that runs them, so these drive
//! the dispatcher directly -- the same pattern
//! `session_event_bridge_test.rs` uses -- and assert on what the panel
//! then holds. What matters here is not pixels but three contracts:
//!
//! * the plan the panel shows is the application's plan for the
//!   selected rule and this session,
//! * a rule that cannot produce a plan says so instead of leaving the
//!   previous rule's plan on screen with Apply still live,
//! * metadata arriving invalidates the plan, because the plan is a
//!   function of that metadata.

mod common;
use common::create_test_shared_state_with_facade;

use std::path::{Path, PathBuf};
use std::time::Duration;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::ids::ArchiveSessionId;
use arclain_app::organization::{
    LayoutDto, OrganizationRuleActionsDto, OrganizationRuleInput, OrganizationRuleSummary,
    OrganizationRuleTriggerDto, PlacementDto, PlacementSourceDto,
};
use arclain_app::ArclainApp;
use arclain_ui::features::organization::presentation::ui::refresh_preview;
use arclain_ui::features::organization::{OrganizePanel, OrganizePanelAction};
use arclain_ui::shared::SharedState;

/// Builds a real ZIP at `dir/name`; the panel reads it through the
/// application's own archive session, so no backend override is needed.
fn build_zip_fixture(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_path, content) in entries {
        writer
            .start_file(*entry_path, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry content");
    }
    writer.finish().expect("finish zip fixture");
    path
}

fn open_session(shared: &SharedState, archive: &Path) -> ArchiveSessionId {
    let app = shared.facade.as_ref().expect("the fixture has a facade");
    shared.services.tokio_runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive.to_path_buf(),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match app.operation(operation_id).await.unwrap().state {
                arclain_app::event::OperationState::Completed {
                    result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
                } => return snapshot.session_id,
                arclain_app::event::OperationState::Failed { error } => {
                    panic!("opening the fixture failed: {error:?}")
                }
                _ => {
                    assert!(std::time::Instant::now() < deadline, "open timed out");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    })
}

fn save_rule(shared: &SharedState, input: OrganizationRuleInput) -> Vec<OrganizationRuleSummary> {
    let app = shared.facade.as_ref().expect("the fixture has a facade");
    shared
        .services
        .tokio_runtime
        .block_on(app.upsert_organization_rule(input))
        .expect("saving the test rule must succeed")
}

/// A rule whose layout is one output called `root_folder`, with a
/// single glob placement into `target`.
fn rule_input(name: &str, root_folder: &str, pattern: &str, target: &str) -> OrganizationRuleInput {
    OrganizationRuleInput {
        id: None,
        name: name.to_string(),
        priority: 10,
        enabled: true,
        trigger: OrganizationRuleTriggerDto::default(),
        actions: OrganizationRuleActionsDto {
            output_name: None,
            layout: LayoutDto {
                name: root_folder.to_string(),
                place: vec![PlacementDto {
                    from: PlacementSourceDto::Matching(pattern.to_string()),
                    into: target.to_string(),
                }],
                ..LayoutDto::default()
            },
        },
    }
}

/// The plan's one output. Every rule here describes a whole-input
/// layout, which resolves to exactly one folder.
fn only_output(
    preview: &arclain_app::organization::OrganizePlanPreview,
) -> &arclain_app::organization::PlannedOutputDto {
    assert_eq!(
        preview.outputs.len(),
        1,
        "a whole-input layout resolves to one output"
    );
    &preview.outputs[0]
}

/// Reports plugin metadata for `session_id` exactly as a plugin's
/// `emit_metadata` host call does.
fn report_plugin_metadata(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    product_id: &str,
    title: &str,
) {
    let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: the session exists"));
    bridge.set_session_metadata(
        session_id.into_raw(),
        Some(serde_json::json!({
            "product_id": product_id,
            "source": "dlsite",
            "title": title,
        })),
    );
}

fn panel_for(
    session_id: ArchiveSessionId,
    archive_name: &str,
    rules: Vec<OrganizationRuleSummary>,
) -> OrganizePanel {
    OrganizePanel::new(
        session_id,
        archive_name.to_string(),
        rules,
        Vec::new(),
        None,
        &[],
    )
}

/// The panel shows the application's plan for the selected rule, and
/// the archive's own file list beside it.
#[test]
fn a_refresh_installs_the_plan_for_the_selected_rule() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let archive = build_zip_fixture(
        _temp.path(),
        "[RJ123456] Placeholder.zip",
        &[
            ("wrapper/Game.exe", b"executable bytes"),
            ("wrapper/readme.txt", b"read me"),
        ],
    );
    let rules = save_rule(&shared, rule_input("Panel Rule", "Organized", "**", ""));
    let rules: Vec<_> = rules
        .into_iter()
        .filter(|rule| rule.name == "Panel Rule")
        .collect();
    let session_id = open_session(&shared, &archive);
    let mut panel = panel_for(session_id, "[RJ123456] Placeholder.zip", rules);

    assert!(panel.preview().is_none(), "nothing is computed up front");

    refresh_preview(&mut panel, &shared);

    let preview = panel.preview().expect("a plan must be installed");
    assert_eq!(preview.session_id, session_id);
    assert_eq!(preview.rule_name, "Panel Rule");
    let output = only_output(preview);
    assert_eq!(output.root_folder, "Organized");
    // A placement strips only what its own glob spells out, so the
    // wrapper folder travels with the files under it.
    let destinations: Vec<&str> = output
        .moves
        .iter()
        .map(|planned| planned.destination.as_str())
        .collect();
    assert_eq!(
        destinations,
        vec!["Organized/wrapper/Game.exe", "Organized/wrapper/readme.txt"]
    );
    assert!(
        !output.reasoning.is_empty(),
        "the plan must say how it arrived at this output"
    );
    assert_eq!(panel.preview_error(), None);

    // The "Original" side is the archive's own content, fetched once.
    assert!(
        !panel.ui_state.original_tree.is_empty(),
        "the original tree must be built from the archive's file list"
    );
    assert!(
        !panel.needs_original_paths(),
        "the file list is not re-fetched on every preview"
    );
    assert!(
        !panel.ui_state.organized_tree.is_empty(),
        "the organized tree must be built from the plan"
    );
}

/// Regression for the swallowed failure: a rule that cannot produce a
/// plan for this archive must render the reason and leave nothing
/// applyable behind. The pre-facade panel kept the previous plan on
/// screen with Apply live, which is how the wrong plan gets applied.
#[test]
fn a_rule_that_cannot_plan_renders_the_reason_and_drops_the_stale_plan() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let archive = build_zip_fixture(
        _temp.path(),
        "fixture.zip",
        &[("a.txt", b"one"), ("b.txt", b"two")],
    );
    let good = save_rule(&shared, rule_input("Good Rule", "Organized", "**", ""));
    // Routes every file outside the organized root, which the planner
    // refuses: a plan may not write outside the folder it owns.
    let all = save_rule(
        &shared,
        rule_input("Escaping Rule", "Organized", "**", "../escape"),
    );
    let good_id = good
        .iter()
        .find(|rule| rule.name == "Good Rule")
        .expect("the good rule must exist")
        .id
        .clone();
    let rules: Vec<_> = all
        .into_iter()
        .filter(|rule| rule.name == "Good Rule" || rule.name == "Escaping Rule")
        .collect();
    let escaping_index = rules
        .iter()
        .position(|rule| rule.name == "Escaping Rule")
        .expect("the escaping rule must be listed");
    let good_index = rules
        .iter()
        .position(|rule| rule.id == good_id)
        .expect("the good rule must be listed");

    let session_id = open_session(&shared, &archive);
    let mut panel = panel_for(session_id, "fixture.zip", rules);

    // Start on the rule that works.
    panel.ui_state.selected_rule_index = good_index;
    refresh_preview(&mut panel, &shared);
    assert!(panel.preview().is_some());
    assert!(panel.can_apply(), "a plan on screen is applyable");

    // Switch to the one that cannot plan.
    panel.ui_state.selected_rule_index = escaping_index;
    refresh_preview(&mut panel, &shared);

    let message = panel
        .preview_error()
        .expect("a failed preview must be surfaced");
    assert!(
        message.contains("no plan"),
        "the panel must say the rule produced no plan: {message}"
    );
    assert!(
        panel.preview().is_none(),
        "the previous rule's plan must not stay on screen"
    );
    assert!(
        panel.ui_state.organized_tree.is_empty(),
        "and neither may the tree built from it"
    );
    assert!(
        !panel.can_apply(),
        "Apply must stay disabled until a preview succeeds"
    );

    // Going back to a working rule recovers.
    panel.ui_state.selected_rule_index = good_index;
    refresh_preview(&mut panel, &shared);
    assert!(panel.preview().is_some());
    assert_eq!(panel.preview_error(), None);
    assert!(panel.can_apply(), "and Apply comes back with it");
}

/// Metadata arriving for the session invalidates the plan (the plan is
/// a function of it), and the recomputed plan reflects the new value.
/// Changing the *profile* does not: it decides the output container,
/// not the layout.
#[test]
fn metadata_arrival_invalidates_the_plan_and_a_profile_change_does_not() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let app = shared
        .facade
        .as_ref()
        .expect("the fixture has a facade")
        .clone();
    let archive = build_zip_fixture(
        _temp.path(),
        "[RJ123456] Placeholder.zip",
        &[("wrapper/Game.exe", b"executable bytes")],
    );
    let rules = save_rule(
        &shared,
        rule_input("Titled Rule", "[$product_id] $title", "**", ""),
    );
    let rules: Vec<_> = rules
        .into_iter()
        .filter(|rule| rule.name == "Titled Rule")
        .collect();
    let session_id = open_session(&shared, &archive);
    let mut panel = panel_for(session_id, "[RJ123456] Placeholder.zip", rules);

    refresh_preview(&mut panel, &shared);
    // With no metadata there is nothing to expand the template from, and
    // an output whose *name* cannot be resolved is not produced at all --
    // it is reported as passed over, with the tokens that stayed unset
    // named in the reason.
    let before = panel.preview().expect("a plan");
    assert!(
        before.outputs.is_empty(),
        "an unresolvable name must not produce a folder: {:?}",
        before.outputs
    );
    assert_eq!(before.skipped_outputs.len(), 1);
    assert!(
        before.skipped_outputs[0].reason.contains("product_id")
            || before.skipped_outputs[0].reason.contains("title"),
        "the reason must name what stayed unset: {:?}",
        before.skipped_outputs[0]
    );

    // A profile change must not invalidate the plan: the panel asks for
    // no refresh, so the plan it holds is unchanged.
    panel.ui_state.selected_profile_index = 3;
    assert!(
        !render_once(&mut panel).contains(&OrganizePanelAction::RefreshPreview),
        "a profile change must not ask for a new plan"
    );

    // Metadata arrives.
    report_plugin_metadata(&app, session_id, "RJ123456", "Plugin Title");
    panel.metadata_changed(None);
    assert!(
        render_once(&mut panel).contains(&OrganizePanelAction::RefreshPreview),
        "metadata arriving must ask for a new plan"
    );

    refresh_preview(&mut panel, &shared);
    assert_eq!(
        only_output(panel.preview().expect("a plan")).root_folder,
        "[RJ123456] Plugin Title",
        "the recomputed plan must use the metadata the session now holds"
    );
}

/// Regression: the organizer is one panel bound to one archive session,
/// but plugin metadata arrives per *tab*. A background tab finishing its
/// fetch must not push its metadata into a panel organizing a different
/// archive — that is not a cosmetic mislabel: `panel.metadata` gates
/// `can_apply()` through `is_dlsite_rule_without_metadata()`, so the
/// wrong archive's metadata would enable Apply for a DLsite rule that
/// has none, and it is what the issues export enumerates screenshots
/// from.
#[test]
fn metadata_for_another_session_never_reaches_the_panel() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let app = shared
        .facade
        .as_ref()
        .expect("the fixture has a facade")
        .clone();

    // A rule that needs DLsite metadata: with none, Apply is blocked
    // however good the plan is.
    let mut dlsite_rule = rule_input("DLsite Rule", "[$product_id] $title", "**", "");
    dlsite_rule.trigger.metadata_source = Some("dlsite".to_string());
    let rules = save_rule(&shared, dlsite_rule);
    let rules: Vec<_> = rules
        .into_iter()
        .filter(|rule| rule.name == "DLsite Rule")
        .collect();

    // Two archives, two sessions, two tabs.
    let panel_archive = build_zip_fixture(
        _temp.path(),
        "[RJ123456] Panel.zip",
        &[("wrapper/Game.exe", b"executable bytes")],
    );
    let other_archive = build_zip_fixture(
        _temp.path(),
        "[RJ000222] Other.zip",
        &[("wrapper/Other.exe", b"other bytes")],
    );
    let panel_session = open_session(&shared, &panel_archive);
    let other_session = open_session(&shared, &other_archive);

    let panel_tab = shared.signals().tabs.get().active().clone();
    panel_tab.archive_path.set(Some(panel_archive));
    panel_tab.archive_session_id.set(Some(panel_session));

    let other_tab_id = {
        let mut col = shared.signals().tabs.get();
        let id = col.open(Some(other_archive));
        shared.signals().tabs.set(col);
        id
    };
    let other_tab = shared
        .signals()
        .tabs
        .get()
        .get(other_tab_id)
        .cloned()
        .expect("the second tab must exist");
    other_tab.archive_session_id.set(Some(other_session));

    // The user is now looking at the second tab while the organizer is
    // still open on the first — the arrangement the old "is this the
    // active tab?" test got wrong in both directions.
    {
        let mut col = shared.signals().tabs.get();
        col.switch_to(other_tab_id);
        shared.signals().tabs.set(col);
    }

    // The panel is organizing the *first* session and has no metadata.
    let mut org_feature = arclain_ui::features::organization::OrganizationFeature::new(&shared);
    let mut panel = panel_for(panel_session, "[RJ123456] Panel.zip", rules);
    refresh_preview(&mut panel, &shared);
    assert!(panel.preview().is_some(), "the plan must be on screen");
    assert!(
        !panel.can_apply(),
        "a DLsite rule with no metadata blocks Apply"
    );
    org_feature.organizer_page = Some(arclain_ui::features::organization::OrganizerPage::new(
        panel,
    ));

    // The OTHER tab's plugin fetch lands: the plugin writes it to its own
    // session, and the session-event consumer stamps the owning tab.
    report_plugin_metadata(&app, other_session, "RJ000222", "Other Archive Title");
    other_tab.metadata.set(Some(serde_json::json!({
        "product_id": "RJ000222",
        "source": "dlsite",
        "title": "Other Archive Title",
    })));
    arclain_ui::core::app_lifecycle::process_metadata_signal(&shared, &mut org_feature);

    assert_eq!(
        other_tab
            .game_metadata
            .get()
            .and_then(|meta| meta.title)
            .as_deref(),
        Some("Other Archive Title"),
        "sanity: the event was processed — it reached its own tab"
    );
    let panel = &org_feature
        .organizer_page
        .as_ref()
        .expect("the panel is still open")
        .panel;
    assert!(
        panel.metadata.is_none(),
        "another session's metadata must not reach this panel"
    );
    assert!(
        !panel.can_apply(),
        "and must not unblock Apply for a rule that still has no metadata"
    );

    // The panel's OWN tab's fetch lands.
    report_plugin_metadata(&app, panel_session, "RJ123456", "Panel Archive Title");
    panel_tab.metadata.set(Some(serde_json::json!({
        "product_id": "RJ123456",
        "source": "dlsite",
        "title": "Panel Archive Title",
    })));
    arclain_ui::core::app_lifecycle::process_metadata_signal(&shared, &mut org_feature);

    let page = org_feature
        .organizer_page
        .as_mut()
        .expect("the panel is still open");
    assert_eq!(
        page.panel
            .metadata
            .as_ref()
            .and_then(|meta| meta.title.as_deref()),
        Some("Panel Archive Title"),
        "its own session's metadata must reach it, even while another tab is active"
    );
    assert!(
        !page.panel.can_apply(),
        "the plan is stale until it is recomputed from the new metadata"
    );

    refresh_preview(&mut page.panel, &shared);
    assert_eq!(
        only_output(page.panel.preview().expect("a plan")).root_folder,
        "[RJ123456] Panel Archive Title",
        "and the recomputed plan is this session's, not the other tab's"
    );
    assert!(page.panel.can_apply(), "Apply unblocks once metadata is in");
}

/// Renders one frame headlessly and reports what the panel asked for.
fn render_once(panel: &mut OrganizePanel) -> Vec<OrganizePanelAction> {
    let ctx = eframe::egui::Context::default();
    let theme = arclain_ui::shared::theme::AppTheme::new(false);
    let mut actions = Vec::new();
    let _ = ctx.run(Default::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(action) = panel.render(ui, ctx, &theme) {
                actions.push(action);
            }
        });
    });
    actions
}

/// A plan with several outputs must reach the screen whole. A panel
/// showing the first of three mod folders, silently, describes a run the
/// user is not about to get -- so this drives a real (headless) frame
/// and asserts every root, and every folder the plan passed over, is in
/// what was rendered.
#[test]
fn every_output_and_every_skipped_folder_reaches_the_rendered_frame() {
    use arclain_app::organization::{
        OrganizeIntegrityDto, OrganizePlanPreview, PlannedMoveDto, PlannedOutputDto,
        SkippedOutputDto,
    };
    use egui_kittest::kittest::Queryable as _;

    let output = |name: &str| PlannedOutputDto {
        root_folder: name.to_string(),
        root_folder_template: "$mod_name".to_string(),
        moves: vec![PlannedMoveDto {
            source: format!("pack/{name}/mod.esp"),
            destination: format!("{name}/mod.esp"),
        }],
        generated_files: Vec::new(),
        downloads: Vec::new(),
        resolved_variables: Vec::new(),
        reasoning: vec![format!("all files placed 1 file into {name}")],
    };

    let mut panel = panel_for(ArchiveSessionId::from_raw(1), "pack.zip", Vec::new());
    panel.set_preview(OrganizePlanPreview {
        session_id: ArchiveSessionId::from_raw(1),
        revision: 1,
        rule_id: "1".to_string(),
        rule_name: "Mods".to_string(),
        outputs: vec![output("Red Mod"), output("Blue Mod")],
        skipped_outputs: vec![SkippedOutputDto {
            root: "pack/green".to_string(),
            reason: "$mod_name was not set".to_string(),
        }],
        integrity: OrganizeIntegrityDto {
            original_files: 3,
            original_folders: 3,
            moved_files: 2,
            generated_files: 0,
            expected_screenshots: 0,
            planned_screenshots: 0,
            expected_modified_files: 2,
            file_discrepancy: -1,
            missing_original_files: vec!["pack/green/mod.esp".to_string()],
            original_hash: 1,
            result_hash: 2,
            content_match: false,
        },
    });

    // Both outputs' files are in the one tree, because that is what the
    // result looks like on disk: three siblings, not one folder.
    let roots: Vec<&str> = panel
        .ui_state
        .organized_tree
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(roots, vec!["Blue Mod", "Red Mod"]);

    // A window-sized surface, because that is what the panel occupies.
    // Its dual-pane view sizes itself from the space it is given and
    // asks egui for `available.y - 40.0`, which egui rejects outright
    // below 40 logical points.
    let mut harness = egui_kittest::Harness::builder()
        .with_size(eframe::egui::vec2(900.0, 700.0))
        .build_ui_state(
            |ui, panel: &mut OrganizePanel| {
                let ctx = ui.ctx().clone();
                let theme = arclain_ui::shared::theme::AppTheme::new(false);
                panel.render(ui, &ctx, &theme);
            },
            panel,
        );
    harness.run();

    for expected in [
        // The count, so a truncated list cannot pass by showing one
        // output and calling it the whole plan.
        "2 outputs:",
        "Red Mod",
        "Blue Mod",
        // The folder the plan passed over, and the reason -- the only
        // place a user learns either.
        "skipped pack/green",
        "$mod_name was not set",
    ] {
        assert!(
            harness
                .query_all_by_label_contains(expected)
                .next()
                .is_some(),
            "the rendered frame must contain {expected:?}"
        );
    }
    // The two assertions that actually discriminate a truncated render.
    // Everything above is satisfiable without the loop having run twice:
    // the count comes from `outputs.len()` directly, and both root names
    // also appear in the merged tree in the right-hand pane. These two
    // are counted, and count what the per-output section emits.
    //
    // Two of them, because each covers for the other's blind spot. The
    // `Why` header is skipped when an output has no reasoning, so a
    // layout that produced none would leave that check green against a
    // panel showing one output of three; the summary line is emitted
    // unconditionally. It in turn cannot tell two outputs apart when
    // their tallies match -- which is exactly the case here, and why it
    // is counted rather than merely found.
    assert_eq!(
        harness
            .query_all_by_label_contains("1 moved, 0 generated, 0 fetched")
            .count(),
        2,
        "every output must render its own summary line, not just the first"
    );
    assert_eq!(
        harness.query_all_by_label("Why").count(),
        2,
        "each output's reasoning must be reachable from the frame"
    );
}
