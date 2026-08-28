//! Integration tests for the organization facade surface:
//! `ArclainApp::organization_rules`/`upsert_organization_rule`/
//! `delete_organization_rule`, `organization_profiles`/
//! `upsert_organization_profile`/`delete_organization_profile`/
//! `set_default_organization_profile`, and `preview_organize_plan`.
//!
//! `crates/app/src/organization.rs`'s own unit tests cover DTO
//! validation and conversion in isolation (pure functions, no I/O); this
//! file's job is proving those pieces are wired together correctly
//! behind the public API against a real bootstrap -- a real SQLite
//! config database in a temp profile, and for the preview a real ZIP
//! fixture opened through the real `start_open_archive` flow, the same
//! way `archive_sessions.rs`/`settings_facade.rs` already do for their
//! own surfaces.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! following this crate's established convention: `ArclainApp` owns its
//! own Tokio runtime, and dropping it must not happen from inside an
//! async context (see `archive_sessions.rs`'s own module doc comment).

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationResult, OperationState};
use arclain_app::ids::ArchiveSessionId;
use arclain_app::organization::{
    has_dlsite_product_code, FetchSourceDto, FetchedFileDto, GeneratedContentDto, GeneratedFileDto,
    LayoutDto, OrganizationProfileInput, OrganizationProfileSummary, OrganizationRuleActionsDto,
    OrganizationRuleInput, OrganizationRuleSummary, OrganizationRuleTriggerDto, PlacementDto,
    PlacementSourceDto, PlannedOutputDto,
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

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

/// Bootstraps an `ArclainApp` against an isolated temp profile -- see
/// `archive_sessions.rs::bootstrap_app`'s identical doc comment for why
/// the dummy 7-Zip seeding is required even for tests that never touch
/// an archive backend.
fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    let sevenzip = support::create_dummy_executable(temp.path(), sevenzip_exe_name());
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: None,
    })
    .expect("bootstrap must succeed")
}

#[cfg(feature = "gameta")]
#[test]
fn dlsite_product_code_detection_is_available_through_the_facade_vocabulary() {
    assert!(has_dlsite_product_code("[RJ123456] Game Title.zip"));
    assert!(has_dlsite_product_code("vj123456_voice.rar"));
    assert!(has_dlsite_product_code("BJ12345678"));
    assert!(!has_dlsite_product_code("ordinary-archive.zip"));
    assert!(!has_dlsite_product_code("RJ12345.zip"));
}

/// The other half of that contract. The query is facade vocabulary, so a
/// lean build still answers it -- same name, same signature, callable by
/// the same frontend -- but with no detector compiled behind it every
/// name answers "no code", which is precisely how metadata-backed rules
/// come to match nothing instead of failing.
#[cfg(not(feature = "gameta"))]
#[test]
fn dlsite_product_code_detection_answers_absent_without_the_metadata_stack() {
    assert!(!has_dlsite_product_code("[RJ123456] Game Title.zip"));
    assert!(!has_dlsite_product_code("vj123456_voice.rar"));
    assert!(!has_dlsite_product_code("BJ12345678"));
    assert!(!has_dlsite_product_code("ordinary-archive.zip"));
}

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

/// Opens `archive` through the real `start_open_archive` flow and
/// returns the session it produced.
async fn open_session(app: &ArclainApp, archive: &Path) -> ArchiveSessionId {
    let mut events = app.subscribe_operations();
    app.start_open_archive(OpenArchiveRequest {
        source_path: archive.to_path_buf(),
        password: None,
    })
    .await
    .expect("start_open_archive must be accepted");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("an open event must arrive within 10s")
            .expect("the operation channel must not close");
        match event.state {
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            } => return snapshot.session_id,
            OperationState::Failed { error } => panic!("opening the fixture failed: {error:?}"),
            _ => {}
        }
    }
}

fn base_rule_input(name: &str) -> OrganizationRuleInput {
    OrganizationRuleInput {
        id: None,
        name: name.to_string(),
        priority: 50,
        enabled: true,
        trigger: OrganizationRuleTriggerDto::default(),
        actions: OrganizationRuleActionsDto {
            output_name: None,
            layout: LayoutDto {
                name: "Organized".to_string(),
                // Routes the executables into `bin/` and carries
                // everything else along beside them. The trailing `All`
                // is what keeps the archive whole: a placement claims
                // the files it matches and a file nothing claimed is
                // left behind entirely.
                place: vec![
                    PlacementDto {
                        from: PlacementSourceDto::Matching("*.exe".to_string()),
                        into: "bin".to_string(),
                    },
                    PlacementDto {
                        from: PlacementSourceDto::All,
                        into: String::new(),
                    },
                ],
                ..LayoutDto::default()
            },
        },
    }
}

/// The arrangement a rule used to ask for with `use_standard_layout`:
/// detect the payload folder, rehome it under `Game/`, write the
/// metadata document beside it and fetch the screenshots.
fn standard_layout() -> LayoutDto {
    LayoutDto {
        name: "Organized".to_string(),
        place: vec![PlacementDto {
            from: PlacementSourceDto::ContentRoot,
            into: "Game".to_string(),
        }],
        generate: vec![GeneratedFileDto {
            into: "metadata.json".to_string(),
            content: GeneratedContentDto::MetadataDocument,
        }],
        fetch: vec![FetchedFileDto {
            into: "screenshots".to_string(),
            source: FetchSourceDto::Screenshots,
            name: "image_$index.$ext".to_string(),
        }],
        ..LayoutDto::default()
    }
}

