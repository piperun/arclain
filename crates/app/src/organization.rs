//! The organization feature's frontend-neutral surface: archive-output
//! profile CRUD, organization-rule CRUD, and the synchronous
//! plan preview an organize panel recomputes as the user changes rules.
//!
//! This module holds the DTOs plus the pure validation/conversion logic
//! over them; `crate::runtime::organization_ops` holds the
//! `AppRuntime`-touching execution layer, and `crate::runtime`'s own
//! `impl ArclainApp` exposes the thin dispatch wrappers -- the same
//! three-layer split `crate::settings`/`runtime::settings_ops` already
//! uses for the settings/secrets surface.
//!
//! ## The two identities, and why they stay separate
//!
//! [`crate::operations::OrganizeRequest`] carries a `profile_id` *and* a
//! `rule_id` because they name two genuinely different things:
//!
//! - an **organization rule** decides the organized *layout* -- which
//!   file ends up where inside the output;
//! - an **archive profile** decides the output archive's *container* --
//!   format, compression level/method, solid, header encryption.
//!
//! Every type and method here preserves that split: rule CRUD and
//! profile CRUD are separate surfaces with separate id spaces, and
//! [`OrganizePlanPreview`] previews the rule half only (see
//! [`crate::runtime::ArclainApp::preview_organize_plan`]'s own doc
//! comment for why the profile half has nothing to preview).
//!
//! ## Ids
//!
//! Rule and profile ids are decimal-integer strings, identical in shape
//! and meaning to `OrganizeRequest`'s own `rule_id`/`profile_id`: a
//! summary's `id` can be handed straight back to `start_organize`. They
//! are *not* [`crate::ids`] opaque identifiers -- those are minted by
//! this process and live only as long as it does, whereas these are the
//! config database's own durable row ids, stable across restarts, which
//! is exactly what a saved rule/profile selection needs.

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::ids::ArchiveSessionId;

use arclain_core::features::organization::{
    ArchiveFormat, ArchiveProfile, GameMetadata, MoveAction, OrganizationRule, RuleActions,
    RuleTrigger,
};

/// The highest compression level any profile may store.
///
/// Mirrors the `-mx=` switch `arclain_core`'s 7-Zip CLI backend builds
/// from `ArchiveProfile::compression_level` (`sevenz_cli/backend.rs`),
/// which 7-Zip itself defines over 0-9, and the 0..=9 slider the
/// pre-facade profiles editor already constrained the value to.
/// Enforced here so an out-of-range level fails immediately, at the
/// save that introduced it, rather than much later as an opaque
/// external-tool failure on the first pack that uses the profile.
const MAX_COMPRESSION_LEVEL: u8 = 9;

// ============================================================================
// Rule DTOs.
// ============================================================================

/// One saved organization rule, mirroring
/// `arclain_core::features::organization::OrganizationRule` field for
/// field so a rules editor can round-trip a rule through
/// [`OrganizationRuleInput`] without losing anything.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationRuleSummary {
    /// Decimal-integer id; hand this straight to
    /// [`crate::operations::OrganizeRequest::rule_id`].
    pub id: String,
    pub name: String,
    /// Higher runs first when several rules match one archive.
    pub priority: i32,
    pub enabled: bool,
    pub trigger: OrganizationRuleTriggerDto,
    pub actions: OrganizationRuleActionsDto,
}

/// What makes a rule apply to an archive. Every field is optional and an
/// unset field matches everything, so a wholly-default trigger matches
/// every archive -- matching `RuleTrigger`'s own semantics.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationRuleTriggerDto {
    /// Matches `GameMetadata::source` exactly (e.g. `"dlsite"`).
    pub metadata_source: Option<String>,
    /// A regular expression matched against the archive's file name.
    pub filename_pattern: Option<String>,
    /// Matches when some entry's path contains this substring.
    pub has_file: Option<String>,
}

