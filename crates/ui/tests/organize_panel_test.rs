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
    OrganizationMoveActionDto, OrganizationRuleActionsDto, OrganizationRuleInput,
    OrganizationRuleSummary, OrganizationRuleTriggerDto,
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

fn rule_input(name: &str, root_folder: &str, pattern: &str, target: &str) -> OrganizationRuleInput {
    OrganizationRuleInput {
        id: None,
        name: name.to_string(),
        priority: 10,
        enabled: true,
        trigger: OrganizationRuleTriggerDto::default(),
        actions: OrganizationRuleActionsDto {
            root_folder: Some(root_folder.to_string()),
            output_name: None,
            move_files: vec![OrganizationMoveActionDto {
                pattern: pattern.to_string(),
                target: target.to_string(),
            }],
            use_standard_layout: false,
        },
    }
}

/// Reports plugin metadata for `session_id` exactly as a plugin's
/// `emit_metadata` host call does.
fn report_plugin_metadata(app: &ArclainApp, session_id: ArchiveSessionId, title: &str) {
    let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: the session exists"));
    bridge.set_session_metadata(
        session_id.into_raw(),
        Some(serde_json::json!({
            "product_id": "RJ123456",
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
    assert_eq!(preview.root_folder, "Organized");
    let destinations: Vec<&str> = preview
        .moves
        .iter()
        .map(|planned| planned.destination.as_str())
        .collect();
    assert_eq!(
        destinations,
        vec!["Organized/Game.exe", "Organized/readme.txt"]
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
    assert_eq!(
        panel.preview().expect("a plan").root_folder,
        "[$product_id] $title",
        "with no metadata there is nothing to expand the template from"
    );

    // A profile change must not invalidate the plan: the panel asks for
    // no refresh, so the plan it holds is unchanged.
    panel.ui_state.selected_profile_index = 3;
    assert!(
        !render_once(&mut panel).contains(&OrganizePanelAction::RefreshPreview),
        "a profile change must not ask for a new plan"
    );

    // Metadata arrives.
    report_plugin_metadata(&app, session_id, "Plugin Title");
    panel.metadata_changed(None);
    assert!(
        render_once(&mut panel).contains(&OrganizePanelAction::RefreshPreview),
        "metadata arriving must ask for a new plan"
    );

    refresh_preview(&mut panel, &shared);
    assert_eq!(
        panel.preview().expect("a plan").root_folder,
        "[RJ123456] Plugin Title",
        "the recomputed plan must use the metadata the session now holds"
    );
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
