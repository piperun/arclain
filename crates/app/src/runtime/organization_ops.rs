//! `AppRuntime`-touching execution logic for the organization facade
//! surface: archive-profile CRUD, organization-rule CRUD, and the
//! synchronous plan preview.
//!
//! `crate::organization` holds the DTOs and the pure validation/
//! conversion logic this module calls into; `crate::runtime`'s own
//! `impl ArclainApp` exposes the thin dispatch wrappers -- the same
//! layering `crate::settings`/`runtime::settings_ops` uses.
//!
//! ## Where the rows live
//!
//! Rules and profiles are two tables in the same config database, both
//! reached through the one pooled handle `ConfigDbs::config_pool`
//! (rules additionally via `arclain_core::services::OrganizationService`,
//! which wraps that same pool). Nothing here opens its own connection:
//! a single pooled path means a write and the re-list that follows it
//! cannot contend with each other across two independent connections to
//! the same file.
//!
//! ## Serializing mutations
//!
//! Every mutating function here takes `AppRuntime::settings_write_lock`
//! for its whole read-validate-write-relist sequence, for the same
//! reason `runtime::settings_ops` does -- and for one specific to this
//! surface: both `set_default_profile` and a `save_profile` that sets
//! `is_default` are *two* statements ("clear every default", "set this
//! one"), so two concurrent callers could otherwise interleave into
//! zero defaults or two. `save_domain_rule`'s "look up by name, then
//! insert or update" is the same read-modify-write shape. SQLite
//! serializes individual statements, not these pairs; this lock does.
//! Read-only functions (`run_organization_rules`,
//! `run_organization_profiles`, `run_preview_organize_plan`) never take
//! it.
//!
//! ## The preview never becomes an operation
//!
//! `run_preview_organize_plan` deliberately does not touch
//! `OperationRegistry`: it mints no `OperationId`, broadcasts no
//! `OperationEvent`, and spawns nothing that outlives the call. Its one
//! `spawn_blocking` exists purely to keep CPU-bound planning off an
//! async worker thread and is awaited before the function returns --
//! see `crate::runtime::ArclainApp::preview_organize_plan`'s own doc
//! comment for why that distinction is load-bearing.

use std::sync::Arc;

use arclain_core::features::organization::engine::RuleEngine;
use arclain_core::features::organization::metrics::IntegrityReport;
use arclain_core::features::organization::{ArchiveProfile, GameMetadata};
use arclain_core::services::OrganizationService;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::ids::ArchiveSessionId;
use crate::organization::{
    self, OrganizationProfileInput, OrganizationProfileSummary, OrganizationRuleInput,
    OrganizationRuleSummary, OrganizeIntegrityDto, OrganizePlanPreview, PlannedDownloadDto,
    PlannedMoveDto, ResolvedVariableDto,
};

use super::AppRuntime;

// ============================================================================
// Shared resolution helpers.
// ============================================================================

/// The opened config/secrets database handle, or `None` when this
/// instance never opened one. Cloning is cheap (pools and database
/// handles behind `Arc`s), and the `RwLock` guard is released before
/// this returns -- nothing here ever holds it across an `.await`.
fn config_dbs(inner: &Arc<AppRuntime>) -> Option<arclain_core::ConfigDbs> {
    inner.session.mutable.read().dbs.clone()
}

fn require_config_dbs(
    inner: &Arc<AppRuntime>,
) -> Result<arclain_core::ConfigDbs, ApplicationError> {
    config_dbs(inner).ok_or_else(organization_unavailable_error)
}

fn require_organization_service(
    inner: &Arc<AppRuntime>,
) -> Result<Arc<OrganizationService>, ApplicationError> {
    inner
        .core_services()
        .organization_service
        .clone()
        .ok_or_else(organization_unavailable_error)
}

/// This application's own runtime handle -- never the caller's ambient
/// one, per the crate's runtime rules. `None` only once the runtime has
/// actually been torn down mid-request.
fn handle_for(inner: &Arc<AppRuntime>) -> Result<tokio::runtime::Handle, ApplicationError> {
    inner.tokio_handle().ok_or_else(shutdown_mid_request_error)
}