/// What a rule does once it applies.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationRuleActionsDto {
    /// The organized output's top folder, as a `$variable` template.
    /// `None` means the literal `"Game"` (`RuleEngine::create_plan`'s own
    /// fallback).
    pub root_folder: Option<String>,
    /// Template for the output archive's name. Stored and round-tripped
    /// by this facade; consumed elsewhere in `arclain_core`.
    pub output_name: Option<String>,
    /// Glob-to-target routing, applied in order; the first matching
    /// pattern wins. Ignored entirely when `use_standard_layout` is set.
    pub move_files: Vec<OrganizationMoveActionDto>,
    /// Detect the archive's inner game-content root and rehome it under
    /// `{root_folder}/Game`, instead of applying `move_files`.
    pub use_standard_layout: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationMoveActionDto {
    pub pattern: String,
    pub target: String,
}

/// A rule create/update request.
///
/// `id: Some(..)` updates that exact rule. `id: None` creates a rule --
/// *unless* an existing rule already has the same `name`, in which case
/// that rule is updated instead. That name-keyed fallback is
/// `OrganizationService::save_domain_rule`'s own long-standing behavior,
/// preserved here rather than replaced, and it matches
/// [`crate::settings::PasswordRuleInput`]'s purely name-keyed upsert:
/// re-saving a rule under a name already in use never silently creates a
/// second rule with that name.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationRuleInput {
    pub id: Option<String>,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    pub trigger: OrganizationRuleTriggerDto,
    pub actions: OrganizationRuleActionsDto,
}

// ============================================================================
// Profile DTOs.
// ============================================================================

/// One archive-output profile (format/compression preset).
///
/// Carries every stored field, not just the three an organize dropdown
/// needs, so the profiles editor and the dropdown read the same type --
/// there are only ever a handful of profiles, so the wider shape costs
/// nothing and avoids a second near-identical DTO.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationProfileSummary {
    /// Decimal-integer id; hand this straight to
    /// [`crate::operations::OrganizeRequest::profile_id`].
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// `"7z"` or `"zip"` -- one of the tokens
    /// [`OrganizationProfileInput::output_format`] accepts.
    pub output_format: String,
    /// 0 (store) through [`MAX_COMPRESSION_LEVEL`].
    pub compression_level: u8,
    pub compression_method: Option<String>,
    pub solid_archive: bool,
    pub encrypt_headers: bool,
    /// Exactly one profile is the default at a time; see
    /// [`crate::runtime::ArclainApp::set_default_organization_profile`]
    /// for the one documented way that invariant can end up with *no*
    /// default.
    pub is_default: bool,
    /// A profile seeded by the application itself. System profiles
    /// cannot be deleted -- see
    /// [`crate::runtime::ArclainApp::delete_organization_profile`].
    pub is_system: bool,
}

/// A profile create/update request.
///
/// `id: Some(..)` updates that exact profile; `id: None` creates one.
/// Unlike [`OrganizationRuleInput`] there is no name-keyed fallback:
/// the profile table has a `UNIQUE` constraint on `name`, so a create
/// whose name is already taken fails loudly instead of silently
/// retargeting an existing row.
///
/// There is deliberately **no `is_system` field**. The flag is what
/// makes a profile undeletable, so it is not something an untrusted
/// caller gets to set: a create always stores `is_system = false`, and
/// an update preserves whatever the stored row already had. The
/// pre-facade profiles page round-tripped the flag through the editor
/// and so never actually changed it; this makes that guarantee
/// structural instead of incidental.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationProfileInput {
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    /// `"7z"` or `"zip"`, matched case-insensitively and stored
    /// canonically. Any other value is rejected as
    /// [`ApplicationErrorKind::InvalidInput`] rather than silently
    /// coerced -- `ArchiveFormat::from_str` maps everything it does not
    /// recognize to 7z, which would leave the stored `output_format`
    /// string and the format actually used to pack disagreeing.
    pub output_format: String,
    pub compression_level: u8,
    /// One of the methods the chosen `output_format` supports
    /// (`ArchiveProfile::available_compression_methods`), matched
    /// case-insensitively and stored canonically, or `None` for the
    /// backend's own default.
    pub compression_method: Option<String>,
    pub solid_archive: bool,
    pub encrypt_headers: bool,
    /// Setting this clears every other profile's default flag, the same
    /// way [`crate::runtime::ArclainApp::set_default_organization_profile`]
    /// does. Clearing it on the profile that currently *is* the default
    /// leaves no default at all.
    pub is_default: bool,
}

// ============================================================================
// Preview DTOs.
// ============================================================================

