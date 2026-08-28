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

use arclain_core::features::organization::engine::{plan_stages_nothing, StagedContent};
use arclain_core::features::organization::layout::{
    FetchSource, Fetched, FileVariable, Generated, GeneratedContent, Layout, OutputSelector,
    Placement, Source,
};
use arclain_core::features::organization::{
    ArchiveFormat, ArchiveProfile, GameMetadata, OrganizationRule, RuleActions, RuleTrigger,
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

/// Reports whether an archive name contains a recognized DLsite product code.
///
/// This is application vocabulary rather than presentation policy: the same
/// detector gates metadata-backed organization rules and metadata lookup.
/// Frontends use this query instead of importing the core utility that owns
/// the underlying Gameta-compatible detection algorithm.
///
/// Without the `gameta` feature there is no detector behind it, so every
/// name answers `false` -- metadata-backed rules match nothing rather
/// than failing, which is how the whole metadata surface behaves in a
/// lean build.
pub fn has_dlsite_product_code(archive_name: &str) -> bool {
    #[cfg(feature = "gameta")]
    {
        arclain_core::utilities::has_dlsite_code(archive_name)
    }
    #[cfg(not(feature = "gameta"))]
    {
        let _ = archive_name;
        false
    }
}

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
///
/// Everything about the *shape* of the result lives in [`Self::layout`];
/// `output_name` is outside it because it names the output archive's
/// container rather than the arrangement inside it. Mirrors
/// `arclain_core::features::organization::RuleActions` field for field.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OrganizationRuleActionsDto {
    /// Template for the output archive's name. Stored and round-tripped
    /// by this facade; consumed elsewhere in `arclain_core`.
    pub output_name: Option<String>,
    pub layout: LayoutDto,
}

/// How an archive is arranged into one or more organized folders.
/// Mirrors `arclain_core::features::organization::layout::Layout`.
///
/// A rule saved under the retired vocabulary (`root_folder`,
/// `move_files`, `use_standard_layout`) is translated to this shape as
/// it is read from the config database, so a summary always reports a
/// layout even for a rule that never stored one.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LayoutDto {
    /// What counts as one output. An archive is not necessarily one
    /// folder: a mod pack produces one per mod.
    pub outputs: OutputSelectorDto,
    /// Variables read out of files inside the input, resolved once per
    /// output, usable in `name` and in any `into`.
    pub file_variables: Vec<FileVariableDto>,
    /// Template for each output's root folder name. Empty means the
    /// output has no wrapper and its content sits at the top level.
    pub name: String,
    /// Where each output's content goes. Evaluated in order; the first
    /// placement that matches a file claims it.
    pub place: Vec<PlacementDto>,
    /// Files written into each output.
    pub generate: Vec<GeneratedFileDto>,
    /// Images fetched into each output.
    pub fetch: Vec<FetchedFileDto>,
}

impl Default for LayoutDto {
    /// One unnamed output that places nothing -- the same default
    /// `Layout` carries, so a blank rule built here and a blank rule
    /// built in core describe the same (empty) arrangement.
    fn default() -> Self {
        Self {
            outputs: OutputSelectorDto::Whole,
            file_variables: Vec::new(),
            name: String::new(),
            place: Vec::new(),
            generate: Vec::new(),
            fetch: Vec::new(),
        }
    }
}

/// What counts as one output.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum OutputSelectorDto {
    /// The whole input is one output.
    Whole,
    /// One output per directory that directly contains `marker`.
    PerDirectoryContaining { marker: String },
}

/// One variable read out of a file inside the input.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FileVariableDto {
    /// Name a template refers to, without the leading `$`.
    pub as_name: String,
    /// Path of the file to read, relative to the output's own root.
    pub file: String,
    /// Key to take from it.
    pub key: String,
}

/// Where one group of an output's files goes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PlacementDto {
    pub from: PlacementSourceDto,
    /// Destination inside the output. Empty means the output's root.
    pub into: String,
}

