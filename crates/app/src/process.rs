//! The Process page's frontend-neutral surface: saved pipeline-preset
//! CRUD, the synchronous pipeline preview a step editor recomputes as
//! the user types, and the interrupted-prior-runs query that backs its
//! "previous runs were interrupted" banner.
//!
//! This module holds the DTOs plus the pure validation/conversion logic
//! over them; [`crate::runtime::process_ops`] holds the
//! `AppRuntime`-touching execution layer, and `crate::runtime`'s own
//! `impl ArclainApp` exposes the thin dispatch wrappers -- the same
//! three-layer split [`crate::organization`]/`runtime::organization_ops`
//! already uses for the organization surface.
//!
//! Not to be confused with `crate::runtime::processing_ops`, whose
//! similar name covers a different job: that module runs the
//! Convert/Organize/Pipeline *operations* (registered, cancellable,
//! event-broadcasting). Nothing in this module registers an operation.
//! The two do share one thing on purpose -- the saved presets file, read
//! through the single [`crate::AppPaths::presets_file`] resolution, so
//! the presets this surface lists are exactly the ones `start_pipeline`
//! can run.
//!
//! ## Three surfaces, one page
//!
//! - **Presets** ([`PipelinePresetSummary`]/[`PipelinePresetInput`]) are
//!   named, saved pipelines stored as JSON in the config directory --
//!   *not* in the configuration database, unlike organization rules and
//!   profiles. See [`crate::AppPaths::presets_file`].
//! - **Preview** ([`PipelinePreviewRequest`] ->
//!   [`PipelinePreviewDto`]) answers "what would this pipeline do?"
//!   without doing any of it. Recomputed at editing frequency, so it
//!   never enters the operation registry -- see
//!   [`crate::ArclainApp::preview_pipeline`]. It describes its inputs
//!   with [`PipelineInputsDto`], the very type
//!   [`crate::operations::PipelineRequest`] takes, and takes no other
//!   parameter of its own: everything that shapes the prediction is
//!   something the run is given too. That equality is enforced by the
//!   compiler, not by prose -- see
//!   `impl From<PipelinePreviewRequest> for PipelineRequest`.
//! - **Interrupted runs** ([`InterruptedPipelineRunDto`]) are
//!   database-persisted across restarts and are emphatically *not*
//!   [`crate::ArclainApp::recent_operations`], which is in-memory and
//!   per-process. See [`crate::ArclainApp::interrupted_pipeline_runs`]
//!   for the full semantics, including the fact that nothing ever clears
//!   them.
//!
//! ## Preset identity is the name
//!
//! A saved preset has no row id: the storage is a JSON array of
//! `{ name, pipeline }` records and every consumer -- this facade's own
//! [`crate::operations::pipeline::PipelineSpecDto::Preset`] included --
//! addresses one by name. So [`PipelinePresetSummary::name`] can be
//! handed straight back as `PipelineSpecDto::Preset { id }`, and the
//! save path treats the name as a unique key (see
//! [`PipelinePresetInput`]).

use std::path::PathBuf;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::operations::pipeline::{
    OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto, PipelineInputsDto,
    PipelineRequest, PipelineSpecDto, PipelineStepDto,
};

// ============================================================================
// Preset DTOs.
// ============================================================================