/// One planned file relocation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PlannedMoveDto {
    /// The entry's current archive-relative path.
    pub source: String,
    /// Where the organized output puts it, relative to the output root.
    pub destination: String,
}

/// One screenshot/asset the organized output would fetch.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PlannedDownloadDto {
    /// Where the fetched bytes land, relative to the output root.
    pub destination: String,
    /// Whether the content cache already holds this asset, so the
    /// organize run would not have to fetch it over the network.
    pub cached: bool,
}

/// One `$name` template variable the rule resolved for this archive.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ResolvedVariableDto {
    /// The bare variable name, without the leading `$`.
    pub name: String,
    pub value: String,
}

/// The plan's self-check: how the organized output's file set compares
/// to the archive's own. Mirrors
/// `arclain_core::features::organization::IntegrityReport` field for
/// field.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizeIntegrityDto {
    pub original_files: u64,
    /// Counts every directory in the session's entry index, including
    /// ancestors the index synthesizes from file paths when the archive
    /// did not list them explicitly -- so this agrees with the folder
    /// count the rest of this facade's archive read model reports for
    /// the same session, which is what a frontend renders alongside it.
    pub original_folders: u64,
    pub moved_files: u64,
    pub generated_files: u64,
    /// How many screenshots the resolved metadata offers.
    pub expected_screenshots: u64,
    /// How many of them the plan actually schedules.
    pub planned_screenshots: u64,
    pub expected_modified_files: u64,
    /// `moved + generated + downloaded` minus `original + generated +
    /// downloaded`. Reported exactly as `IntegrityReport` computes it;
    /// see [`crate::runtime::ArclainApp::preview_organize_plan`]'s doc
    /// comment for the known quirk in that computation.
    pub file_discrepancy: i64,
    /// Files the plan does not relocate anywhere. What the pre-facade
    /// panel's "Export Issues" report listed.
    pub missing_original_files: Vec<String>,
    /// FNV-1a over the sorted original file-path list.
    pub original_hash: u64,
    /// FNV-1a over the sorted planned move-source list.
    pub result_hash: u64,
    /// `original_hash == result_hash`: every file the archive holds is
    /// accounted for by some planned move.
    pub content_match: bool,
}

/// What one organization rule would do to one open archive.
///
/// Purely descriptive: computing this changes nothing, touches no
/// archive bytes, and registers no operation. See
/// [`crate::runtime::ArclainApp::preview_organize_plan`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizePlanPreview {
    pub session_id: ArchiveSessionId,
    /// The session revision this preview was computed against. A caller
    /// holding a preview whose `revision` no longer matches
    /// `archive_snapshot`'s should recompute: the archive was mutated
    /// underneath it. Never reports a revision *newer* than the entries
    /// the plan was built from.
    pub revision: u64,
    /// Echoes the requested rule id, so an out-of-order response cannot
    /// be mistaken for the currently selected rule's preview.
    pub rule_id: String,
    pub rule_name: String,
    /// The output's top folder with every `$variable` already expanded.
    pub root_folder: String,
    /// The same folder before expansion, as the rule stores it.
    pub root_folder_template: String,
    pub use_standard_layout: bool,
    pub moves: Vec<PlannedMoveDto>,
    /// Paths of files the organize run would synthesize (today, the
    /// metadata sidecar). Deliberately paths only: the generated
    /// *content* is never displayed, is regenerated at execution time
    /// anyway, and would put a serialized metadata blob on a path a
    /// frontend recomputes on every rule change.
    pub generated_files: Vec<String>,
    pub downloads: Vec<PlannedDownloadDto>,
    /// Sorted by `name`, so two previews of the same plan compare and
    /// serialize identically (the underlying map has no stable order).
    pub resolved_variables: Vec<ResolvedVariableDto>,
    pub integrity: OrganizeIntegrityDto,
}

// ============================================================================
// Pure DTO <-> domain conversions.
// ============================================================================

/// The plan-facing view of one archive session's plugin-reported
/// metadata: the JSON the `emit_metadata` host function wrote, parsed
/// the one way every planner in this crate reads it. A blob that fails
/// to parse yields a metadata-less plan rather than an error, matching
/// the pre-facade organize panel (which logged and skipped).
///
/// One function with two callers on purpose. A preview
/// ([`crate::runtime::ArclainApp::preview_organize_plan`]) and the
/// session-bound organize that applies it
/// ([`crate::operations::OrganizeRequest::archive_session_id`]) must
/// build their plans from the same metadata or they describe different
/// outcomes -- so they read it through the same function rather than
/// through two copies that have to be kept in agreement.
pub(crate) fn session_metadata_for_planning(
    raw: Option<serde_json::Value>,
) -> Option<GameMetadata> {
    raw.and_then(|value| GameMetadata::from_json(&value.to_string()).ok())
}