/// The plan's one output. Every preview in this file describes a
/// whole-input layout, which resolves to exactly one folder; a test
/// about several outputs lives where the resolution does.
fn only_output(preview: &arclain_app::organization::OrganizePlanPreview) -> &PlannedOutputDto {
    assert_eq!(
        preview.outputs.len(),
        1,
        "a whole-input layout resolves to one output"
    );
    &preview.outputs[0]
}

fn base_profile_input(name: &str) -> OrganizationProfileInput {
    OrganizationProfileInput {
        id: None,
        name: name.to_string(),
        description: Some("test profile".to_string()),
        output_format: "7z".to_string(),
        compression_level: 5,
        compression_method: Some("LZMA2".to_string()),
        solid_archive: true,
        encrypt_headers: false,
        is_default: false,
    }
}

fn find_rule<'a>(
    rules: &'a [OrganizationRuleSummary],
    name: &str,
) -> Option<&'a OrganizationRuleSummary> {
    rules.iter().find(|rule| rule.name == name)
}

fn find_profile<'a>(
    profiles: &'a [OrganizationProfileSummary],
    name: &str,
) -> Option<&'a OrganizationProfileSummary> {
    profiles.iter().find(|profile| profile.name == name)
}

// ============================================================================
// Rule CRUD.
// ============================================================================

#[test]
fn organization_rules_lists_the_rules_bootstrap_seeds() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let rules = runtime
        .block_on(app.organization_rules())
        .expect("organization_rules must succeed");

    assert!(
        !rules.is_empty(),
        "a fresh bootstrap seeds at least one organization rule"
    );
    for rule in &rules {
        assert!(!rule.id.is_empty());
        assert!(!rule.name.is_empty());
        assert!(
            rule.id.parse::<i64>().is_ok(),
            "a rule id must be usable as OrganizeRequest::rule_id"
        );
    }
}

#[test]
fn a_rule_round_trips_through_create_update_and_delete() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut input = base_rule_input("Round Trip");
    input.trigger.filename_pattern = Some(r"RJ\d+".to_string());

    let after_create = runtime
        .block_on(app.upsert_organization_rule(input.clone()))
        .expect("creating a rule must succeed");
    let created = find_rule(&after_create, "Round Trip").expect("the new rule must be listed");
    let created_id = created.id.clone();

    assert_eq!(created.priority, 50);
    assert!(created.enabled);
    assert_eq!(created.trigger.filename_pattern.as_deref(), Some(r"RJ\d+"));
    assert_eq!(created.actions.layout.name, "Organized");
    assert_eq!(created.actions.layout.place.len(), 2);
    assert_eq!(created.actions.layout.place[0].into, "bin");

    // Update by id: every field the editor can change survives.
    let mut update = input.clone();
    update.id = Some(created_id.clone());
    update.name = "Round Trip Renamed".to_string();
    update.priority = 5;
    update.enabled = false;
    update.actions.layout = standard_layout();

    let after_update = runtime
        .block_on(app.upsert_organization_rule(update))
        .expect("updating a rule must succeed");
    let updated =
        find_rule(&after_update, "Round Trip Renamed").expect("the renamed rule must be listed");

    assert_eq!(updated.id, created_id, "an update must not mint a new id");
    assert_eq!(updated.priority, 5);
    assert!(!updated.enabled);
    assert_eq!(
        updated.actions.layout,
        standard_layout(),
        "the whole layout survives an update, enums and all"
    );
    assert!(find_rule(&after_update, "Round Trip").is_none());

    let after_delete = runtime
        .block_on(app.delete_organization_rule(created_id.clone()))
        .expect("deleting a rule must succeed");
    assert!(find_rule(&after_delete, "Round Trip Renamed").is_none());

    // The returned list is the real post-delete state, not a stale copy.
    let relisted = runtime
        .block_on(app.organization_rules())
        .expect("organization_rules must succeed");
    assert_eq!(relisted.len(), after_delete.len());
}

/// `save_domain_rule` looks a rule up by name when no id is supplied, so
/// an id-less save under an existing name updates that rule rather than
/// creating a second one with the same name -- the same name-keyed
/// upsert `upsert_password_rule` performs.
#[test]
fn an_id_less_save_under_an_existing_name_updates_that_rule() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let created = runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Name Keyed")))
        .expect("creating a rule must succeed");
    let created_id = find_rule(&created, "Name Keyed")
        .expect("the new rule must be listed")
        .id
        .clone();

    let mut second = base_rule_input("Name Keyed");
    second.priority = 99;
    let after_second = runtime
        .block_on(app.upsert_organization_rule(second))
        .expect("a second id-less save must succeed");

    let matching: Vec<_> = after_second
        .iter()
        .filter(|rule| rule.name == "Name Keyed")
        .collect();
    assert_eq!(matching.len(), 1, "no duplicate rule may be created");
    assert_eq!(matching[0].id, created_id);
    assert_eq!(matching[0].priority, 99);
}