/// The source of files a placement carries.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PlacementSourceDto {
    /// Everything under this output's root.
    All,
    /// Paths matching a glob, relative to the output's root.
    Matching(String),
    /// The folder that looks like the payload, by indicator scoring.
    ContentRoot,
}

/// A file written into each output.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GeneratedFileDto {
    pub into: String,
    pub content: GeneratedContentDto,
}

/// The kind of file content to generate.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum GeneratedContentDto {
    /// The layered document the metadata provider produced.
    MetadataDocument,
}

/// An image fetched into each output.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FetchedFileDto {
    pub into: String,
    pub source: FetchSourceDto,
    /// Template for each fetched file's name. Two tokens beyond the
    /// output's own variables: `$index` is the item's position counted
    /// from one and padded to three digits, and `$ext` is the extension
    /// the source URL carries (`jpg` when it names none).
    pub name: String,
}

/// The source of images to fetch into an output.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FetchSourceDto {
    Screenshots,
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

/// One output format an [`OrganizationProfileInput`] may name, with
/// everything a profile editor needs to offer it: the token to submit,
/// how to label it, the extension its outputs get, and the compression
/// methods it supports.
///
/// Exists so a frontend's format and method dropdowns are not a second,
/// drifting copy of what this facade actually accepts. Every field is
/// derived from `arclain_core`'s own format definitions, so a format
/// added there appears here (and is accepted by
/// [`OrganizationProfileInput::output_format`]) without a frontend
/// change.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ArchiveFormatOptionDto {
    /// What [`OrganizationProfileInput::output_format`] accepts and
    /// [`OrganizationProfileSummary::output_format`] reports (`"7z"`,
    /// `"zip"`).
    pub token: String,
    /// Human-facing label for this format.
    pub display_name: String,
    /// The file extension an organized output built with this format
    /// gets.
    pub extension: String,
    /// Accepted [`OrganizationProfileInput::compression_method`] values
    /// for this format, in the order a picker should offer them; the
    /// first is [`Self::default_compression_method`].
    pub compression_methods: Vec<String>,
    /// What the backend uses when a profile stores no method.
    pub default_compression_method: String,
    /// Whether `solid_archive` means anything for this format, as
    /// `arclain_core`'s own `ArchiveFormat::supports_solid_archive`
    /// answers it: a container with no solid-block concept ignores the
    /// flag, so a profile editor hides the toggle rather than storing
    /// something nothing can honor (the column still exists, and this
    /// facade still round-trips whatever is in it).
    pub supports_solid_archive: bool,
    /// Whether `encrypt_headers` means anything for this format, from
    /// `ArchiveFormat::supports_header_encryption` — only a container
    /// that can encrypt its own file listing.
    pub supports_header_encryption: bool,
}

/// Every output format a profile may store. Pure and stateless -- no
/// app handle, no async, no I/O (the same shape `crate::analyze_url`
/// has, for the same reason: a frontend needs this to render a form,
/// not to perform an operation).
pub fn archive_format_options() -> Vec<ArchiveFormatOptionDto> {
    ArchiveFormat::all()
        .iter()
        .map(|format| {
            let probe = ArchiveProfile {
                format: *format,
                ..ArchiveProfile::default()
            };
            ArchiveFormatOptionDto {
                token: format.as_str().to_string(),
                display_name: format.display_name().to_string(),
                extension: format.extension().to_string(),
                compression_methods: probe
                    .available_compression_methods()
                    .iter()
                    .map(|method| (*method).to_string())
                    .collect(),
                default_compression_method: probe.default_compression_method().to_string(),
                supports_solid_archive: format.supports_solid_archive(),
                supports_header_encryption: format.supports_header_encryption(),
            }
        })
        .collect()
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
    /// Every folder the run would produce, in the order the plan
    /// resolved them. One archive is not one output: a mod pack is one
    /// folder per mod, so a surface that renders only the first
    /// describes a fraction of the run.
    pub outputs: Vec<PlannedOutputDto>,
    /// Every folder that could *not* be named, and why. A passed-over
    /// folder is not an error -- the run simply produces nothing for it
    /// -- but it is the only place a user learns that it happened, so it
    /// travels with the plan rather than being dropped.
    pub skipped_outputs: Vec<SkippedOutputDto>,
    pub integrity: OrganizeIntegrityDto,
}