pub(crate) fn summarize_rule(rule: &OrganizationRule) -> OrganizationRuleSummary {
    OrganizationRuleSummary {
        id: rule.id.to_string(),
        name: rule.name.clone(),
        priority: rule.priority,
        enabled: rule.is_enabled,
        trigger: OrganizationRuleTriggerDto {
            metadata_source: rule.trigger.metadata_source.clone(),
            filename_pattern: rule.trigger.filename_pattern.clone(),
            has_file: rule.trigger.has_file.clone(),
        },
        actions: OrganizationRuleActionsDto {
            root_folder: rule.actions.root_folder.clone(),
            output_name: rule.actions.output_name.clone(),
            move_files: rule
                .actions
                .move_files
                .iter()
                .map(|action| OrganizationMoveActionDto {
                    pattern: action.pattern.clone(),
                    target: action.target.clone(),
                })
                .collect(),
            use_standard_layout: rule.actions.use_standard_layout,
        },
    }
}

pub(crate) fn summarize_profile(profile: &ArchiveProfile) -> OrganizationProfileSummary {
    OrganizationProfileSummary {
        id: profile.id.to_string(),
        name: profile.name.clone(),
        description: profile.description.clone(),
        output_format: profile.format.as_str().to_string(),
        compression_level: profile.compression_level,
        compression_method: profile.compression_method.clone(),
        solid_archive: profile.solid_archive,
        encrypt_headers: profile.encrypt_headers,
        is_default: profile.is_default,
        is_system: profile.is_system,
    }
}

/// Validates `input` and, on success, builds the core rule to persist.
/// `id` is the already-parsed row id (`0` meaning "create, or update the
/// rule with this name" -- see [`OrganizationRuleInput`]'s own doc
/// comment).
pub(crate) fn rule_to_core(
    input: &OrganizationRuleInput,
    id: i64,
) -> Result<OrganizationRule, ApplicationError> {
    if input.name.trim().is_empty() {
        return Err(invalid_input_error("name", "rule name must not be empty"));
    }
    if let Some(pattern) = trimmed_non_empty(input.trigger.filename_pattern.as_deref()) {
        if regex::Regex::new(pattern).is_err() {
            return Err(invalid_input_error(
                "trigger.filename_pattern",
                "filename pattern is not a valid regular expression",
            ));
        }
    }
    for action in &input.actions.move_files {
        if action.pattern.trim().is_empty() {
            return Err(invalid_input_error(
                "actions.move_files.pattern",
                "move pattern must not be empty",
            ));
        }
    }
    Ok(OrganizationRule {
        id,
        name: input.name.clone(),
        priority: input.priority,
        is_enabled: input.enabled,
        trigger: RuleTrigger {
            metadata_source: input.trigger.metadata_source.clone(),
            filename_pattern: input.trigger.filename_pattern.clone(),
            has_file: input.trigger.has_file.clone(),
        },
        actions: RuleActions {
            root_folder: input.actions.root_folder.clone(),
            output_name: input.actions.output_name.clone(),
            move_files: input
                .actions
                .move_files
                .iter()
                .map(|action| MoveAction {
                    pattern: action.pattern.clone(),
                    target: action.target.clone(),
                })
                .collect(),
            use_standard_layout: input.actions.use_standard_layout,
        },
    })
}