/// A positive id naming no row makes the underlying `UPDATE` affect
/// nothing and still report success, which would silently discard the
/// caller's edit.
#[test]
fn updating_a_rule_that_does_not_exist_is_not_a_silent_no_op() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut input = base_rule_input("Ghost");
    input.id = Some("999999".to_string());
    let error = runtime
        .block_on(app.upsert_organization_rule(input))
        .expect_err("an unknown rule id must fail");

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    assert_eq!(error.field.as_deref(), Some("rule_id"));

    let rules = runtime
        .block_on(app.organization_rules())
        .expect("organization_rules must succeed");
    assert!(find_rule(&rules, "Ghost").is_none());
}

#[test]
fn deleting_an_unknown_rule_reports_not_found() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.delete_organization_rule("999999".to_string()))
        .expect_err("an unknown rule id must fail");
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

#[test]
fn a_rule_id_that_is_not_a_positive_integer_is_rejected() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    for bad in ["dlsite-standard", "0", "-3"] {
        let error = runtime
            .block_on(app.delete_organization_rule(bad.to_string()))
            .expect_err("a non-positive-integer id must fail");
        assert_eq!(
            error.kind,
            ApplicationErrorKind::InvalidInput,
            "id {bad:?} must be rejected structurally"
        );
    }
}

/// `organization_rules.name` is `UNIQUE`, so renaming a rule onto
/// another rule's name violates the constraint. That must read as a
/// `Conflict` naming the field to change, not as a retryable storage
/// failure.
#[test]
fn renaming_a_rule_onto_another_rules_name_reports_a_conflict() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Occupied")))
        .expect("creating the first rule must succeed");
    let created = runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Renamable")))
        .expect("creating the second rule must succeed");
    let renamable_id = find_rule(&created, "Renamable")
        .expect("the second rule must be listed")
        .id
        .clone();

    let mut rename = base_rule_input("Occupied");
    rename.id = Some(renamable_id.clone());
    let error = runtime
        .block_on(app.upsert_organization_rule(rename))
        .expect_err("renaming onto a taken name must fail");

    assert_eq!(error.kind, ApplicationErrorKind::Conflict);
    assert_eq!(error.field.as_deref(), Some("name"));
    assert!(!error.retryable, "a name collision is not retryable");

    let rules = runtime
        .block_on(app.organization_rules())
        .expect("organization_rules must succeed");
    assert_eq!(
        find_rule(&rules, "Renamable").map(|rule| rule.id.clone()),
        Some(renamable_id),
        "the rejected rename left the rule untouched"
    );
}

#[test]
fn an_uncompilable_trigger_pattern_never_reaches_storage() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut input = base_rule_input("Broken Pattern");
    input.trigger.filename_pattern = Some("[unterminated".to_string());
    let error = runtime
        .block_on(app.upsert_organization_rule(input))
        .expect_err("an uncompilable pattern must fail");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    let rules = runtime
        .block_on(app.organization_rules())
        .expect("organization_rules must succeed");
    assert!(find_rule(&rules, "Broken Pattern").is_none());
}

// ============================================================================
// Profile CRUD.
// ============================================================================

#[test]
fn organization_profiles_lists_seeded_system_defaults() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let profiles = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");

    assert!(
        !profiles.is_empty(),
        "a fresh bootstrap seeds default archive profiles"
    );
    for profile in &profiles {
        assert!(!profile.id.is_empty());
        assert!(!profile.name.is_empty());
        assert!(!profile.output_format.is_empty());
        assert!(
            profile.is_system,
            "every bootstrap-seeded profile is a system profile"
        );
    }
    assert_eq!(
        profiles.iter().filter(|profile| profile.is_default).count(),
        1,
        "exactly one seeded profile is the default"
    );
}

#[test]
fn a_profile_round_trips_through_create_update_and_delete() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let after_create = runtime
        .block_on(app.upsert_organization_profile(base_profile_input("Custom")))
        .expect("creating a profile must succeed");
    let created = find_profile(&after_create, "Custom").expect("the new profile must be listed");
    let created_id = created.id.clone();

    assert_eq!(created.output_format, "7z");
    assert_eq!(created.compression_level, 5);
    assert_eq!(created.compression_method.as_deref(), Some("LZMA2"));
    assert!(created.solid_archive);
    assert!(!created.encrypt_headers);
    assert!(!created.is_default);
    assert!(
        !created.is_system,
        "a profile created through the facade is never a system profile"
    );

    let mut update = base_profile_input("Custom Renamed");
    update.id = Some(created_id.clone());
    update.output_format = "zip".to_string();
    update.compression_method = Some("Deflate".to_string());
    update.compression_level = 9;
    update.solid_archive = false;
    update.encrypt_headers = true;

    let after_update = runtime
        .block_on(app.upsert_organization_profile(update))
        .expect("updating a profile must succeed");
    let updated =
        find_profile(&after_update, "Custom Renamed").expect("the renamed profile must be listed");

    assert_eq!(updated.id, created_id, "an update must not mint a new id");
    assert_eq!(updated.output_format, "zip");
    assert_eq!(updated.compression_method.as_deref(), Some("Deflate"));
    assert_eq!(updated.compression_level, 9);
    assert!(!updated.solid_archive);
    assert!(updated.encrypt_headers);

    let after_delete = runtime
        .block_on(app.delete_organization_profile(created_id))
        .expect("deleting a non-system profile must succeed");
    assert!(find_profile(&after_delete, "Custom Renamed").is_none());
}