/// One saved pipeline preset.
///
/// Mirrors `arclain_core::SavedPreset` (a `name` plus an
/// `arclain_core::Pipeline`) through the same DTO vocabulary
/// [`crate::operations::PipelineRequest`] already speaks, so a preset a
/// caller lists, edits and saves back is expressed in exactly the terms
/// `start_pipeline` accepts -- there is no second, drifting description
/// of what a pipeline is.
///
/// ## The one field deliberately not here: `input`
///
/// `arclain_core::Pipeline` also carries an `input:
/// Option<PipelineInput>`, and the pre-facade Process page stored
/// whatever files the user happened to have selected into every preset
/// it saved. Nothing has ever read it back: applying a preset in that
/// page explicitly preserved the *current* input and overwrote
/// everything else, and `start_pipeline` replaces `.input` per file. So
/// the field was pure dead weight that also wrote the user's local file
/// paths into a config file. This DTO omits it, and a save through this
/// facade stores `input: None`. Reading an older presets file that has
/// one still works -- the field is simply not surfaced.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PipelinePresetSummary {
    /// Hand this straight to
    /// [`crate::operations::pipeline::PipelineSpecDto::Preset::id`].
    pub name: String,
    pub steps: Vec<PipelineStepDto>,
    /// Where the preset's own stored output location points. Note that
    /// [`crate::ArclainApp::start_pipeline`] always overrides this with
    /// [`crate::operations::PipelineRequest::destination`] -- the stored
    /// value is what a frontend pre-fills its destination picker with
    /// when the preset is applied, not what the run uses.
    pub destination: PipelineDestinationDto,
    /// `None` means "inherit the application-wide
    /// `default_collision_policy` setting" -- honoured by both the run
    /// and [`crate::ArclainApp::preview_pipeline`], which completes that
    /// ladder itself rather than leaving `arclain_core`'s preview to
    /// assume `Smart`.
    pub collision_policy: Option<OutputCollisionPolicyDto>,
    pub output_artifact: OutputArtifactDto,
    /// Whether this entry is one of the presets the application ships,
    /// unmodified: its name and every field this DTO carries match a
    /// built-in exactly.
    ///
    /// **Descriptive, not protective.** It confers no immunity: built-ins
    /// can be edited, renamed, shadowed and deleted like any other
    /// preset, and the change is permanent (see
    /// [`crate::ArclainApp::pipeline_presets`] for why). Editing a
    /// built-in flips this to `false` for that entry, because it is then
    /// no longer the shipped pipeline; saving a preset that happens to
    /// match a shipped one exactly reports `true`, because it then is.
    pub builtin: bool,
}

/// A preset create/update request.
///
/// The name is the key: saving under a name that already exists
/// **replaces that preset in place**, preserving its position in the
/// list, rather than appending a second entry with the same name. The
/// pre-facade Save button appended unconditionally, and its
/// minute-resolution default name made same-name duplicates reachable by
/// clicking Save twice -- at which point the dropdown showed two
/// identical rows, applying one always picked the first, and deleting
/// one deleted both. Name-keyed upsert is the same convention
/// [`crate::settings::PasswordRuleInput`] and
/// [`crate::organization::OrganizationRuleInput`] already use, and it
/// matches how a preset is *resolved* for a run
/// (`PipelineSpecDto::Preset` takes the first match by name).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PipelinePresetInput {
    /// Stored trimmed. Rejected as
    /// [`ApplicationErrorKind::InvalidInput`] when it is empty or only
    /// whitespace: a preset with no name can neither be selected nor
    /// deleted again.
    pub name: String,
    /// Rejected when empty, for the same reason
    /// [`crate::operations::PipelineRequest::validate`] rejects an empty
    /// ad-hoc step list -- and one more specific to storage: `validate`
    /// only checks the ad-hoc branch, so a stored empty preset would
    /// slip past it and run as a bare repack. Every step is validated
    /// here too, so a preset cannot store an unparseable rule id or
    /// format that only fails much later, on the first run that uses it.
    pub steps: Vec<PipelineStepDto>,
    pub destination: PipelineDestinationDto,
    pub collision_policy: Option<OutputCollisionPolicyDto>,
    pub output_artifact: OutputArtifactDto,
}

// ============================================================================
// Preview DTOs.
// ============================================================================