/// Validates `input` and, on success, builds the core profile to
/// persist. `id` is the already-parsed row id (`0` meaning "create"),
/// and `is_system` is carried over from the stored row on an update --
/// never taken from the caller (see [`OrganizationProfileInput`]).
pub(crate) fn profile_to_core(
    input: &OrganizationProfileInput,
    id: i64,
    is_system: bool,
) -> Result<ArchiveProfile, ApplicationError> {
    if input.name.trim().is_empty() {
        return Err(invalid_input_error(
            "name",
            "profile name must not be empty",
        ));
    }
    let format = parse_output_format(&input.output_format)?;
    if input.compression_level > MAX_COMPRESSION_LEVEL {
        return Err(invalid_input_error(
            "compression_level",
            "compression level must be between 0 and 9",
        ));
    }
    let compression_method = match trimmed_non_empty(input.compression_method.as_deref()) {
        Some(method) => Some(canonical_compression_method(format, method)?),
        None => None,
    };
    Ok(ArchiveProfile {
        id,
        name: input.name.clone(),
        description: input.description.clone(),
        format,
        compression_level: input.compression_level,
        compression_method,
        solid_archive: input.solid_archive,
        encrypt_headers: input.encrypt_headers,
        is_default: input.is_default,
        is_system,
    })
}

/// Accepts any casing of a format token `ArchiveFormat` itself
/// round-trips (`ArchiveFormat::all`), so adding a format to
/// `arclain_core` widens this automatically rather than silently
/// leaving it rejected here.
fn parse_output_format(value: &str) -> Result<ArchiveFormat, ApplicationError> {
    let wanted = value.trim().to_lowercase();
    ArchiveFormat::all()
        .iter()
        .copied()
        .find(|format| format.as_str().eq_ignore_ascii_case(&wanted))
        .ok_or_else(|| {
            invalid_input_error("output_format", "unsupported archive output format")
                .with_diagnostic(format!(
                    "got {value:?}; expected one of {}",
                    supported_format_tokens()
                ))
        })
}

fn supported_format_tokens() -> String {
    ArchiveFormat::all()
        .iter()
        .map(|format| format.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Maps a caller-supplied method name onto the exact spelling
/// `arclain_core`'s packer expects for `format`.
///
/// Rejecting an unknown method here rather than passing it through is
/// deliberate: for 7z the value is interpolated straight into the
/// backend's `-m0=<method>` switch, so an unrecognized one becomes an
/// opaque external-tool failure on the first pack that uses the profile
/// -- long after, and far away from, the save that introduced it.
fn canonical_compression_method(
    format: ArchiveFormat,
    method: &str,
) -> Result<String, ApplicationError> {
    let probe = ArchiveProfile {
        format,
        ..ArchiveProfile::default()
    };
    probe
        .available_compression_methods()
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(method))
        .map(|candidate| (*candidate).to_string())
        .ok_or_else(|| {
            invalid_input_error(
                "compression_method",
                "unsupported compression method for this output format",
            )
            .with_diagnostic(format!(
                "got {method:?}; {} supports {}",
                format.as_str(),
                probe.available_compression_methods().join(", ")
            ))
        })
}

fn trimmed_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Parses a decimal-integer rule/profile id, the same shape and the same
/// tolerance for surrounding whitespace
/// [`crate::operations::OrganizeRequest`] already accepts.
pub(crate) fn parse_id(value: &str, field: &'static str) -> Result<i64, ApplicationError> {
    value.trim().parse::<i64>().map_err(|_| {
        ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "expected a decimal integer id",
        )
        .with_diagnostic(format!("field {field:?}: got {value:?}"))
        .with_recoverability(Recoverability::UserAction)
        .with_field(field)
    })
}

