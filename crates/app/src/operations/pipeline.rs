//! `PipelineRequest`: the facade request DTO for running either a saved
//! preset or an ad-hoc, Process-page-style step list
//! ([`crate::ArclainApp::start_pipeline`]).
//!
//! ## Amended shape
//!
//! This task's first submission modeled `PipelineRequest` as "run a
//! saved preset only" (`{ inputs, destination: PathBuf, preset_id:
//! String }`), reasoning that the stable, serializable surface a bridge
//! needs is "run this named pipeline", not a step-builder API. The
//! amended contract disagrees: `crates/ui/src/features/process/view.rs`'s
//! Process page lets a user assemble an *ad-hoc* step list interactively
//! (the "+ Flatten"/"+ Organize"/"+ Convert" buttons in
//! `render_pipeline_panel`) and separately choose its own output
//! location (`PipelineOutput::SameFolder` or a picked folder) and
//! per-run collision policy (the "If output exists:" dropdown) --
//! independent of whether a preset was ever loaded. `PipelineSpecDto`
//! now expresses both cases (`Preset { id }` for a saved
//! `arclain_core::SavedPreset`, `Steps { steps }` for an ad-hoc list),
//! and `destination`/`collision_policy` are top-level request fields
//! mirroring exactly what the Process page's UI state already lets a
//! user choose independently of the step list.
//!
//! `PipelineDestinationDto`/`OutputCollisionPolicyDto`/`PipelineStepDto`/
//! `CompressionLevelDto` mirror `arclain_core::{PipelineOutput,
//! OutputCollisionPolicy, PipelineStep, CompressionLevel}` variant-for-
//! variant -- see each type's own doc comment for the one deliberate
//! omission (`PipelineStep::Convert::password`).

use std::path::PathBuf;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};

use super::convert::{empty_inputs_error, parse_convert_format};

/// Mirrors `arclain_core::CompressionLevel` exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionLevelDto {
    Fast,
    Normal,
    Max,
}

impl CompressionLevelDto {
    pub(crate) fn to_core(self) -> arclain_core::CompressionLevel {
        match self {
            Self::Fast => arclain_core::CompressionLevel::Fast,
            Self::Normal => arclain_core::CompressionLevel::Normal,
            Self::Max => arclain_core::CompressionLevel::Max,
        }
    }

    pub(crate) fn from_core(level: arclain_core::CompressionLevel) -> Self {
        match level {
            arclain_core::CompressionLevel::Fast => Self::Fast,
            arclain_core::CompressionLevel::Normal => Self::Normal,
            arclain_core::CompressionLevel::Max => Self::Max,
        }
    }
}

/// Mirrors `arclain_core::PipelineStep` exactly, with one deliberate
/// omission: `PipelineStep::Convert` also carries a `password:
/// Option<String>` field, but `arclain_core`'s own executor never reads
/// it (`crates/core/src/features/pipeline/executor.rs::run_one_inner`'s
/// `Convert` match arm binds it `password: _`) -- it is accepted but
/// inert. Exposing a plain, unencrypted `Option<String>` password field
/// on a `Serialize`/`Deserialize` DTO would be inconsistent with every
/// other secret this facade carries (`crate::challenge::SecretInput`,
/// deliberately never serializable), for a field that does not actually
/// do anything yet. Omitted rather than half-wired; a future task that
/// makes the executor actually encrypt can add it back deliberately.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineStepDto {
    Flatten {
        strip_common_prefix: bool,
        max_depth: u32,
    },
    Organize {
        /// An `arclain_core::OrganizationRule` id, decimal string form
        /// (matching `OrganizeRequest::rule_id`'s own convention).
        rule_id: String,
    },
    Convert {
        format: String,
        compression: CompressionLevelDto,
    },
}

impl PipelineStepDto {
    fn validate(&self) -> Result<(), ApplicationError> {
        match self {
            Self::Flatten { .. } => Ok(()),
            Self::Organize { rule_id } => parse_step_rule_id(rule_id).map(|_| ()),
            Self::Convert { format, .. } => parse_convert_format(format).map(|_| ()),
        }
    }