/// Parses a caller-supplied row id and rejects anything that is not a
/// real, positive row id.
///
/// Zero and negatives are rejected rather than passed through: the
/// underlying `save_domain_rule` treats a non-positive id as "no id
/// supplied" and silently falls back to a name lookup, so a caller who
/// meant to update row `-5` would instead create or retarget some other
/// rule entirely.
fn parse_row_id(value: &str, field: &'static str) -> Result<i64, ApplicationError> {
    let id = organization::parse_id(value, field)?;
    if id <= 0 {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "id must be a positive row id",
        )
        .with_diagnostic(format!("field {field:?}: got {id}"))
        .with_recoverability(Recoverability::UserAction)
        .with_field(field));
    }
    Ok(id)
}

// ============================================================================
// Rules.
// ============================================================================

/// Every saved organization rule. Empty (not an error) when no
/// organization service is configured, matching
/// [`run_organization_profiles`]'s identical treatment of a missing
/// config database.
pub(super) async fn run_organization_rules(
    inner: &Arc<AppRuntime>,
) -> Result<Vec<OrganizationRuleSummary>, ApplicationError> {
    let Some(service) = inner.core_services().organization_service.clone() else {
        return Ok(Vec::new());
    };
    list_rules(inner, service).await
}

/// Every rule as the core domain type, which carries the id and name the
/// pre-write checks need. One query; [`list_rules`] maps the same result
/// into DTOs.
async fn list_core_rules(
    inner: &Arc<AppRuntime>,
    service: Arc<OrganizationService>,
) -> Result<Vec<arclain_core::features::organization::OrganizationRule>, ApplicationError> {
    let handle = handle_for(inner)?;
    handle
        .spawn_blocking(move || service.list_domain_rules())
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("listing organization rules", error))
}

async fn list_rules(
    inner: &Arc<AppRuntime>,
    service: Arc<OrganizationService>,
) -> Result<Vec<OrganizationRuleSummary>, ApplicationError> {
    Ok(list_core_rules(inner, service)
        .await?
        .iter()
        .map(organization::summarize_rule)
        .collect())
}

pub(super) async fn run_upsert_organization_rule(
    inner: &Arc<AppRuntime>,
    input: OrganizationRuleInput,
) -> Result<Vec<OrganizationRuleSummary>, ApplicationError> {
    // Structural validation first: a malformed input never reaches the
    // write lock, let alone the database.
    let id = match input.id.as_deref() {
        Some(value) => parse_row_id(value, "id")?,
        None => 0,
    };
    let rule = organization::rule_to_core(&input, id)?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let service = require_organization_service(inner)?;
    let existing = list_core_rules(inner, service.clone()).await?;

    if id > 0 {
        // `save_domain_rule` issues a plain `UPDATE ... WHERE id = ?`
        // for a positive id: an id naming no row updates nothing and
        // still reports success. Confirm the row first so a stale id
        // fails loudly instead of silently discarding the edit.
        if !existing.iter().any(|candidate| candidate.id == id) {
            return Err(rule_not_found_error(id));
        }
        // `organization_rules.name` is `UNIQUE`, so renaming onto
        // another rule's name is a constraint violation. Catching it
        // here reports a `Conflict` naming the field the caller must
        // change; letting it reach SQLite would instead surface as an
        // opaque, and falsely retryable, storage failure. Only an update
        // needs this: an id-less save under an existing name is the
        // documented name-keyed upsert, not a collision.
        if existing
            .iter()
            .any(|candidate| candidate.id != id && candidate.name == rule.name)
        {
            return Err(duplicate_name_error("organization rule", &rule.name));
        }
    }

    let save_service = service.clone();
    handle_for(inner)?
        .spawn_blocking(move || save_service.save_domain_rule(&rule))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("saving organization rule", error))?;

    list_rules(inner, service).await
}

/// Deletes the rule with `rule_id`.
///
/// `NotFound` when no rule has that id. A *system* rule is a documented
/// no-op: `arclain_db::delete_rule` filters on `is_system = false`, so
/// the delete affects no rows and reports success, and the returned list
/// still contains the rule. Nothing this facade exposes can create a
/// system rule (`save_domain_rule` always writes `is_system = false`),
/// so that branch is unreachable through the facade today -- it is
/// documented because the *storage* rule is real, not because this
/// surface can reach it.
pub(super) async fn run_delete_organization_rule(
    inner: &Arc<AppRuntime>,
    rule_id: String,
) -> Result<Vec<OrganizationRuleSummary>, ApplicationError> {
    let id = parse_row_id(&rule_id, "rule_id")?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let service = require_organization_service(inner)?;
    let existing = list_core_rules(inner, service.clone()).await?;
    if !existing.iter().any(|candidate| candidate.id == id) {
        return Err(rule_not_found_error(id));
    }

    let delete_service = service.clone();
    handle_for(inner)?
        .spawn_blocking(move || delete_service.delete_domain_rule(id))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("deleting organization rule", error))?;

    list_rules(inner, service).await
}