#[test]
fn an_unknown_profile_id_reports_not_found_on_every_mutation() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut update = base_profile_input("Ghost");
    update.id = Some("999999".to_string());
    for error in [
        runtime
            .block_on(app.upsert_organization_profile(update))
            .expect_err("upsert with an unknown id must fail"),
        runtime
            .block_on(app.delete_organization_profile("999999".to_string()))
            .expect_err("delete with an unknown id must fail"),
        runtime
            .block_on(app.set_default_organization_profile("999999".to_string()))
            .expect_err("set-default with an unknown id must fail"),
    ] {
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
        assert_eq!(error.field.as_deref(), Some("profile_id"));
    }
}

/// `set_default_profile` clears every default *before* setting the named
/// one, so an unvalidated unknown id would leave the configuration with
/// no default at all. The `NotFound` above is what prevents that; this
/// pins the state it protects.
#[test]
fn a_rejected_set_default_leaves_the_existing_default_intact() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let before = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    let default_before = before
        .iter()
        .find(|profile| profile.is_default)
        .expect("a seeded default exists")
        .id
        .clone();

    runtime
        .block_on(app.set_default_organization_profile("999999".to_string()))
        .expect_err("an unknown id must fail");

    let after = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    let defaults: Vec<_> = after.iter().filter(|profile| profile.is_default).collect();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].id, default_before);
}

#[test]
fn setting_a_default_moves_the_flag_off_every_other_profile() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let created = runtime
        .block_on(app.upsert_organization_profile(base_profile_input("New Default")))
        .expect("creating a profile must succeed");
    let created_id = find_profile(&created, "New Default")
        .expect("the new profile must be listed")
        .id
        .clone();

    let after = runtime
        .block_on(app.set_default_organization_profile(created_id.clone()))
        .expect("set-default must succeed");

    let defaults: Vec<_> = after.iter().filter(|profile| profile.is_default).collect();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].id, created_id);
}

/// Saving a profile with `is_default` set is a second path to the same
/// invariant, and it must not leave two defaults behind.
#[test]
fn saving_a_profile_as_default_clears_the_previous_default() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut input = base_profile_input("Default By Save");
    input.is_default = true;
    let after = runtime
        .block_on(app.upsert_organization_profile(input))
        .expect("creating a default profile must succeed");

    let defaults: Vec<_> = after.iter().filter(|profile| profile.is_default).collect();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].name, "Default By Save");
}

// ── The delete semantics this facade preserves rather than invents ──────

/// Pins `arclain_db::delete_profile`'s existing behavior as reached
/// through the facade: the delete filters on `is_system = false`, so a
/// system profile is silently immune. The call still reports success --
/// but the list it returns is the real post-delete state, so a caller
/// that renders it sees the profile survived.
#[test]
fn deleting_a_system_profile_succeeds_and_leaves_the_profile_in_place() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let before = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    let system = before
        .iter()
        .find(|profile| profile.is_system)
        .expect("bootstrap seeds system profiles")
        .clone();

    let after = runtime
        .block_on(app.delete_organization_profile(system.id.clone()))
        .expect("deleting a system profile reports success");

    let survivor = find_profile(&after, &system.name)
        .expect("a system profile is immune to delete and must still be listed");
    assert_eq!(survivor.id, system.id);
    assert_eq!(after.len(), before.len());
}

/// Pins the other half of the storage layer's delete semantics: nothing
/// promotes a replacement default, and nothing else in the schema
/// references a profile row, so deleting the default simply leaves the
/// configuration with none. Documented on
/// `ArclainApp::delete_organization_profile`; a future change that
/// starts auto-promoting a replacement must update both.
#[test]
fn deleting_the_default_profile_leaves_no_default_behind() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    // The seeded defaults are all system profiles (immune to delete), so
    // the deletable default has to be one this test creates.
    let mut input = base_profile_input("Doomed Default");
    input.is_default = true;
    let created = runtime
        .block_on(app.upsert_organization_profile(input))
        .expect("creating a default profile must succeed");
    let created_id = find_profile(&created, "Doomed Default")
        .expect("the new profile must be listed")
        .id
        .clone();
    assert_eq!(
        created.iter().filter(|profile| profile.is_default).count(),
        1
    );

    let after = runtime
        .block_on(app.delete_organization_profile(created_id))
        .expect("deleting the default profile must succeed");

    assert!(find_profile(&after, "Doomed Default").is_none());
    assert!(
        !after.is_empty(),
        "the other profiles are untouched by the delete"
    );
    assert_eq!(
        after.iter().filter(|profile| profile.is_default).count(),
        0,
        "no profile is promoted to replace the deleted default"
    );
}