/// One pipeline preview request.
///
/// Field-for-field the same run description
/// [`crate::operations::PipelineRequest`] carries -- the same
/// [`PipelineInputsDto`], `destination`, [`PipelineSpecDto`] and
/// `collision_policy`. A caller hands the one description to both, and
/// there is deliberately **nothing else on this type**: every input to
/// the predicted result is an input to the run.
///
/// In particular there is no metadata parameter. The names in a
/// predicted output path come from `arclain_core`'s `stem_from`, and
/// [`crate::ArclainApp::preview_pipeline`] resolves the metadata half of
/// that *per input*, through the very lookup the executor performs -- so
/// this type cannot be told to predict names the run will not produce.
/// See that method's doc comment for what a caller-supplied-metadata
/// shape got wrong, and why it was wrong specifically for a batch.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PipelinePreviewRequest {
    pub inputs: PipelineInputsDto,
    pub destination: PipelineDestinationDto,
    /// [`PipelineSpecDto::Steps`] is the interaction-frequency case: it
    /// is pure, in-memory, and is what a step editor holds. A
    /// [`PipelineSpecDto::Preset`] additionally reads the presets file,
    /// so it is a fine way to preview a preset a user just selected, but
    /// not something to recompute on every keystroke.
    pub pipeline: PipelineSpecDto,
    /// `None` defers to the resolved pipeline's own stored policy, then
    /// to the application-wide `default_collision_policy` setting, then
    /// to `Smart` -- the identical ladder
    /// [`crate::ArclainApp::start_pipeline`] resolves, materialized by
    /// the preview so the collision warning describes the run's real
    /// policy rather than a hardcoded one.
    pub collision_policy: Option<OutputCollisionPolicyDto>,
}

/// Runs what was previewed: turns a preview request into the
/// [`PipelineRequest`] describing the identical run.
///
/// # This conversion is the enforcement, not a convenience
///
/// The property this whole surface rests on is that
/// [`PipelinePreviewRequest`] is *exactly*
/// [`PipelineRequest`]'s field set. Prose cannot hold that: a field added
/// to one type and not the other would silently reopen the
/// preview-describes-something-else divergence, and no test would notice
/// because no test can know about a field it was not written to check.
///
/// This function makes the compiler notice instead. It **destructures**
/// the preview request by naming every field, and **constructs** the run
/// request by naming every field:
///
/// * a new field on [`PipelinePreviewRequest`] fails the pattern
///   (`E0027`, "pattern does not mention field");
/// * a new field on [`PipelineRequest`] fails the literal (`E0063`,
///   "missing field in initializer").
///
/// Either way the build stops until both types agree again. **Do not add
/// `..` to either side** -- a rest pattern or a struct-update base would
/// make both errors disappear and turn the guarantee back into a comment.
///
/// Being useful is a bonus: a frontend that previews continuously and
/// then runs already holds the preview request, so this is the natural
/// hand-off. Note that a preview accepts two things a run refuses -- no
/// inputs and no steps -- so the result still has to pass
/// [`crate::ArclainApp::start_pipeline`]'s own validation.
impl From<PipelinePreviewRequest> for PipelineRequest {
    fn from(preview: PipelinePreviewRequest) -> Self {
        let PipelinePreviewRequest {
            inputs,
            destination,
            pipeline,
            collision_policy,
        } = preview;
        Self {
            inputs,
            destination,
            pipeline,
            collision_policy,
        }
    }
}

/// What the pipeline would do to one input file. Mirrors
/// `arclain_core::PreviewEntry` field for field.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PipelinePreviewEntryDto {
    pub input: PathBuf,
    /// Human-readable, one per pipeline step, in execution order --
    /// `arclain_core`'s own strings, carried verbatim so a frontend
    /// renders what the pre-facade panel rendered. Descriptive text, not
    /// a machine-readable plan: a caller that needs structure has it
    /// already, in the [`PipelineStepDto`] list it submitted.
    pub operations: Vec<String>,
    /// Where the run is predicted to write. `None` when the pipeline
    /// produces an archive but no step chose a format -- see
    /// [`crate::operations::pipeline::OutputArtifactDto`] for why that is
    /// a preview gap rather than a run failure (the executor falls back
    /// to zip).
    pub expected_output: Option<PathBuf>,
    /// Per-input warnings, today only the output-already-exists notice
    /// and the collision policy's consequence for it.
    pub warnings: Vec<String>,
}