impl OrganizePlanPreview {
    /// Whether running this plan would put nothing on disk.
    ///
    /// A surface offering Apply has to ask this as well as whether a
    /// plan is on screen -- a rule with no placements previews
    /// perfectly well and describes exactly that run. Answered through
    /// `arclain_core`'s `StagedContent`, which is the same definition
    /// the core plan and the applier use, rather than a second copy of
    /// it over the DTO's three lists.
    pub fn stages_nothing(&self) -> bool {
        plan_stages_nothing(&self.outputs)
    }
}

impl StagedContent for PlannedOutputDto {
    fn staged_counts(&self) -> (usize, usize, usize) {
        (
            self.moves.len(),
            self.generated_files.len(),
            self.downloads.len(),
        )
    }
}

/// One folder an organize run would produce.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PlannedOutputDto {
    /// This output's top folder with every `$variable` already expanded.
    /// Empty means the output has no wrapper and its content sits at the
    /// top level (only ever true of a lone output).
    pub root_folder: String,
    /// The same folder before expansion, as the rule's layout stores it.
    pub root_folder_template: String,
    pub moves: Vec<PlannedMoveDto>,
    /// Paths of files the organize run would synthesize into this output
    /// (today, the metadata sidecar). Deliberately paths only: the
    /// generated *content* is never displayed, is regenerated at
    /// execution time anyway, and would put a serialized metadata blob on
    /// a path a frontend recomputes on every rule change.
    pub generated_files: Vec<String>,
    pub downloads: Vec<PlannedDownloadDto>,
    /// Sorted by `name`, so two previews of the same plan compare and
    /// serialize identically (the underlying map has no stable order).
    pub resolved_variables: Vec<ResolvedVariableDto>,
    /// Why this output looks the way it does: which folder was taken as
    /// the payload and on what evidence, what each placement claimed,
    /// which files nothing carried. Rendered, not merely carried -- a
    /// preview that says what will happen without saying why leaves a
    /// user unable to tell a good inference from a bad one.
    pub reasoning: Vec<String>,
}

/// A folder the plan passed over, with the reason it could not be named.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SkippedOutputDto {
    /// The folder inside the archive that would have become an output.
    /// Empty when the whole input was the candidate.
    pub root: String,
    pub reason: String,
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
            output_name: rule.actions.output_name.clone(),
            layout: layout_to_dto(&rule.actions.layout),
        },
    }
}

/// A stored layout as the facade reports it. Total: every core shape has
/// a DTO counterpart, so a summary never has to omit part of a layout it
/// cannot describe.
fn layout_to_dto(layout: &Layout) -> LayoutDto {
    LayoutDto {
        outputs: match &layout.outputs {
            OutputSelector::Whole => OutputSelectorDto::Whole,
            OutputSelector::PerDirectoryContaining { marker } => {
                OutputSelectorDto::PerDirectoryContaining {
                    marker: marker.clone(),
                }
            }
        },
        file_variables: layout
            .file_variables
            .iter()
            .map(|variable| FileVariableDto {
                as_name: variable.as_name.clone(),
                file: variable.file.clone(),
                key: variable.key.clone(),
            })
            .collect(),
        name: layout.name.clone(),
        place: layout
            .place
            .iter()
            .map(|placement| PlacementDto {
                from: match &placement.from {
                    Source::All => PlacementSourceDto::All,
                    Source::Matching(glob) => PlacementSourceDto::Matching(glob.clone()),
                    Source::ContentRoot => PlacementSourceDto::ContentRoot,
                },
                into: placement.into.clone(),
            })
            .collect(),
        generate: layout
            .generate
            .iter()
            .map(|generated| GeneratedFileDto {
                into: generated.into.clone(),
                content: match generated.content {
                    GeneratedContent::MetadataDocument => GeneratedContentDto::MetadataDocument,
                },
            })
            .collect(),
        fetch: layout
            .fetch
            .iter()
            .map(|fetched| FetchedFileDto {
                into: fetched.into.clone(),
                source: match fetched.source {
                    FetchSource::Screenshots => FetchSourceDto::Screenshots,
                },
                name: fetched.name.clone(),
            })
            .collect(),
    }
}