    /// Translates to the `arclain_core` step this DTO mirrors. Assumes
    /// [`Self::validate`] already succeeded -- called only after
    /// [`PipelineRequest::validate`] has, so the `.expect`-free `?`
    /// propagation here can never actually observe the parse failures
    /// `validate` already would have caught first.
    pub(crate) fn to_core(&self) -> Result<arclain_core::PipelineStep, ApplicationError> {
        match self {
            Self::Flatten {
                strip_common_prefix,
                max_depth,
            } => Ok(arclain_core::PipelineStep::Flatten {
                strip_common_prefix: *strip_common_prefix,
                max_depth: *max_depth,
            }),
            Self::Organize { rule_id } => Ok(arclain_core::PipelineStep::Organize {
                rule_id: parse_step_rule_id(rule_id)?,
            }),
            Self::Convert {
                format,
                compression,
            } => Ok(arclain_core::PipelineStep::Convert {
                format: parse_convert_format(format)?,
                compression: compression.to_core(),
                password: None,
            }),
        }
    }

    /// The reverse of [`Self::to_core`], for reading a stored
    /// `arclain_core::Pipeline` back out as DTOs (see
    /// [`crate::process::PipelinePresetSummary`]).
    ///
    /// Total, and lossy in exactly one documented place: a stored
    /// `Convert` step's inert `password` has no DTO field to land in
    /// (see this type's own doc comment), so a preset that carries one
    /// loses it on the next save through this facade. Nothing reads that
    /// field -- `arclain_core`'s executor binds it `password: _` -- so
    /// dropping it changes no behavior; it is called out because a
    /// round trip through these two functions is otherwise exact.
    ///
    /// `format` is emitted as the extension token (`"zip"`/`"7z"`),
    /// which is precisely what [`parse_convert_format`] accepts, so
    /// `from_core(step).to_core()` never fails on a `Convert` step.
    pub(crate) fn from_core(step: &arclain_core::PipelineStep) -> Self {
        match step {
            arclain_core::PipelineStep::Flatten {
                strip_common_prefix,
                max_depth,
            } => Self::Flatten {
                strip_common_prefix: *strip_common_prefix,
                max_depth: *max_depth,
            },
            arclain_core::PipelineStep::Organize { rule_id } => Self::Organize {
                rule_id: rule_id.to_string(),
            },
            arclain_core::PipelineStep::Convert {
                format,
                compression,
                password: _,
            } => Self::Convert {
                format: format.extension().to_string(),
                compression: CompressionLevelDto::from_core(*compression),
            },
        }
    }
}

fn parse_step_rule_id(rule_id: &str) -> Result<i64, ApplicationError> {
    rule_id.trim().parse::<i64>().map_err(|_| {
        ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "expected a decimal integer rule id",
        )
        .with_diagnostic(format!("Organize step rule_id: got {rule_id:?}"))
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field("rule_id")
    })
}

/// Mirrors `arclain_core::PipelineOutput` exactly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineDestinationDto {
    SameFolder,
    Folder { path: PathBuf },
}

impl PipelineDestinationDto {
    pub(crate) fn to_core(&self) -> arclain_core::PipelineOutput {
        match self {
            Self::SameFolder => arclain_core::PipelineOutput::SameFolder,
            Self::Folder { path } => arclain_core::PipelineOutput::NewFolder(path.clone()),
        }
    }

    pub(crate) fn from_core(output: &arclain_core::PipelineOutput) -> Self {
        match output {
            arclain_core::PipelineOutput::SameFolder => Self::SameFolder,
            arclain_core::PipelineOutput::NewFolder(path) => Self::Folder { path: path.clone() },
        }
    }
}

/// Mirrors `arclain_core::OutputCollisionPolicy` exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputCollisionPolicyDto {
    Fail,
    Skip,
    Overwrite,
    Smart,
}

impl OutputCollisionPolicyDto {
    pub(crate) fn to_core(self) -> arclain_core::OutputCollisionPolicy {
        match self {
            Self::Fail => arclain_core::OutputCollisionPolicy::Fail,
            Self::Skip => arclain_core::OutputCollisionPolicy::Skip,
            Self::Overwrite => arclain_core::OutputCollisionPolicy::Overwrite,
            Self::Smart => arclain_core::OutputCollisionPolicy::Smart,
        }
    }

