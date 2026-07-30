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
//!   [`crate::ArclainApp::preview_pipeline`].
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
use crate::ids::ArchiveSessionId;
use crate::operations::pipeline::{
    OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto, PipelineSpecDto,
    PipelineStepDto,
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
    /// `default_collision_policy` setting".
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

/// What a pipeline preview runs over.
///
/// Mirrors `arclain_core::PipelineInput`, which is what the preview
/// itself consumes -- including the `Folder` case, whose expansion (the
/// set of files in that directory `arclain_core` recognizes as archives)
/// is core's own definition and must not be re-derived by a frontend.
///
/// **`Folder` has no counterpart on
/// [`crate::operations::PipelineRequest`] today**: that request takes a
/// plain file list, so `start_pipeline` cannot be handed a directory.
/// The bridge is this preview's own answer -- every
/// [`PipelinePreviewEntryDto::input`] is one expanded file, in order, so
/// a caller that previewed a folder already holds exactly the
/// `Vec<PathBuf>` `start_pipeline` needs.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelinePreviewInputsDto {
    Files {
        paths: Vec<PathBuf>,
    },
    /// Every archive directly inside this directory (not recursive).
    Folder {
        path: PathBuf,
    },
}

impl PipelinePreviewInputsDto {
    pub(crate) fn to_core(&self) -> arclain_core::PipelineInput {
        match self {
            Self::Files { paths } => arclain_core::PipelineInput::Files(paths.clone()),
            Self::Folder { path } => arclain_core::PipelineInput::Folder(path.clone()),
        }
    }
}

/// One pipeline preview request.
///
/// Field-for-field the same run description
/// [`crate::operations::PipelineRequest`] carries -- same `destination`,
/// same [`PipelineSpecDto`], same `collision_policy` -- so a caller can
/// hand one description to both and cannot accidentally preview
/// something other than what it is about to run. The two differences are
/// deliberate and documented: [`Self::inputs`] additionally admits a
/// folder (see [`PipelinePreviewInputsDto`]), and [`Self::metadata`]
/// exists only here.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PipelinePreviewRequest {
    pub inputs: PipelinePreviewInputsDto,
    pub destination: PipelineDestinationDto,
    /// [`PipelineSpecDto::Steps`] is the interaction-frequency case: it
    /// is pure, in-memory, and is what a step editor holds. A
    /// [`PipelineSpecDto::Preset`] additionally reads the presets file,
    /// so it is a fine way to preview a preset a user just selected, but
    /// not something to recompute on every keystroke.
    pub pipeline: PipelineSpecDto,
    pub collision_policy: Option<OutputCollisionPolicyDto>,
    /// Which archive session's plugin-reported metadata names the
    /// outputs, or `None` for no metadata at all.
    ///
    /// **Read this before relying on the predicted output path.** The
    /// preview's output name comes from `arclain_core`'s
    /// `stem_from`: a sanitized metadata title if there is one, else a
    /// product code detected in the input's own file name, else the
    /// input's stem. This field supplies the metadata half -- and it
    /// supplies *one* blob, applied to every input, exactly as the
    /// pre-facade Process page did (it passed the active tab's fetched
    /// `game_metadata`, regardless of which files the pipeline was
    /// pointed at).
    ///
    /// [`crate::ArclainApp::start_pipeline`] resolves metadata from a
    /// **different source**: `arclain_core`'s executor looks each input
    /// up individually in the DLsite library, keyed on a product code
    /// detected in that input's own file name. A session's plugin blob
    /// and the library's row for the same product routinely differ, and
    /// nothing reconciles them -- so a predicted output path here can
    /// legitimately disagree with the path the run writes. That
    /// divergence predates this facade and is mirrored, not
    /// manufactured, here; see
    /// [`crate::ArclainApp::preview_pipeline`]'s own doc comment.
    ///
    /// `NotFound` when the session id names no open session -- an
    /// unknown session is never silently downgraded to "no metadata",
    /// since that would quietly change every predicted output name.
    pub metadata: Option<ArchiveSessionId>,
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

pub(crate) fn preview_to_dto(preview: arclain_core::PipelinePreview) -> PipelinePreviewDto {
    PipelinePreviewDto {
        entries: preview
            .entries
            .into_iter()
            .map(|entry| PipelinePreviewEntryDto {
                input: entry.input,
                operations: entry.operations,
                expected_output: entry.expected_output,
                warnings: entry.warnings,
            })
            .collect(),
        global_warnings: preview.global_warnings,
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

    #[test]
    fn preview_inputs_translate_to_the_matching_core_input() {
        assert_eq!(
            PipelinePreviewInputsDto::Files {
                paths: vec![PathBuf::from("/a.zip")]
            }
            .to_core(),
            arclain_core::PipelineInput::Files(vec![PathBuf::from("/a.zip")])
        );
        assert_eq!(
            PipelinePreviewInputsDto::Folder {
                path: PathBuf::from("/dir")
            }
            .to_core(),
            arclain_core::PipelineInput::Folder(PathBuf::from("/dir"))
        );
    }

    #[test]
    fn a_core_preview_maps_field_for_field() {
        let core = arclain_core::PipelinePreview {
            entries: vec![arclain_core::PreviewEntry {
                input: PathBuf::from("/in/RJ123456.rar"),
                operations: vec!["Convert to .zip".to_string()],
                expected_output: Some(PathBuf::from("/out/RJ123456.zip")),
                warnings: vec!["Output already exists".to_string()],
            }],
            global_warnings: vec!["No operations added".to_string()],
        };
        let dto = preview_to_dto(core.clone());

        assert_eq!(dto.global_warnings, core.global_warnings);
        assert_eq!(dto.entries.len(), 1);
        assert_eq!(dto.entries[0].input, core.entries[0].input);
        assert_eq!(dto.entries[0].operations, core.entries[0].operations);
        assert_eq!(
            dto.entries[0].expected_output,
            core.entries[0].expected_output
        );
        assert_eq!(dto.entries[0].warnings, core.entries[0].warnings);
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
            PipelinePreviewInputsDto::Files {
                paths: vec![PathBuf::from("/a.zip"), PathBuf::from("/b.7z")],
            },
            PipelinePreviewInputsDto::Folder {
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
                metadata: Some(ArchiveSessionId::from_raw(5)),
            };
            assert_eq!(round_trip(&request), request);
        }

        let request = PipelinePreviewRequest {
            inputs: PipelinePreviewInputsDto::Files { paths: Vec::new() },
            destination: PipelineDestinationDto::Folder {
                path: PathBuf::from("/out"),
            },
            pipeline: PipelineSpecDto::Preset {
                id: "Convert to 7z (Max)".to_string(),
            },
            collision_policy: None,
            metadata: None,
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