fn invalid_input_error(field: &'static str, summary: &'static str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, summary)
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_input() -> OrganizationProfileInput {
        OrganizationProfileInput {
            id: None,
            name: "Balanced".to_string(),
            description: Some("middle of the road".to_string()),
            output_format: "7z".to_string(),
            compression_level: 5,
            compression_method: Some("LZMA2".to_string()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: false,
        }
    }

    fn rule_input() -> OrganizationRuleInput {
        OrganizationRuleInput {
            id: None,
            name: "Standard".to_string(),
            priority: 10,
            enabled: true,
            trigger: OrganizationRuleTriggerDto {
                metadata_source: None,
                filename_pattern: Some(r"(RJ|VJ)\d+".to_string()),
                has_file: None,
            },
            actions: OrganizationRuleActionsDto {
                root_folder: Some("[$product_id] $title".to_string()),
                output_name: None,
                move_files: vec![OrganizationMoveActionDto {
                    pattern: "*.exe".to_string(),
                    target: "bin".to_string(),
                }],
                use_standard_layout: false,
            },
        }
    }

    // ── rule conversion/validation ──────────────────────────────────────

    #[test]
    fn rule_round_trips_through_core_and_back() {
        let input = rule_input();
        let core = rule_to_core(&input, 7).expect("valid rule input");
        let summary = summarize_rule(&core);

        assert_eq!(summary.id, "7");
        assert_eq!(summary.name, input.name);
        assert_eq!(summary.priority, input.priority);
        assert_eq!(summary.enabled, input.enabled);
        assert_eq!(summary.trigger, input.trigger);
        assert_eq!(summary.actions, input.actions);
    }

    #[test]
    fn an_empty_rule_name_is_rejected() {
        let mut input = rule_input();
        input.name = "   ".to_string();
        let error = rule_to_core(&input, 0).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("name"));
    }

    /// The pattern is compiled by `RuleEngine::matches_trigger`, which
    /// silently treats a compile failure as "no match" -- so an invalid
    /// pattern would otherwise save fine and then quietly never fire.
    #[test]
    fn an_uncompilable_filename_pattern_is_rejected() {
        let mut input = rule_input();
        input.trigger.filename_pattern = Some("[unterminated".to_string());
        let error = rule_to_core(&input, 0).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("trigger.filename_pattern"));
    }

    #[test]
    fn a_blank_filename_pattern_is_not_treated_as_a_regex() {
        let mut input = rule_input();
        input.trigger.filename_pattern = Some("   ".to_string());
        let core = rule_to_core(&input, 0).expect("a blank pattern is not a regex error");
        assert_eq!(core.trigger.filename_pattern.as_deref(), Some("   "));
    }

    #[test]
    fn an_empty_move_pattern_is_rejected() {
        let mut input = rule_input();
        input.actions.move_files[0].pattern = String::new();
        let error = rule_to_core(&input, 0).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("actions.move_files.pattern"));
    }

    // ── profile conversion/validation ───────────────────────────────────

    #[test]
    fn profile_round_trips_through_core_and_back() {
        let input = profile_input();
        let core = profile_to_core(&input, 3, false).expect("valid profile input");
        let summary = summarize_profile(&core);

        assert_eq!(summary.id, "3");
        assert_eq!(summary.name, input.name);
        assert_eq!(summary.description, input.description);
        assert_eq!(summary.output_format, "7z");
        assert_eq!(summary.compression_level, 5);
        assert_eq!(summary.compression_method.as_deref(), Some("LZMA2"));
        assert!(summary.solid_archive);
        assert!(!summary.encrypt_headers);
        assert!(!summary.is_default);
        assert!(!summary.is_system);
    }

    #[test]
    fn an_output_format_is_matched_case_insensitively_and_stored_canonically() {
        let mut input = profile_input();
        input.output_format = "ZIP".to_string();
        input.compression_method = Some("deflate".to_string());
        let core = profile_to_core(&input, 0, false).expect("ZIP is a known format");
        assert_eq!(core.format.as_str(), "zip");
        assert_eq!(core.compression_method.as_deref(), Some("Deflate"));
    }

    /// `ArchiveFormat::from_str` maps everything it does not recognize to
    /// 7z. Storing an unrecognized token would therefore leave the
    /// profile *reporting* one format while *packing* another.
    #[test]
    fn an_unknown_output_format_is_rejected_rather_than_coerced_to_7z() {
        let mut input = profile_input();
        input.output_format = "rar".to_string();
        let error = profile_to_core(&input, 0, false).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("output_format"));
    }

    #[test]
    fn an_out_of_range_compression_level_is_rejected() {
        let mut input = profile_input();
        input.compression_level = 10;
        let error = profile_to_core(&input, 0, false).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("compression_level"));
    }

    #[test]
    fn a_compression_method_the_chosen_format_does_not_support_is_rejected() {
        let mut input = profile_input();
        input.output_format = "zip".to_string();
        input.compression_method = Some("PPMd".to_string());
        let error = profile_to_core(&input, 0, false).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("compression_method"));
    }

    #[test]
    fn a_blank_compression_method_means_the_backend_default() {
        let mut input = profile_input();
        input.compression_method = Some("  ".to_string());
        let core = profile_to_core(&input, 0, false).expect("a blank method is not an error");
        assert!(core.compression_method.is_none());
    }

    /// The flag is not on the input at all, so neither a create nor an
    /// update can introduce or clear it -- the caller of
    /// `profile_to_core` supplies it from the stored row.
    #[test]
    fn is_system_comes_from_the_caller_never_from_the_input() {
        let input = profile_input();
        assert!(!profile_to_core(&input, 0, false).unwrap().is_system);
        assert!(profile_to_core(&input, 4, true).unwrap().is_system);
    }

    // ── ids ─────────────────────────────────────────────────────────────

    #[test]
    fn ids_parse_with_the_same_whitespace_tolerance_as_organize_request() {
        assert_eq!(parse_id(" 42 \n", "rule_id").unwrap(), 42);
        let error = parse_id("dlsite-standard", "rule_id").unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("rule_id"));
    }

    // ── serialization ───────────────────────────────────────────────────

    fn round_trip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let json = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn rule_dtos_round_trip_through_json() {
        let input = rule_input();
        assert_eq!(round_trip(&input), input);

        let summary = summarize_rule(&rule_to_core(&input, 11).unwrap());
        assert_eq!(round_trip(&summary), summary);
        assert_eq!(round_trip(&summary.trigger), summary.trigger);
        assert_eq!(round_trip(&summary.actions), summary.actions);
        assert_eq!(
            round_trip(&summary.actions.move_files[0]),
            summary.actions.move_files[0]
        );
    }

    #[test]
    fn profile_dtos_round_trip_through_json() {
        let input = profile_input();
        assert_eq!(round_trip(&input), input);

        let summary = summarize_profile(&profile_to_core(&input, 2, true).unwrap());
        assert_eq!(round_trip(&summary), summary);
    }

    #[test]
    fn preview_dtos_round_trip_through_json() {
        let preview = OrganizePlanPreview {
            session_id: ArchiveSessionId::from_raw(9),
            revision: 3,
            rule_id: "11".to_string(),
            rule_name: "Standard".to_string(),
            root_folder: "[RJ1] Title".to_string(),
            root_folder_template: "[$product_id] $title".to_string(),
            use_standard_layout: true,
            moves: vec![PlannedMoveDto {
                source: "inner/game.exe".to_string(),
                destination: "[RJ1] Title/Game/game.exe".to_string(),
            }],
            generated_files: vec!["[RJ1] Title/metadata.json".to_string()],
            downloads: vec![PlannedDownloadDto {
                destination: "[RJ1] Title/screenshots/0.jpg".to_string(),
                cached: true,
            }],
            resolved_variables: vec![ResolvedVariableDto {
                name: "product_id".to_string(),
                value: "RJ1".to_string(),
            }],
            integrity: OrganizeIntegrityDto {
                original_files: 2,
                original_folders: 1,
                moved_files: 1,
                generated_files: 1,
                expected_screenshots: 1,
                planned_screenshots: 1,
                expected_modified_files: 3,
                file_discrepancy: -1,
                missing_original_files: vec!["inner/readme.txt".to_string()],
                original_hash: 0xcbf2_9ce4_8422_2325,
                result_hash: 0x0100_0000_01b3,
                content_match: false,
            },
        };

        assert_eq!(round_trip(&preview), preview);
        assert_eq!(round_trip(&preview.moves[0]), preview.moves[0]);
        assert_eq!(round_trip(&preview.downloads[0]), preview.downloads[0]);
        assert_eq!(
            round_trip(&preview.resolved_variables[0]),
            preview.resolved_variables[0]
        );
        assert_eq!(round_trip(&preview.integrity), preview.integrity);
    }

    /// The two hash fields are the widest integers in the DTO family --
    /// a serializer that silently narrowed them (or a `f64`-backed JSON
    /// number) would corrupt the `content_match` evidence a caller
    /// displays alongside them.
    #[test]
    fn integrity_hashes_survive_the_full_u64_range() {
        let integrity = OrganizeIntegrityDto {
            original_files: 0,
            original_folders: 0,
            moved_files: 0,
            generated_files: 0,
            expected_screenshots: 0,
            planned_screenshots: 0,
            expected_modified_files: 0,
            file_discrepancy: i64::MIN,
            missing_original_files: Vec::new(),
            original_hash: u64::MAX,
            result_hash: u64::MAX - 1,
            content_match: false,
        };
        assert_eq!(round_trip(&integrity), integrity);
    }
}