    pub(crate) fn from_core(policy: arclain_core::OutputCollisionPolicy) -> Self {
        match policy {
            arclain_core::OutputCollisionPolicy::Fail => Self::Fail,
            arclain_core::OutputCollisionPolicy::Skip => Self::Skip,
            arclain_core::OutputCollisionPolicy::Overwrite => Self::Overwrite,
            arclain_core::OutputCollisionPolicy::Smart => Self::Smart,
        }
    }
}

/// Mirrors `arclain_core::OutputArtifact` exactly: whether an ad-hoc
/// step list's result is packed into an archive or left as a plain
/// folder. Defaults to `Archive` on deserialization (a caller/bridge
/// that omits the field gets the same default `arclain_core::
/// OutputArtifact` itself does --
/// `crates/core/src/features/pipeline/types.rs:215-220`'s own
/// `#[default]`, which the Process page's "Output as:" dropdown
/// inherits by using that same type), and this is a real default the
/// executor relies on,
/// not a placeholder: a step list with no `Convert` step still packs
/// into a zip when `output_artifact` is `Archive` (`execute_pipeline`
/// falls back to `ConvertFormat::Zip` when no `Convert` step chose a
/// format) -- documented, intended behavior on the Process page, not a
/// gap to route around. An earlier draft of [`PipelineSpecDto::Steps`]
/// had no field for this at all and *derived* the artifact kind
/// instead (`Archive` iff a `Convert` step was present); review caught
/// that this silently diverges from the Process page whenever a caller
/// wants a Flatten-only run packed anyway (or a Convert-bearing run left
/// as a folder, e.g. for a later manual step) -- an explicit field
/// expresses both cases the derivation could not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputArtifactDto {
    #[default]
    Archive,
    Folder,
}

impl OutputArtifactDto {
    pub(crate) fn to_core(self) -> arclain_core::OutputArtifact {
        match self {
            Self::Archive => arclain_core::OutputArtifact::Archive,
            Self::Folder => arclain_core::OutputArtifact::Folder,
        }
    }

    pub(crate) fn from_core(artifact: arclain_core::OutputArtifact) -> Self {
        match artifact {
            arclain_core::OutputArtifact::Archive => Self::Archive,
            arclain_core::OutputArtifact::Folder => Self::Folder,
        }
    }
}

/// Which pipeline to run: a saved, named preset, or an ad-hoc step list
/// assembled the way the Process page's step builder does. See this
/// module's own doc comment.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineSpecDto {
    /// Matches an `arclain_core::SavedPreset::name` (presets have no
    /// numeric id -- see `arclain_core::{builtin_presets, load_presets}`).
    Preset { id: String },
    Steps {
        steps: Vec<PipelineStepDto>,
        /// See [`OutputArtifactDto`]'s own doc comment.
        #[serde(default)]
        output_artifact: OutputArtifactDto,
    },
}

/// Runs either a saved preset or an ad-hoc step list over a batch of
/// inputs, writing results to `destination`. `collision_policy`
/// overrides the resolved pipeline's own collision policy when set,
/// matching the Process page's own per-run "If output exists:" dropdown
/// (`None` defers to the preset's stored policy, or the app-wide
/// default setting, exactly as an ad-hoc pipeline with no explicit
/// choice already does).
#[derive(Debug)]
pub struct PipelineRequest {
    pub inputs: Vec<PathBuf>,
    pub destination: PipelineDestinationDto,
    pub pipeline: PipelineSpecDto,
    pub collision_policy: Option<OutputCollisionPolicyDto>,
}

impl PipelineRequest {
    /// The purely-structural, no-I/O checks this request needs: a
    /// non-empty input list, and (for [`PipelineSpecDto::Steps`]) a
    /// non-empty step list whose every step's own fields parse. Whether
    /// a [`PipelineSpecDto::Preset`] id actually names a known preset
    /// requires reading the presets file, so
    /// [`crate::runtime::ArclainApp::start_pipeline`] resolves that
    /// separately (see `processing_ops::resolve_pipeline_spec`) after
    /// this passes.
    pub(crate) fn validate(&self) -> Result<(), ApplicationError> {
        if self.inputs.is_empty() {
            return Err(empty_inputs_error());
        }
        if let PipelineSpecDto::Steps { steps, .. } = &self.pipeline {
            // The Process page itself refuses to run an empty step list
            // ("No operations added") -- an ad-hoc pipeline with zero
            // steps is not a degenerate no-op worth silently accepting,
            // it is a request that forgot to say what to do.
            if steps.is_empty() {
                return Err(empty_steps_error());
            }
            for step in steps {
                step.validate()?;
            }
        }
        Ok(())
    }
}