/// The reverse of [`layout_to_dto`], so an untouched rule loaded into an
/// editor and saved again stores exactly what it started with.
fn layout_to_core(layout: &LayoutDto) -> Layout {
    Layout {
        outputs: match &layout.outputs {
            OutputSelectorDto::Whole => OutputSelector::Whole,
            OutputSelectorDto::PerDirectoryContaining { marker } => {
                OutputSelector::PerDirectoryContaining {
                    marker: marker.clone(),
                }
            }
        },
        file_variables: layout
            .file_variables
            .iter()
            .map(|variable| FileVariable {
                as_name: variable.as_name.clone(),
                file: variable.file.clone(),
                key: variable.key.clone(),
            })
            .collect(),
        name: layout.name.clone(),
        place: layout
            .place
            .iter()
            .map(|placement| Placement {
                from: match &placement.from {
                    PlacementSourceDto::All => Source::All,
                    PlacementSourceDto::Matching(glob) => Source::Matching(glob.clone()),
                    PlacementSourceDto::ContentRoot => Source::ContentRoot,
                },
                into: placement.into.clone(),
            })
            .collect(),
        generate: layout
            .generate
            .iter()
            .map(|generated| Generated {
                into: generated.into.clone(),
                content: match generated.content {
                    GeneratedContentDto::MetadataDocument => GeneratedContent::MetadataDocument,
                },
            })
            .collect(),
        fetch: layout
            .fetch
            .iter()
            .map(|fetched| Fetched {
                into: fetched.into.clone(),
                source: match fetched.source {
                    FetchSourceDto::Screenshots => FetchSource::Screenshots,
                },
                name: fetched.name.clone(),
            })
            .collect(),
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
    // A glob nothing can match is the layout equivalent of the empty
    // move pattern this used to reject: the placement claims no file,
    // silently, and the files it was meant to carry fall through to
    // whatever comes after it.
    for placement in &input.actions.layout.place {
        if let PlacementSourceDto::Matching(glob) = &placement.from {
            if glob.trim().is_empty() {
                return Err(invalid_input_error(
                    "actions.layout.place.from",
                    "a matching placement's glob must not be empty",
                ));
            }
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
            output_name: input.actions.output_name.clone(),
            layout: layout_to_core(&input.actions.layout),
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
                output_name: None,
                layout: LayoutDto {
                    outputs: OutputSelectorDto::Whole,
                    file_variables: vec![FileVariableDto {
                        as_name: "mod_name".to_string(),
                        file: "modinfo.ini".to_string(),
                        key: "name".to_string(),
                    }],
                    name: "[$product_id] $title".to_string(),
                    place: vec![PlacementDto {
                        from: PlacementSourceDto::Matching("*.exe".to_string()),
                        into: "bin".to_string(),
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
                },
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

    /// A glob nothing can match claims no file and says nothing about
    /// it, so the placement that was meant to route those files is
    /// simply absent from the result.
    #[test]
    fn an_empty_placement_glob_is_rejected() {
        let mut input = rule_input();
        input.actions.layout.place[0].from = PlacementSourceDto::Matching(String::new());
        let error = rule_to_core(&input, 0).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("actions.layout.place.from"));
    }

    /// Every shape the layout vocabulary can express survives the trip
    /// out to a frontend and back, so an editor that round-trips a rule
    /// it did not touch cannot silently flatten part of its layout.
    #[test]
    fn every_layout_shape_survives_the_trip_through_core() {
        let mut input = rule_input();
        input.actions.layout.outputs = OutputSelectorDto::PerDirectoryContaining {
            marker: "modinfo.ini".to_string(),
        };
        input.actions.layout.place = vec![
            PlacementDto {
                from: PlacementSourceDto::ContentRoot,
                into: "Game".to_string(),
            },
            PlacementDto {
                from: PlacementSourceDto::Matching("*.txt".to_string()),
                into: "docs".to_string(),
            },
            PlacementDto {
                from: PlacementSourceDto::All,
                into: String::new(),
            },
        ];

        let core = rule_to_core(&input, 1).expect("valid rule input");
        assert_eq!(summarize_rule(&core).actions, input.actions);
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

    // ── format options ──────────────────────────────────────────────────

    /// The whole point of the enumeration: every token it offers must be
    /// a token [`profile_to_core`] accepts, and every method it lists for
    /// a format must be accepted *for that format* -- otherwise a picker
    /// built from it can compose a profile this facade then rejects.
    #[test]
    fn every_offered_format_and_method_is_one_a_profile_input_accepts() {
        let options = archive_format_options();
        assert!(!options.is_empty());

        for option in &options {
            assert!(
                option
                    .compression_methods
                    .contains(&option.default_compression_method),
                "a format's default method must be one it offers"
            );
            for method in &option.compression_methods {
                let mut input = profile_input();
                input.output_format = option.token.clone();
                input.compression_method = Some(method.clone());
                let core = profile_to_core(&input, 0, false)
                    .unwrap_or_else(|_| panic!("{} / {method} must be accepted", option.token));
                assert_eq!(core.format.as_str(), option.token);
                assert_eq!(core.compression_method.as_deref(), Some(method.as_str()));
                assert_eq!(core.format.extension(), option.extension);
            }
        }
    }

    /// The capability bits are core's answer, not a second copy of it:
    /// a format core teaches to support solid blocks must show up here
    /// without this crate being edited.
    #[test]
    fn format_options_report_the_capabilities_core_reports() {
        for option in archive_format_options() {
            let format = ArchiveFormat::all()
                .iter()
                .find(|format| format.as_str() == option.token)
                .expect("every offered token must name a real format");
            assert_eq!(
                option.supports_solid_archive,
                format.supports_solid_archive(),
                "{} solid-archive support",
                option.token
            );
            assert_eq!(
                option.supports_header_encryption,
                format.supports_header_encryption(),
                "{} header-encryption support",
                option.token
            );
        }
    }

    #[test]
    fn format_options_round_trip_through_json() {
        for option in archive_format_options() {
            assert_eq!(round_trip(&option), option);
        }
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
            round_trip(&summary.actions.layout),
            summary.actions.layout,
            "a layout is the half of a rule a bridge is most likely to \
             drop: it is the only nested, enum-bearing shape here"
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
            outputs: vec![PlannedOutputDto {
                root_folder: "[RJ123456] Placeholder Game".to_string(),
                root_folder_template: "[$product_id] $title".to_string(),
                moves: vec![PlannedMoveDto {
                    source: "inner/game.exe".to_string(),
                    destination: "[RJ123456] Placeholder Game/Game/game.exe".to_string(),
                }],
                generated_files: vec!["[RJ123456] Placeholder Game/metadata.json".to_string()],
                downloads: vec![PlannedDownloadDto {
                    destination: "[RJ123456] Placeholder Game/screenshots/0.jpg".to_string(),
                    cached: true,
                }],
                resolved_variables: vec![ResolvedVariableDto {
                    name: "product_id".to_string(),
                    value: "RJ123456".to_string(),
                }],
                reasoning: vec!["the content root is inner, on 2 indicators".to_string()],
            }],
            skipped_outputs: vec![SkippedOutputDto {
                root: "extras".to_string(),
                reason: "$title was not set".to_string(),
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

        let output = &preview.outputs[0];
        assert_eq!(round_trip(&preview), preview);
        assert_eq!(round_trip(output), *output);
        assert_eq!(round_trip(&output.moves[0]), output.moves[0]);
        assert_eq!(round_trip(&output.downloads[0]), output.downloads[0]);
        assert_eq!(
            round_trip(&output.resolved_variables[0]),
            output.resolved_variables[0]
        );
        assert_eq!(
            round_trip(&preview.skipped_outputs[0]),
            preview.skipped_outputs[0]
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