/// The flag is not on `OrganizationProfileInput` at all, so an update
/// cannot clear it -- which is what keeps a system profile undeletable
/// through the facade. Pins that the update path carries the stored
/// value over rather than defaulting it to `false`.
#[test]
fn updating_a_system_profile_cannot_clear_its_system_flag() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let before = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    let system = before
        .iter()
        .find(|profile| profile.is_system)
        .expect("bootstrap seeds system profiles")
        .clone();

    let mut update = base_profile_input(&system.name);
    update.id = Some(system.id.clone());
    let after = runtime
        .block_on(app.upsert_organization_profile(update))
        .expect("updating a system profile must succeed");

    let updated = find_profile(&after, &system.name).expect("the profile must still be listed");
    assert!(
        updated.is_system,
        "an update must carry the stored is_system flag over"
    );
}

/// `archive_profiles.name` is `UNIQUE` and a profile has no name-keyed
/// upsert fallback, so a create under a taken name is always a
/// collision. The bootstrap-seeded profiles give a guaranteed-taken name
/// to collide with.
#[test]
fn creating_a_profile_under_a_taken_name_reports_a_conflict() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let seeded = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    let taken = seeded[0].name.clone();

    let error = runtime
        .block_on(app.upsert_organization_profile(base_profile_input(&taken)))
        .expect_err("creating under a taken name must fail");

    assert_eq!(error.kind, ApplicationErrorKind::Conflict);
    assert_eq!(error.field.as_deref(), Some("name"));
    assert!(!error.retryable, "a name collision is not retryable");

    let after = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    assert_eq!(after.len(), seeded.len(), "nothing was created");
}

/// Re-saving a profile without changing its name is not a collision with
/// itself.
#[test]
fn updating_a_profile_under_its_own_name_is_not_a_conflict() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let created = runtime
        .block_on(app.upsert_organization_profile(base_profile_input("Stable Name")))
        .expect("creating a profile must succeed");
    let id = find_profile(&created, "Stable Name")
        .expect("the new profile must be listed")
        .id
        .clone();

    let mut again = base_profile_input("Stable Name");
    again.id = Some(id.clone());
    again.compression_level = 1;
    let after = runtime
        .block_on(app.upsert_organization_profile(again))
        .expect("re-saving under its own name must succeed");

    let updated = find_profile(&after, "Stable Name").expect("the profile must still be listed");
    assert_eq!(updated.id, id);
    assert_eq!(updated.compression_level, 1);
}

#[test]
fn an_unsupported_output_format_never_reaches_storage() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let mut input = base_profile_input("Bad Format");
    input.output_format = "rar".to_string();
    let error = runtime
        .block_on(app.upsert_organization_profile(input))
        .expect_err("an unsupported format must fail");
    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("output_format"));

    let profiles = runtime
        .block_on(app.organization_profiles())
        .expect("organization_profiles must succeed");
    assert!(find_profile(&profiles, "Bad Format").is_none());
}

// ============================================================================
// Preview.
// ============================================================================

#[test]
fn preview_plans_every_file_in_the_open_archive() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(
        temp.path(),
        "sample.zip",
        &[
            ("wrapper/Game.exe", b"executable bytes"),
            ("wrapper/data/pack.bin", b"data bytes"),
            ("wrapper/readme.txt", b"read me"),
        ],
    );

    let created = runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Preview Rule")))
        .expect("creating a rule must succeed");
    let rule_id = find_rule(&created, "Preview Rule")
        .expect("the new rule must be listed")
        .id
        .clone();

    let (session_id, preview) = runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        let preview = app
            .preview_organize_plan(session_id, rule_id.clone())
            .await
            .expect("preview must succeed");
        (session_id, preview)
    });

    assert_eq!(preview.session_id, session_id);
    assert_eq!(preview.rule_id, rule_id);
    assert_eq!(preview.rule_name, "Preview Rule");
    let output = only_output(&preview);
    assert_eq!(output.root_folder, "Organized");
    assert_eq!(output.root_folder_template, "Organized");
    assert!(
        preview.skipped_outputs.is_empty(),
        "nothing was passed over"
    );
    assert_eq!(
        preview.revision, 1,
        "a freshly opened session is revision 1"
    );

    // Every file is planned; the `*.exe` placement routes one of them
    // into `bin/`, the trailing `All` carries the rest. A placement
    // strips only what its own glob spells out, so the wrapper
    // directory travels with the files under it.
    let destinations: Vec<&str> = output
        .moves
        .iter()
        .map(|planned| planned.destination.as_str())
        .collect();
    assert_eq!(
        destinations,
        vec![
            "Organized/bin/wrapper/Game.exe",
            "Organized/wrapper/data/pack.bin",
            "Organized/wrapper/readme.txt",
        ]
    );
    let sources: Vec<&str> = output
        .moves
        .iter()
        .map(|planned| planned.source.as_str())
        .collect();
    assert_eq!(
        sources,
        vec![
            "wrapper/Game.exe",
            "wrapper/data/pack.bin",
            "wrapper/readme.txt",
        ]
    );

    // Integrity: every original file is accounted for by a planned move.
    assert_eq!(preview.integrity.original_files, 3);
    assert_eq!(preview.integrity.moved_files, 3);
    assert!(preview.integrity.missing_original_files.is_empty());
    assert!(preview.integrity.content_match);
    assert_eq!(
        preview.integrity.original_hash,
        preview.integrity.result_hash
    );
    assert_eq!(preview.integrity.expected_modified_files, 3);

    // Every placement says what it did, whether or not there was
    // anything to infer -- a preview that describes an outcome without
    // saying how it was reached is one a user can only trust or
    // distrust.
    assert_eq!(
        output.reasoning.len(),
        2,
        "one line per placement: {:?}",
        output.reasoning
    );

    // No plugin metadata has been reported for this session, so nothing
    // is generated and nothing is scheduled for download.
    assert!(output.generated_files.is_empty());
    assert!(output.downloads.is_empty());
    assert_eq!(preview.integrity.generated_files, 0);
    assert_eq!(preview.integrity.expected_screenshots, 0);
    assert_eq!(preview.integrity.planned_screenshots, 0);

    // The archive lists no directory entries of its own; the two
    // ancestors the entry index synthesizes are what the folder count
    // reports, matching the rest of this facade's read model.
    assert_eq!(preview.integrity.original_folders, 2);
}