/// A whole pipeline preview. Mirrors `arclain_core::PipelinePreview`.
///
/// Purely descriptive: computing this writes nothing, reads no archive
/// bytes, and registers no operation.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PipelinePreviewDto {
    /// One per input, in input order. Empty when the pipeline has no
    /// input at all, or when a folder input turned up no archives (in
    /// which case [`Self::global_warnings`] says so).
    pub entries: Vec<PipelinePreviewEntryDto>,
    /// Warnings about the pipeline as a whole rather than one input --
    /// an empty step list, or a folder that yielded nothing.
    pub global_warnings: Vec<String>,
}

// ============================================================================
// Interrupted-run DTO.
// ============================================================================

/// One pipeline run a previous process started and never finished.
///
/// A subset of the stored `pipeline_runs` row: what a banner or a
/// recovery list would show, without the content hashes and internal
/// dedup keys nothing outside `arclain_core` has a use for.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct InterruptedPipelineRunDto {
    /// The archive the interrupted run was processing.
    pub input_path: PathBuf,
    /// When the run started, in Unix seconds.
    pub started_at_unix: i64,
    /// When the run was *declared* interrupted, in Unix seconds -- the
    /// startup sweep's own clock, not the moment the process actually
    /// died (which nothing records). This is the value
    /// [`crate::ArclainApp::interrupted_pipeline_runs`]'s `since_unix`
    /// filters on.
    pub interrupted_at_unix: i64,
    /// Which Arclain version started the run.
    pub arclain_version: String,
}

// ============================================================================
// Pure DTO <-> domain conversions.
// ============================================================================

/// Maps a stored preset into its DTO with `builtin` set to `builtin`.
fn to_summary(preset: &arclain_core::SavedPreset, builtin: bool) -> PipelinePresetSummary {
    PipelinePresetSummary {
        name: preset.name.clone(),
        steps: preset
            .pipeline
            .steps
            .iter()
            .map(PipelineStepDto::from_core)
            .collect(),
        destination: PipelineDestinationDto::from_core(&preset.pipeline.output),
        collision_policy: preset
            .pipeline
            .collision_policy
            .map(OutputCollisionPolicyDto::from_core),
        output_artifact: OutputArtifactDto::from_core(preset.pipeline.output_artifact),
        builtin,
    }
}

/// Maps one stored preset into its DTO, deciding
/// [`PipelinePresetSummary::builtin`] by comparing it against
/// `builtins` (from [`builtin_preset_summaries`]).
///
/// The comparison is the derived whole-struct equality against a probe
/// that already claims `builtin: true`, rather than a hand-written list
/// of fields: a field added to the DTO later then participates
/// automatically instead of being silently left out of the check. It is
/// therefore a comparison in *DTO space* -- a stored preset differing
/// only in the `input` this DTO deliberately drops (see
/// [`PipelinePresetSummary`]) is still, in every way a caller can
/// observe, the shipped pipeline.
pub(crate) fn summarize_preset(
    preset: &arclain_core::SavedPreset,
    builtins: &[PipelinePresetSummary],
) -> PipelinePresetSummary {
    let probe = to_summary(preset, true);
    let builtin = builtins.contains(&probe);
    PipelinePresetSummary { builtin, ..probe }
}

/// The shipped presets, as DTOs, with `builtin` already `true` -- the
/// reference set [`summarize_preset`] compares against, and the list
/// itself when no presets file exists yet.
pub(crate) fn builtin_preset_summaries() -> Vec<PipelinePresetSummary> {
    arclain_core::builtin_presets()
        .iter()
        .map(|preset| to_summary(preset, true))
        .collect()
}