// ============================================================================
// Profiles.
// ============================================================================

/// Every configured archive-output profile. Empty (not an error) when
/// the config database never opened.
pub(super) async fn run_organization_profiles(
    inner: &Arc<AppRuntime>,
) -> Result<Vec<OrganizationProfileSummary>, ApplicationError> {
    let Some(dbs) = config_dbs(inner) else {
        return Ok(Vec::new());
    };
    list_profiles(inner, &dbs).await
}

/// Every profile as the core domain type, which carries the id, name and
/// `is_system` flag the pre-write checks need. One query; [`list_profiles`]
/// maps the same result into DTOs.
///
/// There is no by-id counterpart: `arclain_db` has a `get_profile`, but
/// `arclain_core` does not re-export it, and this table holds a handful
/// of rows a user maintains by hand. Reaching around core for one narrow
/// query would mean a second access path to the same table for no
/// measurable gain -- and every caller that wants one row here also
/// wants to check a name against the others.
async fn list_core_profiles(
    inner: &Arc<AppRuntime>,
    dbs: &arclain_core::ConfigDbs,
) -> Result<Vec<ArchiveProfile>, ApplicationError> {
    let handle = handle_for(inner)?;
    let pool = dbs.config_pool.clone();
    handle
        .spawn_blocking(move || {
            pool.with_conn(|conn| {
                Ok(arclain_core::list_profiles(conn)?
                    .iter()
                    .map(ArchiveProfile::from_db)
                    .collect::<Vec<_>>())
            })
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("listing archive profiles", error))
}

async fn list_profiles(
    inner: &Arc<AppRuntime>,
    dbs: &arclain_core::ConfigDbs,
) -> Result<Vec<OrganizationProfileSummary>, ApplicationError> {
    Ok(list_core_profiles(inner, dbs)
        .await?
        .iter()
        .map(organization::summarize_profile)
        .collect())
}

pub(super) async fn run_upsert_organization_profile(
    inner: &Arc<AppRuntime>,
    input: OrganizationProfileInput,
) -> Result<Vec<OrganizationProfileSummary>, ApplicationError> {
    let id = match input.id.as_deref() {
        Some(value) => Some(parse_row_id(value, "id")?),
        None => None,
    };
    // Validate the caller-supplied fields before taking any lock. The
    // `is_system` argument is provisional here (a create always stores
    // `false`); an update replaces it with the stored row's own value
    // below, so the flag can never be introduced or cleared by a caller.
    organization::profile_to_core(&input, id.unwrap_or(0), false)?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let dbs = require_config_dbs(inner)?;
    let existing = list_core_profiles(inner, &dbs).await?;

    let is_system = match id {
        Some(id) => match existing.iter().find(|profile| profile.id == id) {
            Some(stored) => stored.is_system,
            None => return Err(profile_not_found_error(id)),
        },
        None => false,
    };
    // `archive_profiles.name` is `UNIQUE`. Unlike a rule, a profile has
    // no name-keyed upsert fallback, so a create under a taken name is
    // always a collision and an update renaming onto another profile's
    // name is too. Reporting it as a `Conflict` here names the field the
    // caller must change; letting it reach SQLite would surface as an
    // opaque, and falsely retryable, storage failure.
    if existing
        .iter()
        .any(|profile| Some(profile.id) != id && profile.name == input.name)
    {
        return Err(duplicate_name_error("archive profile", &input.name));
    }

    let profile = organization::profile_to_core(&input, id.unwrap_or(0), is_system)?;
    let pool = dbs.config_pool.clone();
    handle_for(inner)?
        .spawn_blocking(move || {
            pool.with_conn(|conn| arclain_core::save_profile(conn, &profile.to_db()))
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("saving archive profile", error))?;

    list_profiles(inner, &dbs).await
}

/// Deletes the profile with `profile_id`, preserving the storage
/// layer's semantics exactly:
///
/// * **Unknown id** -> `NotFound`. This facade validates the id against
///   the table before delegating (`arclain_db::delete_profile` itself
///   would simply affect no rows and report success).
/// * **System profile** -> success, *and the profile is still there*.
///   `arclain_db::delete_profile` filters on `is_system = false`, so a
///   system profile is silently immune. The returned list is the real
///   post-delete state, so a caller that renders it sees the profile
///   survived rather than being told it is gone.
/// * **The default profile** (and it is not a system profile) -> deleted,
///   and **no profile becomes the default in its place**. Nothing
///   promotes a replacement, so the table is left with zero defaults
///   until something sets one. Nothing else in the schema references a
///   profile row either, so there is no other referential check to
///   perform: an in-flight organize that already resolved this profile
///   keeps its own copy, and a *future* `start_organize` naming the
///   deleted id fails with its own `NotFound`.
pub(super) async fn run_delete_organization_profile(
    inner: &Arc<AppRuntime>,
    profile_id: String,
) -> Result<Vec<OrganizationProfileSummary>, ApplicationError> {
    let id = parse_row_id(&profile_id, "profile_id")?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let dbs = require_config_dbs(inner)?;
    if !profile_exists(inner, &dbs, id).await? {
        return Err(profile_not_found_error(id));
    }

    let pool = dbs.config_pool.clone();
    handle_for(inner)?
        .spawn_blocking(move || {
            pool.with_conn(|conn| arclain_core::delete_profile(conn, id as i32))
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("deleting archive profile", error))?;

    list_profiles(inner, &dbs).await
}

/// Makes `profile_id` the one default profile, clearing every other
/// profile's default flag.
///
/// The existence check in front of the write is load-bearing, not
/// cosmetic: `arclain_db::set_default_profile` clears every default
/// *first* and only then sets the named one, so delegating an id that
/// names no row would leave the table with no default at all.
pub(super) async fn run_set_default_organization_profile(
    inner: &Arc<AppRuntime>,
    profile_id: String,
) -> Result<Vec<OrganizationProfileSummary>, ApplicationError> {
    let id = parse_row_id(&profile_id, "profile_id")?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let dbs = require_config_dbs(inner)?;
    if !profile_exists(inner, &dbs, id).await? {
        return Err(profile_not_found_error(id));
    }

    let pool = dbs.config_pool.clone();
    handle_for(inner)?
        .spawn_blocking(move || {
            pool.with_conn(|conn| arclain_core::set_default_profile(conn, id as i32))
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("setting the default archive profile", error))?;

    list_profiles(inner, &dbs).await
}

async fn profile_exists(
    inner: &Arc<AppRuntime>,
    dbs: &arclain_core::ConfigDbs,
    id: i64,
) -> Result<bool, ApplicationError> {
    Ok(list_core_profiles(inner, dbs)
        .await?
        .iter()
        .any(|profile| profile.id == id))
}

// ============================================================================
// Preview.
// ============================================================================

pub(super) async fn run_preview_organize_plan(
    inner: &Arc<AppRuntime>,
    session_id: ArchiveSessionId,
    rule_id: String,
) -> Result<OrganizePlanPreview, ApplicationError> {
    let id = parse_row_id(&rule_id, "rule_id")?;
    let session = inner.archive_sessions().get(session_id).await?;
    let service = require_organization_service(inner)?;
    let handle = handle_for(inner)?;

    let rule = handle
        .spawn_blocking(move || service.get_domain_rule(id))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("looking up organization rule", error))?
        .ok_or_else(|| rule_not_found_error(id))?;

    // Everything below is CPU-bound over the whole entry list (rebuild,
    // prune, plan, integrity), so it runs on the blocking pool rather
    // than on an async worker -- and it is awaited here, so nothing
    // outlives this call.
    let echo_rule_id = rule_id;
    handle
        .spawn_blocking(move || {
            let (revision, entries) = session.organization_entries();
            let archive_name = session
                .source_path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            // The plugin-reported metadata blob this session holds, read
            // exactly the way the pre-facade organize panel read it: the
            // same JSON the `emit_metadata` host function wrote, parsed
            // through `GameMetadata::from_json`. A blob that fails to
            // parse yields a metadata-less preview rather than an error,
            // matching the pre-facade behavior (which logged and skipped).
            let metadata = session
                .metadata()
                .and_then(|value| GameMetadata::from_json(&value.to_string()).ok());

            let plan = RuleEngine::create_plan(&rule, &archive_name, &entries, metadata.as_ref())
                .map_err(|error| unusable_plan_error(&error))?;
            let report = IntegrityReport::calculate(&entries, Some(&plan), metadata.as_ref());

            Ok(build_preview(
                session_id,
                revision,
                echo_rule_id,
                &plan,
                report,
            ))
        })
        .await
        .map_err(internal_join_error)?
}

/// Assembles the DTO, giving every list a deterministic order.
///
/// The underlying plan's own ordering is not meaningful: the rule
/// engine flattens a `HashMap`-backed path tree, and the integrity
/// report's "missing" list comes out of a `HashSet` difference, so both
/// vary run to run for identical input. Sorting here costs one pass and
/// makes two previews of the same plan compare, serialize, and render
/// identically.
fn build_preview(
    session_id: ArchiveSessionId,
    revision: u64,
    rule_id: String,
    plan: &arclain_core::features::organization::engine::OrganizationPlan,
    report: IntegrityReport,
) -> OrganizePlanPreview {
    let mut moves: Vec<PlannedMoveDto> = plan
        .moves
        .iter()
        .map(|(source, destination)| PlannedMoveDto {
            source: source.clone(),
            destination: destination.clone(),
        })
        .collect();
    moves.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.destination.cmp(&b.destination))
    });

    let mut generated_files: Vec<String> = plan
        .generated_files
        .iter()
        .map(|(path, _content)| path.clone())
        .collect();
    generated_files.sort();

    let mut downloads: Vec<PlannedDownloadDto> = plan
        .downloads
        .iter()
        .map(|download| PlannedDownloadDto {
            destination: download.dest_path.clone(),
            cached: download.cached,
        })
        .collect();
    downloads.sort_by(|a, b| a.destination.cmp(&b.destination));

    let mut resolved_variables: Vec<ResolvedVariableDto> = plan
        .resolved_variables
        .iter()
        .map(|(name, value)| ResolvedVariableDto {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    resolved_variables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut missing_original_files = report.missing_original_files;
    missing_original_files.sort();

    OrganizePlanPreview {
        session_id,
        revision,
        rule_id,
        rule_name: plan.rule_name.clone(),
        root_folder: plan.root_folder.clone(),
        root_folder_template: plan.root_folder_template.clone(),
        use_standard_layout: plan.use_standard_layout,
        moves,
        generated_files,
        downloads,
        resolved_variables,
        integrity: OrganizeIntegrityDto {
            original_files: report.original_files as u64,
            original_folders: report.original_folders as u64,
            moved_files: report.moved_files as u64,
            generated_files: report.generated_files as u64,
            expected_screenshots: report.expected_screenshots as u64,
            planned_screenshots: report.planned_screenshots as u64,
            expected_modified_files: report.expected_modified_files as u64,
            file_discrepancy: report.file_discrepancy,
            missing_original_files,
            original_hash: report.original_hash,
            result_hash: report.result_hash,
            content_match: report.content_match,
        },
    }
}

// ============================================================================
// Error helpers.
// ============================================================================

fn organization_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "organization data is unavailable: no configuration database is open",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn rule_not_found_error(rule_id: i64) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such organization rule")
        .with_diagnostic(format!("rule id {rule_id} does not exist"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("rule_id")
}

/// Both tables declare `name TEXT NOT NULL UNIQUE`. Reported as a
/// `Conflict` the caller resolves by choosing another name -- not as a
/// `Backend` failure, which is what the raw constraint violation would
/// otherwise be classified (and, worse, marked retryable) as.
fn duplicate_name_error(kind: &'static str, name: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "another entry already uses that name",
    )
    .with_diagnostic(format!("an {kind} named {name:?} already exists"))
    .with_recoverability(Recoverability::UserAction)
    .with_field("name")
}

fn profile_not_found_error(profile_id: i64) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such archive profile")
        .with_diagnostic(format!("profile id {profile_id} does not exist"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("profile_id")
}

/// A rule that cannot produce a usable plan for *this* archive: an
/// output path escaping the organized root, or two entries planned onto
/// one destination. Classified as `InvalidInput` against `rule_id`
/// rather than `Backend`, because the fix is always to edit or choose a
/// different rule -- nothing is retryable and nothing about the archive
/// is broken.
fn unusable_plan_error(error: &anyhow::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "this rule cannot produce a valid plan for this archive",
    )
    .with_diagnostic(format!("{error:#}"))
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::ChooseDestination)
    .with_field("rule_id")
}

fn backend_error(context: &'static str, error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, "organization storage failed")
        .with_diagnostic(format!("{context}: {error:#}"))
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true)
}

fn shutdown_mid_request_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "application is shutting down",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn internal_join_error(join_error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
        .with_diagnostic(join_error.to_string())
}