/// Files a plan does not relocate must come back named, not merely
/// counted: that list is what the panel's integrity strip and its issue
/// export are built on. Standard layout produces exactly this case --
/// it rehomes the detected game-content root and drops everything
/// outside it -- and so does a 0-byte file, which the rule engine prunes
/// before planning but which the integrity report still counts as an
/// original.
#[test]
fn preview_names_the_files_no_move_covers() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(
        temp.path(),
        "standard.zip",
        &[
            ("inner/Game.exe", b"executable bytes"),
            ("inner/data.bin", b"data bytes"),
            ("inner/empty.log", b""),
            ("outside/readme.txt", b"read me"),
        ],
    );

    let mut input = base_rule_input("Standard Layout");
    input.actions.layout = standard_layout();
    let created = runtime
        .block_on(app.upsert_organization_rule(input))
        .expect("creating a rule must succeed");
    let rule_id = find_rule(&created, "Standard Layout")
        .expect("the new rule must be listed")
        .id
        .clone();

    let preview = runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        app.preview_organize_plan(session_id, rule_id)
            .await
            .expect("preview must succeed")
    });

    let output = only_output(&preview);
    assert_eq!(
        output
            .moves
            .iter()
            .map(|planned| planned.destination.as_str())
            .collect::<Vec<_>>(),
        vec!["Organized/Game/Game.exe", "Organized/Game/data.bin"],
        "only the detected content root is rehomed"
    );
    // The same fact the missing list carries, said where a user reading
    // the plan will see it: which folder was taken as the payload, and
    // which file that decision left behind.
    assert!(
        output.reasoning.iter().any(|line| line.contains("inner")),
        "the reasoning must name the folder it took as the payload: {:?}",
        output.reasoning
    );
    assert!(
        output
            .reasoning
            .iter()
            .any(|line| line.contains("outside/readme.txt")),
        "and the file no placement carried: {:?}",
        output.reasoning
    );

    assert_eq!(preview.integrity.original_files, 4);
    assert_eq!(preview.integrity.moved_files, 2);
    assert!(!preview.integrity.content_match);
    assert_ne!(
        preview.integrity.original_hash,
        preview.integrity.result_hash
    );
    assert_eq!(
        preview.integrity.missing_original_files,
        vec![
            "inner/empty.log".to_string(),
            "outside/readme.txt".to_string()
        ],
        "the missing list is sorted, and names both the pruned 0-byte \
         file and the file outside the content root"
    );
}

/// Two previews of the same rule against the same session must be
/// byte-identical: the rule engine flattens a `HashMap`-backed tree and
/// the integrity report diffs a `HashSet`, both of which vary run to
/// run, so the facade sorts every list before handing it out.
#[test]
fn two_previews_of_one_plan_are_identical() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(
        temp.path(),
        "many.zip",
        &[
            ("a/one.bin", b"1"),
            ("a/two.bin", b"22"),
            ("b/three.bin", b"333"),
            ("b/four.bin", b"4444"),
            ("c/five.bin", b"55555"),
        ],
    );

    let created = runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Deterministic")))
        .expect("creating a rule must succeed");
    let rule_id = find_rule(&created, "Deterministic")
        .expect("the new rule must be listed")
        .id
        .clone();

    let (first, second) = runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        let first = app
            .preview_organize_plan(session_id, rule_id.clone())
            .await
            .expect("first preview must succeed");
        let second = app
            .preview_organize_plan(session_id, rule_id)
            .await
            .expect("second preview must succeed");
        (first, second)
    });

    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
}

/// The preview is what tells a panel that the selected rule cannot be
/// applied at all. Pre-facade this surfaced as nothing: the failure was
/// swallowed and the previous rule's plan stayed on screen with Apply
/// still enabled.
#[test]
fn a_rule_that_cannot_produce_a_plan_reports_invalid_input() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(temp.path(), "small.zip", &[("only.txt", b"content")]);

    let mut input = base_rule_input("Escaping Root");
    input.actions.layout.name = "../outside".to_string();
    let created = runtime
        .block_on(app.upsert_organization_rule(input))
        .expect("creating the rule must succeed");
    let rule_id = find_rule(&created, "Escaping Root")
        .expect("the new rule must be listed")
        .id
        .clone();

    let error = runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        app.preview_organize_plan(session_id, rule_id)
            .await
            .expect_err("a root folder escaping the output must be rejected")
    });

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("rule_id"));
}