/// Validates `input` and, on success, builds the core preset to persist.
///
/// `input: None` on the stored pipeline is this facade's choice, not a
/// dropped field -- see [`PipelinePresetSummary`]'s own doc comment.
pub(crate) fn preset_to_core(
    input: &PipelinePresetInput,
) -> Result<arclain_core::SavedPreset, ApplicationError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid_input_error("name", "preset name must not be empty"));
    }
    if input.steps.is_empty() {
        return Err(invalid_input_error(
            "steps",
            "a preset needs at least one step",
        ));
    }
    let steps = input
        .steps
        .iter()
        .map(PipelineStepDto::to_core)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(arclain_core::SavedPreset {
        name: name.to_string(),
        pipeline: arclain_core::Pipeline {
            input: None,
            steps,
            output: input.destination.to_core(),
            collision_policy: input
                .collision_policy
                .map(OutputCollisionPolicyDto::to_core),
            output_artifact: input.output_artifact.to_core(),
        },
    })
}

/// Clears every stored `PipelineStep::Convert::password` in `presets`,
/// reporting whether anything was actually cleared.
///
/// ## Why this exists, and why it runs on the *write* paths
///
/// `arclain_core::PipelineStep::Convert` carries a `password:
/// Option<String>` that the pipeline executor never reads -- it binds
/// the field (`executor.rs`'s `Convert` arm: `password: _`) and
/// discards it. A pre-facade Process page nonetheless offered a
/// password text field for it, so a user who typed one had the plain
/// secret serialized into `pipeline_presets.json` in the configuration
/// directory, where it does nothing but sit.
///
/// [`PipelineStepDto`] has no counterpart field, so a preset *rewritten*
/// through this facade loses it. That alone is not enough: both write
/// paths are read-modify-write over the whole file, so saving preset A
/// re-serializes preset B verbatim -- actively re-persisting B's secret
/// on every unrelated save. This makes any write that touches the file
/// clear the whole file's secrets, which is the only point at which the
/// residue actually stops being rewritten.
///
/// **Deliberately not done on load.** `arclain_core::load_presets`
/// collapses *missing*, *unreadable* and *unparseable* into one answer
/// (the built-ins), so a rewrite driven off its return value would
/// silently overwrite a corrupt-but-hand-recoverable presets file with
/// the two shipped defaults, destroying every preset the user has. The
/// write paths are safe because the user asked for a write: the file is
/// being replaced either way, and this only changes what is put in it.
pub(crate) fn strip_stored_step_secrets(presets: &mut [arclain_core::SavedPreset]) -> bool {
    let mut stripped = false;
    for preset in presets.iter_mut() {
        for step in preset.pipeline.steps.iter_mut() {
            if let arclain_core::PipelineStep::Convert { password, .. } = step {
                if password.take().is_some() {
                    stripped = true;
                }
            }
        }
    }
    stripped
}