fn empty_steps_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "an ad-hoc pipeline needs at least one step",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::ChooseDestination)
    .with_field("steps")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ApplicationErrorKind;

    fn request(inputs: Vec<PathBuf>, pipeline: PipelineSpecDto) -> PipelineRequest {
        PipelineRequest {
            inputs,
            destination: PipelineDestinationDto::SameFolder,
            pipeline,
            collision_policy: None,
        }
    }

    #[test]
    fn empty_inputs_are_rejected() {
        let err = request(
            vec![],
            PipelineSpecDto::Preset {
                id: "RE Mod Cleanup".to_string(),
            },
        )
        .validate()
        .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("inputs"));
    }

    #[test]
    fn preset_spec_needs_no_further_structural_validation() {
        request(
            vec![PathBuf::from("a.rar")],
            PipelineSpecDto::Preset {
                id: "RE Mod Cleanup".to_string(),
            },
        )
        .validate()
        .expect("a preset spec with non-empty inputs must validate");
    }

    #[test]
    fn steps_spec_validates_every_step() {
        let valid = request(
            vec![PathBuf::from("a.rar")],
            PipelineSpecDto::Steps {
                steps: vec![
                    PipelineStepDto::Flatten {
                        strip_common_prefix: true,
                        max_depth: 1,
                    },
                    PipelineStepDto::Convert {
                        format: "zip".to_string(),
                        compression: CompressionLevelDto::Normal,
                    },
                ],
                output_artifact: OutputArtifactDto::Archive,
            },
        );
        valid.validate().expect("every step is well-formed");

        let invalid = request(
            vec![PathBuf::from("a.rar")],
            PipelineSpecDto::Steps {
                steps: vec![PipelineStepDto::Convert {
                    format: "rar".to_string(),
                    compression: CompressionLevelDto::Normal,
                }],
                output_artifact: OutputArtifactDto::Archive,
            },
        );
        let err = invalid.validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("format"));
    }

    #[test]
    fn steps_spec_rejects_an_empty_step_list() {
        let err = request(
            vec![PathBuf::from("a.rar")],
            PipelineSpecDto::Steps {
                steps: vec![],
                output_artifact: OutputArtifactDto::default(),
            },
        )
        .validate()
        .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("steps"));
    }

    #[test]
    fn organize_step_rejects_a_non_numeric_rule_id() {
        let request = request(
            vec![PathBuf::from("a.rar")],
            PipelineSpecDto::Steps {
                steps: vec![PipelineStepDto::Organize {
                    rule_id: "not-a-number".to_string(),
                }],
                output_artifact: OutputArtifactDto::Archive,
            },
        );
        let err = request.validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("rule_id"));
    }

    #[test]
    fn step_dto_translates_to_the_matching_core_step() {
        assert_eq!(
            PipelineStepDto::Flatten {
                strip_common_prefix: true,
                max_depth: 3,
            }
            .to_core()
            .unwrap(),
            arclain_core::PipelineStep::Flatten {
                strip_common_prefix: true,
                max_depth: 3,
            }
        );
        assert_eq!(
            PipelineStepDto::Organize {
                rule_id: "9".to_string()
            }
            .to_core()
            .unwrap(),
            arclain_core::PipelineStep::Organize { rule_id: 9 }
        );
        assert_eq!(
            PipelineStepDto::Convert {
                format: "7z".to_string(),
                compression: CompressionLevelDto::Max,
            }
            .to_core()
            .unwrap(),
            arclain_core::PipelineStep::Convert {
                format: arclain_core::ConvertFormat::SevenZ,
                compression: arclain_core::CompressionLevel::Max,
                password: None,
            }
        );
    }

    #[test]
    fn destination_dto_translates_to_the_matching_core_output() {
        assert_eq!(
            PipelineDestinationDto::SameFolder.to_core(),
            arclain_core::PipelineOutput::SameFolder
        );
        assert_eq!(
            PipelineDestinationDto::Folder {
                path: PathBuf::from("/out")
            }
            .to_core(),
            arclain_core::PipelineOutput::NewFolder(PathBuf::from("/out"))
        );
    }

    #[test]
    fn collision_policy_dto_translates_to_the_matching_core_policy() {
        assert_eq!(
            OutputCollisionPolicyDto::Overwrite.to_core(),
            arclain_core::OutputCollisionPolicy::Overwrite
        );
        assert_eq!(
            OutputCollisionPolicyDto::Smart.to_core(),
            arclain_core::OutputCollisionPolicy::Smart
        );
    }

    #[test]
    fn output_artifact_dto_translates_to_the_matching_core_artifact() {
        assert_eq!(
            OutputArtifactDto::Archive.to_core(),
            arclain_core::OutputArtifact::Archive
        );
        assert_eq!(
            OutputArtifactDto::Folder.to_core(),
            arclain_core::OutputArtifact::Folder
        );
    }

    /// `from_core` exists so a stored preset can be read back out as
    /// DTOs (`crate::process`). The property that matters is that the
    /// pair is a true round trip on everything the DTO carries -- if it
    /// were not, listing a preset and saving it straight back would
    /// silently rewrite it.
    #[test]
    fn step_dtos_round_trip_from_core_and_back() {
        for step in [
            arclain_core::PipelineStep::Flatten {
                strip_common_prefix: true,
                max_depth: 0,
            },
            arclain_core::PipelineStep::Flatten {
                strip_common_prefix: false,
                max_depth: 7,
            },
            arclain_core::PipelineStep::Organize { rule_id: 42 },
            arclain_core::PipelineStep::Convert {
                format: arclain_core::ConvertFormat::Zip,
                compression: arclain_core::CompressionLevel::Fast,
                password: None,
            },
            arclain_core::PipelineStep::Convert {
                format: arclain_core::ConvertFormat::SevenZ,
                compression: arclain_core::CompressionLevel::Max,
                password: None,
            },
        ] {
            let dto = PipelineStepDto::from_core(&step);
            assert_eq!(
                dto.to_core()
                    .expect("a step built from core must translate back"),
                step
            );
        }
    }

    /// The one documented lossy edge: a stored `Convert` step's inert
    /// `password` has no DTO field, so it is dropped rather than
    /// silently smuggled through. Pins that this is the *only* thing
    /// that changes.
    #[test]
    fn a_stored_convert_password_is_dropped_not_carried() {
        let stored = arclain_core::PipelineStep::Convert {
            format: arclain_core::ConvertFormat::Zip,
            compression: arclain_core::CompressionLevel::Normal,
            password: Some("hunter2".to_string()),
        };
        let round_tripped = PipelineStepDto::from_core(&stored).to_core().unwrap();
        assert_eq!(
            round_tripped,
            arclain_core::PipelineStep::Convert {
                format: arclain_core::ConvertFormat::Zip,
                compression: arclain_core::CompressionLevel::Normal,
                password: None,
            }
        );
    }

    #[test]
    fn destination_policy_and_artifact_dtos_round_trip_from_core_and_back() {
        for output in [
            arclain_core::PipelineOutput::SameFolder,
            arclain_core::PipelineOutput::NewFolder(PathBuf::from("/out")),
        ] {
            assert_eq!(PipelineDestinationDto::from_core(&output).to_core(), output);
        }
        for policy in [
            arclain_core::OutputCollisionPolicy::Fail,
            arclain_core::OutputCollisionPolicy::Skip,
            arclain_core::OutputCollisionPolicy::Overwrite,
            arclain_core::OutputCollisionPolicy::Smart,
        ] {
            assert_eq!(
                OutputCollisionPolicyDto::from_core(policy).to_core(),
                policy
            );
        }
        for artifact in [
            arclain_core::OutputArtifact::Archive,
            arclain_core::OutputArtifact::Folder,
        ] {
            assert_eq!(OutputArtifactDto::from_core(artifact).to_core(), artifact);
        }
    }

    #[test]
    fn output_artifact_dto_defaults_to_archive() {
        // Pins the claim in `OutputArtifactDto`'s own doc comment: a
        // caller/bridge that omits the field (or explicitly uses
        // `Default::default()`) gets `Archive`, matching the Process
        // page's own "Output as:" dropdown default.
        assert_eq!(OutputArtifactDto::default(), OutputArtifactDto::Archive);
    }
}