#[test]
fn previewing_against_an_unknown_session_or_rule_reports_not_found() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(temp.path(), "small.zip", &[("only.txt", b"content")]);

    let created = runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Present")))
        .expect("creating a rule must succeed");
    let rule_id = find_rule(&created, "Present")
        .expect("the new rule must be listed")
        .id
        .clone();

    let (unknown_session, unknown_rule) = runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        let unknown_session = app
            .preview_organize_plan(ArchiveSessionId::from_raw(9_999), rule_id)
            .await
            .expect_err("an unknown session must fail");
        let unknown_rule = app
            .preview_organize_plan(session_id, "999999".to_string())
            .await
            .expect_err("an unknown rule must fail");
        (unknown_session, unknown_rule)
    });

    assert_eq!(unknown_session.kind, ApplicationErrorKind::NotFound);
    assert_eq!(unknown_rule.kind, ApplicationErrorKind::NotFound);
    assert_eq!(unknown_rule.field.as_deref(), Some("rule_id"));
}

/// The preview must be safe to call at UI interaction frequency: it
/// registers no operation, so hammering it leaves nothing behind for a
/// caller to reap, and never blocks a later real operation.
#[test]
fn repeated_previews_register_no_operations() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(temp.path(), "small.zip", &[("only.txt", b"content")]);

    let created = runtime
        .block_on(app.upsert_organization_rule(base_rule_input("Hot Path")))
        .expect("creating a rule must succeed");
    let rule_id = find_rule(&created, "Hot Path")
        .expect("the new rule must be listed")
        .id
        .clone();

    let (before, after) = runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        let before = app
            .recent_operations(64)
            .await
            .expect("recent_operations must succeed")
            .len();
        for _ in 0..25 {
            app.preview_organize_plan(session_id, rule_id.clone())
                .await
                .expect("preview must succeed");
        }
        let after = app
            .recent_operations(64)
            .await
            .expect("recent_operations must succeed")
            .len();
        (before, after)
    });

    assert_eq!(
        before, after,
        "a preview must never appear in the operation registry"
    );
}

// ============================================================================
// Public-DTO constructibility, mirroring `public_contract.rs`'s pattern.
// ============================================================================

#[test]
fn constructs_every_public_organization_dto() {
    use arclain_app::organization::{
        OrganizeIntegrityDto, OrganizePlanPreview, PlannedDownloadDto, PlannedMoveDto,
        ResolvedVariableDto, SkippedOutputDto,
    };

    let rule_input = base_rule_input("dto");
    assert_eq!(rule_input.trigger, OrganizationRuleTriggerDto::default());

    let rule_summary = OrganizationRuleSummary {
        id: "1".to_string(),
        name: "dto".to_string(),
        priority: 0,
        enabled: true,
        trigger: OrganizationRuleTriggerDto {
            metadata_source: Some("dlsite".to_string()),
            filename_pattern: None,
            has_file: None,
        },
        actions: OrganizationRuleActionsDto::default(),
    };
    assert!(rule_summary.enabled);

    let profile_summary = OrganizationProfileSummary {
        id: "1".to_string(),
        name: "dto".to_string(),
        description: None,
        output_format: "7z".to_string(),
        compression_level: 9,
        compression_method: None,
        solid_archive: true,
        encrypt_headers: false,
        is_default: true,
        is_system: true,
    };
    assert_eq!(profile_summary.output_format, "7z");
    assert_eq!(base_profile_input("dto").compression_level, 5);

    let preview = OrganizePlanPreview {
        session_id: ArchiveSessionId::from_raw(1),
        revision: 1,
        rule_id: "1".to_string(),
        rule_name: "dto".to_string(),
        outputs: vec![PlannedOutputDto {
            root_folder: "Out".to_string(),
            root_folder_template: "Out".to_string(),
            moves: vec![PlannedMoveDto {
                source: "a".to_string(),
                destination: "Out/a".to_string(),
            }],
            generated_files: vec!["Out/metadata.json".to_string()],
            downloads: vec![PlannedDownloadDto {
                destination: "Out/0.jpg".to_string(),
                cached: false,
            }],
            resolved_variables: vec![ResolvedVariableDto {
                name: "title".to_string(),
                value: "T".to_string(),
            }],
            reasoning: vec!["all files placed 1 file into Out".to_string()],
        }],
        skipped_outputs: vec![SkippedOutputDto {
            root: "extras".to_string(),
            reason: "$title was not set".to_string(),
        }],
        integrity: OrganizeIntegrityDto {
            original_files: 1,
            original_folders: 0,
            moved_files: 1,
            generated_files: 1,
            expected_screenshots: 1,
            planned_screenshots: 1,
            expected_modified_files: 3,
            file_discrepancy: 0,
            missing_original_files: Vec::new(),
            original_hash: 1,
            result_hash: 1,
            content_match: true,
        },
    };
    assert_eq!(preview.revision, 1);

    // The whole family round-trips through JSON, so a bridge can carry
    // it without a hand-written codec.
    let json = serde_json::to_string(&preview).expect("serialize");
    let decoded: OrganizePlanPreview = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, preview);
}