/// Maps one `arclain_core::PreviewEntry`. Entry-level rather than
/// whole-preview because [`crate::runtime::process_ops`] assembles the
/// result from one core preview *per input* -- see
/// [`crate::ArclainApp::preview_pipeline`] for why the loop lives there.
pub(crate) fn preview_entry_to_dto(entry: arclain_core::PreviewEntry) -> PipelinePreviewEntryDto {
    PipelinePreviewEntryDto {
        input: entry.input,
        operations: entry.operations,
        expected_output: entry.expected_output,
        warnings: entry.warnings,
    }
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
    use crate::operations::pipeline::CompressionLevelDto;

    fn preset_input() -> PipelinePresetInput {
        PipelinePresetInput {
            name: "Flatten then zip".to_string(),
            steps: vec![
                PipelineStepDto::Flatten {
                    strip_common_prefix: true,
                    max_depth: 0,
                },
                PipelineStepDto::Convert {
                    format: "zip".to_string(),
                    compression: CompressionLevelDto::Normal,
                },
            ],
            destination: PipelineDestinationDto::Folder {
                path: PathBuf::from("/out"),
            },
            collision_policy: Some(OutputCollisionPolicyDto::Overwrite),
            output_artifact: OutputArtifactDto::Archive,
        }
    }

    // ── preset conversion/validation ────────────────────────────────────

    #[test]
    fn a_preset_round_trips_through_core_and_back() {
        let input = preset_input();
        let core = preset_to_core(&input).expect("valid preset input");
        let summary = summarize_preset(&core, &builtin_preset_summaries());

        assert_eq!(summary.name, input.name);
        assert_eq!(summary.steps, input.steps);
        assert_eq!(summary.destination, input.destination);
        assert_eq!(summary.collision_policy, input.collision_policy);
        assert_eq!(summary.output_artifact, input.output_artifact);
        assert!(!summary.builtin);
    }

    /// The stored pipeline never carries an input, so a preset cannot
    /// smuggle the user's local file paths into a config file (see
    /// `PipelinePresetSummary`'s own doc comment).
    #[test]
    fn a_saved_preset_stores_no_input() {
        let core = preset_to_core(&preset_input()).unwrap();
        assert!(core.pipeline.input.is_none());
    }

    #[test]
    fn a_preset_name_is_trimmed_and_a_blank_one_is_rejected() {
        let mut input = preset_input();
        input.name = "  Spaced  ".to_string();
        assert_eq!(preset_to_core(&input).unwrap().name, "Spaced");

        input.name = "   ".to_string();
        let error = preset_to_core(&input).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("name"));
    }

    /// `PipelineRequest::validate` only rejects an empty *ad-hoc* step
    /// list, so without this check a stored empty preset would run
    /// through `start_pipeline` as a bare repack rather than being
    /// refused.
    #[test]
    fn a_preset_with_no_steps_is_rejected() {
        let mut input = preset_input();
        input.steps.clear();
        let error = preset_to_core(&input).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("steps"));
    }

    #[test]
    fn a_preset_step_that_cannot_run_is_rejected_at_save_time() {
        let mut input = preset_input();
        input.steps = vec![PipelineStepDto::Convert {
            format: "rar".to_string(),
            compression: CompressionLevelDto::Normal,
        }];
        let error = preset_to_core(&input).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("format"));

        input.steps = vec![PipelineStepDto::Organize {
            rule_id: "not-a-number".to_string(),
        }];
        let error = preset_to_core(&input).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("rule_id"));
    }

    // ── stored-secret stripping ─────────────────────────────────────────

    fn legacy_preset_with_password(name: &str, password: &str) -> arclain_core::SavedPreset {
        arclain_core::SavedPreset {
            name: name.to_string(),
            pipeline: arclain_core::Pipeline {
                input: None,
                steps: vec![
                    arclain_core::PipelineStep::Flatten {
                        strip_common_prefix: true,
                        max_depth: 1,
                    },
                    arclain_core::PipelineStep::Convert {
                        format: arclain_core::ConvertFormat::Zip,
                        compression: arclain_core::CompressionLevel::Normal,
                        password: Some(password.to_string()),
                    },
                ],
                output: arclain_core::PipelineOutput::SameFolder,
                collision_policy: None,
                output_artifact: arclain_core::OutputArtifact::Archive,
            },
        }
    }

    fn stored_passwords(presets: &[arclain_core::SavedPreset]) -> Vec<Option<String>> {
        presets
            .iter()
            .flat_map(|preset| &preset.pipeline.steps)
            .filter_map(|step| match step {
                arclain_core::PipelineStep::Convert { password, .. } => Some(password.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn stripping_clears_every_stored_convert_password_and_nothing_else() {
        let mut presets = vec![
            legacy_preset_with_password("Legacy A", "hunter2"),
            legacy_preset_with_password("Legacy B", "correct horse"),
        ];
        let before_steps: Vec<usize> = presets
            .iter()
            .map(|preset| preset.pipeline.steps.len())
            .collect();

        assert!(strip_stored_step_secrets(&mut presets));
        assert_eq!(stored_passwords(&presets), vec![None, None]);

        // The steps themselves are untouched -- this clears a field, it
        // does not rewrite the pipeline.
        let after_steps: Vec<usize> = presets
            .iter()
            .map(|preset| preset.pipeline.steps.len())
            .collect();
        assert_eq!(after_steps, before_steps);
        assert!(matches!(
            presets[0].pipeline.steps[0],
            arclain_core::PipelineStep::Flatten {
                strip_common_prefix: true,
                max_depth: 1
            }
        ));
        assert_eq!(presets[0].name, "Legacy A");
    }

    /// Reports `false` when there was nothing to clear, so the write
    /// paths can log only when a secret was genuinely removed.
    #[test]
    fn stripping_a_clean_preset_list_reports_no_change() {
        let mut presets = vec![preset_to_core(&preset_input()).unwrap()];
        assert!(!strip_stored_step_secrets(&mut presets));
        assert_eq!(stored_passwords(&presets), vec![None]);

        assert!(!strip_stored_step_secrets(&mut Vec::new()));
    }

    // ── built-in detection ──────────────────────────────────────────────

    /// Every shipped preset must report `builtin: true` when read back
    /// unchanged -- otherwise the flag says nothing.
    #[test]
    fn the_shipped_presets_are_recognized_as_builtin() {
        let builtins = builtin_preset_summaries();
        assert!(!builtins.is_empty());
        for preset in arclain_core::builtin_presets() {
            let summary = summarize_preset(&preset, &builtins);
            assert!(
                summary.builtin,
                "{} must be recognized as a shipped preset",
                preset.name
            );
        }
    }

    /// The flag describes the *pipeline*, not just the name: a user who
    /// edits a built-in and keeps its name no longer has the shipped
    /// pipeline, and the flag must say so.
    #[test]
    fn editing_a_builtin_stops_it_being_reported_as_builtin() {
        let builtins = builtin_preset_summaries();
        let mut edited = arclain_core::builtin_presets()
            .into_iter()
            .next()
            .expect("at least one shipped preset");
        edited
            .pipeline
            .steps
            .push(arclain_core::PipelineStep::Flatten {
                strip_common_prefix: false,
                max_depth: 3,
            });
        assert!(!summarize_preset(&edited, &builtins).builtin);
    }

    /// The mirror image: a user preset that happens to be exactly a
    /// shipped pipeline under a different name is not the built-in.
    #[test]
    fn a_renamed_copy_of_a_builtin_is_not_reported_as_builtin() {
        let builtins = builtin_preset_summaries();
        let mut renamed = arclain_core::builtin_presets()
            .into_iter()
            .next()
            .expect("at least one shipped preset");
        renamed.name = "My copy".to_string();
        assert!(!summarize_preset(&renamed, &builtins).builtin);
    }

    /// `input` is the one core field the DTO drops, so it must not
    /// participate in the comparison -- an older presets file that
    /// stashed a file list into a shipped preset still holds the shipped
    /// pipeline.
    #[test]
    fn a_stashed_input_does_not_stop_a_builtin_being_recognized() {
        let builtins = builtin_preset_summaries();
        let mut legacy = arclain_core::builtin_presets()
            .into_iter()
            .next()
            .expect("at least one shipped preset");
        legacy.pipeline.input = Some(arclain_core::PipelineInput::Files(vec![PathBuf::from(
            "/somewhere/RJ123456.zip",
        )]));
        assert!(summarize_preset(&legacy, &builtins).builtin);
    }

    // ── preview conversion ──────────────────────────────────────────────

    /// The conversion's *value* behaviour. Its real job -- failing the
    /// build when the two request types stop agreeing -- is done by the
    /// compiler and cannot be asserted from here; see the `impl`'s own
    /// doc comment. This pins that it moves every field across
    /// unchanged, so the run really is the previewed one.
    #[test]
    fn a_preview_request_converts_into_the_identical_run_request() {
        let preview = PipelinePreviewRequest {
            inputs: PipelineInputsDto::Folder {
                path: PathBuf::from("/batch"),
            },
            destination: PipelineDestinationDto::Folder {
                path: PathBuf::from("/out"),
            },
            pipeline: PipelineSpecDto::Steps {
                steps: preset_input().steps,
                output_artifact: OutputArtifactDto::Folder,
            },
            collision_policy: Some(OutputCollisionPolicyDto::Overwrite),
        };
        let run = PipelineRequest::from(preview.clone());

        assert_eq!(run.inputs, preview.inputs);
        assert_eq!(run.destination, preview.destination);
        assert_eq!(run.pipeline, preview.pipeline);
        assert_eq!(run.collision_policy, preview.collision_policy);
    }

    #[test]
    fn a_core_preview_entry_maps_field_for_field() {
        let core = arclain_core::PreviewEntry {
            input: PathBuf::from("/in/RJ123456.rar"),
            operations: vec!["Convert to .zip".to_string()],
            expected_output: Some(PathBuf::from("/out/RJ123456.zip")),
            warnings: vec!["Output already exists".to_string()],
        };
        let dto = preview_entry_to_dto(core.clone());

        assert_eq!(dto.input, core.input);
        assert_eq!(dto.operations, core.operations);
        assert_eq!(dto.expected_output, core.expected_output);
        assert_eq!(dto.warnings, core.warnings);
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
    fn preset_dtos_round_trip_through_json() {
        let input = preset_input();
        assert_eq!(round_trip(&input), input);

        let summary = summarize_preset(
            &preset_to_core(&input).unwrap(),
            &builtin_preset_summaries(),
        );
        assert_eq!(round_trip(&summary), summary);

        for builtin in builtin_preset_summaries() {
            assert_eq!(round_trip(&builtin), builtin);
        }
    }

    #[test]
    fn preview_dtos_round_trip_through_json() {
        for inputs in [
            PipelineInputsDto::Files {
                paths: vec![PathBuf::from("/a.zip"), PathBuf::from("/b.7z")],
            },
            PipelineInputsDto::Folder {
                path: PathBuf::from("/dir"),
            },
        ] {
            assert_eq!(round_trip(&inputs), inputs);

            let request = PipelinePreviewRequest {
                inputs: inputs.clone(),
                destination: PipelineDestinationDto::SameFolder,
                pipeline: PipelineSpecDto::Steps {
                    steps: preset_input().steps,
                    output_artifact: OutputArtifactDto::Folder,
                },
                collision_policy: Some(OutputCollisionPolicyDto::Skip),
            };
            assert_eq!(round_trip(&request), request);
        }

        let request = PipelinePreviewRequest {
            inputs: PipelineInputsDto::Files { paths: Vec::new() },
            destination: PipelineDestinationDto::Folder {
                path: PathBuf::from("/out"),
            },
            pipeline: PipelineSpecDto::Preset {
                id: "Convert to 7z (Max)".to_string(),
            },
            collision_policy: None,
        };
        assert_eq!(round_trip(&request), request);

        let preview = PipelinePreviewDto {
            entries: vec![PipelinePreviewEntryDto {
                input: PathBuf::from("/in/RJ123456.rar"),
                operations: vec!["Flatten nested archives (recursive)".to_string()],
                expected_output: Some(PathBuf::from("/out/RJ123456.zip")),
                warnings: vec!["Output already exists".to_string()],
            }],
            global_warnings: vec!["No operations added".to_string()],
        };
        assert_eq!(round_trip(&preview), preview);
        assert_eq!(round_trip(&preview.entries[0]), preview.entries[0]);
        assert_eq!(
            round_trip(&PipelinePreviewDto::default()),
            PipelinePreviewDto::default()
        );
    }

    /// The two timestamps are the widest integers this surface reports
    /// and are read straight out of SQLite `INTEGER` columns, so they
    /// must survive the whole `i64` range rather than being narrowed by
    /// a `f64`-backed JSON number.
    #[test]
    fn interrupted_run_dtos_round_trip_through_json() {
        for run in [
            InterruptedPipelineRunDto {
                input_path: PathBuf::from("/mods/RJ123456.rar"),
                started_at_unix: 1_700_000_000,
                interrupted_at_unix: 1_700_003_600,
                arclain_version: "2.1.0".to_string(),
            },
            InterruptedPipelineRunDto {
                input_path: PathBuf::new(),
                started_at_unix: i64::MIN,
                interrupted_at_unix: i64::MAX,
                arclain_version: String::new(),
            },
        ] {
            assert_eq!(round_trip(&run), run);
        }
    }
}
