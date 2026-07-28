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
}

/// Which pipeline to run: a saved, named preset, or an ad-hoc step list
/// assembled the way the Process page's step builder does. See this
/// module's own doc comment.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineSpecDto {
    /// Matches an `arclain_core::SavedPreset::name` (presets have no
    /// numeric id -- see `arclain_core::{builtin_presets, load_presets}`).
    Preset {
        id: String,
    },
    Steps {
        steps: Vec<PipelineStepDto>,
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
    /// non-empty input list, and (for [`PipelineSpecDto::Steps`]) every
    /// step's own fields must parse. Whether a [`PipelineSpecDto::Preset`]
    /// id actually names a known preset requires reading the presets
    /// file, so [`crate::runtime::ArclainApp::start_pipeline`] resolves
    /// that separately (see `processing_ops::resolve_pipeline_spec`)
    /// after this passes.
    pub(crate) fn validate(&self) -> Result<(), ApplicationError> {
        if self.inputs.is_empty() {
            return Err(empty_inputs_error());
        }
        if let PipelineSpecDto::Steps { steps } = &self.pipeline {
            for step in steps {
                step.validate()?;
            }
        }
        Ok(())
    }
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
            },
        );
        let err = invalid.validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("format"));
    }

    #[test]
    fn organize_step_rejects_a_non_numeric_rule_id() {
        let request = request(
            vec![PathBuf::from("a.rar")],
            PipelineSpecDto::Steps {
                steps: vec![PipelineStepDto::Organize {
                    rule_id: "not-a-number".to_string(),
                }],
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
}