/// `arclain_app::settings::OrganizationProfileSummary` stays reachable at
/// its historical path even though the type now lives in
/// `arclain_app::organization`.
#[test]
fn the_profile_summary_is_reachable_from_both_module_paths() {
    fn takes_organization(_: arclain_app::organization::OrganizationProfileSummary) {}

    let profile = arclain_app::settings::OrganizationProfileSummary {
        id: "1".to_string(),
        name: "dto".to_string(),
        description: None,
        output_format: "zip".to_string(),
        compression_level: 0,
        compression_method: None,
        solid_archive: false,
        encrypt_headers: false,
        is_default: false,
        is_system: false,
    };
    takes_organization(profile);
}

// ============================================================================
// Which rules apply to one archive.
// ============================================================================

/// What an organize panel preselects from: a trigger that names a file
/// the archive does not contain does not match it, an unconstrained one
/// matches everything, and the answer is core's own trigger matcher --
/// not a list a frontend re-derives by inspecting `trigger` itself.
#[test]
fn matching_rule_ids_report_only_the_rules_whose_trigger_applies() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(
        temp.path(),
        "[RJ123456] Placeholder.zip",
        &[("wrapper/Game.exe", b"executable bytes")],
    );

    runtime.block_on(async {
        // Matches everything (a wholly-default trigger).
        let anything = base_rule_input("Anything");
        app.upsert_organization_rule(anything)
            .await
            .expect("creating a rule must succeed");

        // Matches this archive's name.
        let mut by_name = base_rule_input("By Name");
        by_name.trigger.filename_pattern = Some(r"^\[RJ\d+\]".to_string());
        app.upsert_organization_rule(by_name)
            .await
            .expect("creating a rule must succeed");

        // Names a file this archive does not contain.
        let mut by_missing_file = base_rule_input("By Missing File");
        by_missing_file.trigger.has_file = Some("installer.msi".to_string());
        app.upsert_organization_rule(by_missing_file)
            .await
            .expect("creating a rule must succeed");

        // Names one it does: the only trigger that reads the entry list
        // at all, so this is what proves the list is there when it is
        // needed.
        let mut by_present_file = base_rule_input("By Present File");
        by_present_file.trigger.has_file = Some("Game.exe".to_string());
        let rules = app
            .upsert_organization_rule(by_present_file)
            .await
            .expect("creating a rule must succeed");

        let session_id = open_session(&app, &archive).await;
        let matching = app
            .matching_organization_rule_ids(session_id)
            .await
            .expect("an open session must report its matching rules");

        let id_of = |name: &str| find_rule(&rules, name).expect("rule must exist").id.clone();
        assert!(matching.contains(&id_of("Anything")));
        assert!(matching.contains(&id_of("By Name")));
        assert!(
            matching.contains(&id_of("By Present File")),
            "a trigger naming a file the archive holds must match"
        );
        assert!(
            !matching.contains(&id_of("By Missing File")),
            "a trigger naming an absent file must not match"
        );

        // Same order the rule list itself uses, so a panel picking "the
        // first matching rule" picks the first one it displays.
        let listed: Vec<String> = app
            .organization_rules()
            .await
            .unwrap()
            .into_iter()
            .map(|rule| rule.id)
            .filter(|id| matching.contains(id))
            .collect();
        assert_eq!(matching, listed);
    });
}

/// A trigger that requires metadata from a source the session has not
/// reported does not match -- and starts matching once a plugin reports
/// it, which is exactly why a panel re-runs this when metadata arrives.
#[test]
fn a_metadata_source_trigger_matches_only_once_that_metadata_arrives() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(
        temp.path(),
        "[RJ123456] Placeholder.zip",
        &[("wrapper/Game.exe", b"executable bytes")],
    );

    runtime.block_on(async {
        let mut dlsite_only = base_rule_input("DLsite Only");
        dlsite_only.trigger.metadata_source = Some("dlsite".to_string());
        let rules = app
            .upsert_organization_rule(dlsite_only)
            .await
            .expect("creating a rule must succeed");
        let rule_id = find_rule(&rules, "DLsite Only")
            .expect("the new rule must be listed")
            .id
            .clone();

        let session_id = open_session(&app, &archive).await;
        assert!(
            !app.matching_organization_rule_ids(session_id)
                .await
                .unwrap()
                .contains(&rule_id),
            "a rule requiring metadata cannot match a session that has none"
        );

        let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: the session exists"));
        bridge.set_session_metadata(
            session_id.into_raw(),
            Some(serde_json::json!({
                "product_id": "RJ123456",
                "source": "dlsite",
                "title": "Placeholder Title",
            })),
        );

        assert!(
            app.matching_organization_rule_ids(session_id)
                .await
                .unwrap()
                .contains(&rule_id),
            "the same rule must start matching once the session reports dlsite metadata"
        );
    });
}

#[test]
fn matching_rule_ids_reject_an_unknown_session_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.matching_organization_rule_ids(ArchiveSessionId::from_raw(999_999)))
        .unwrap_err();
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}
