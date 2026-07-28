//! `PipelineRequest`: the facade request DTO for running a saved,
//! multi-step preset ([`crate::ArclainApp::start_pipeline`]).
//!
//! Characterization (pre-facade flow this replaces): `crates/ui/src/
//! features/process/view.rs`'s Process page lets a user assemble an
//! arbitrary `arclain_core::Pipeline` (any mix of `Flatten`/`Organize`/
//! `Convert` steps, in order) and either run it directly or load one
//! from `arclain_core::{builtin_presets, load_presets}` (a
//! `Vec<arclain_core::SavedPreset>`, each a `{ name, pipeline }` pair --
//! presets are keyed by *name*, there is no numeric preset id, so
//! `preset_id: String` maps onto `SavedPreset::name` with no parsing
//! or translation needed, unlike `OrganizeRequest::profile_id`).
//!
//! `PipelineRequest` only ever *runs* a saved preset (matching
//! `arclain_core::builtin_presets()`/a user's `pipeline_presets.json`);
//! it does not let a caller assemble an ad-hoc step list -- the stable,
//! serializable request surface a bridge consumer needs is "run this
//! named, already-configured pipeline over these inputs", not a step-
//! builder API. `inputs`/`destination` override the preset's own
//! `Pipeline::input`/`Pipeline::output` (a preset stores only the step
//! list + defaults, exactly as `crates/ui`'s Process page already
//! treats input/output as chosen separately from which preset is
//! loaded); every other field of the preset's `Pipeline` (steps,
//! `output_artifact`, `collision_policy`) is used unchanged.

use std::path::PathBuf;

use crate::error::ApplicationError;

use super::convert::empty_inputs_error;

/// Runs a saved pipeline preset (see the module doc comment) over a
/// batch of inputs, writing results under `destination`.
#[derive(Debug)]
pub struct PipelineRequest {
    pub inputs: Vec<PathBuf>,
    pub destination: PathBuf,
    pub preset_id: String,
}

impl PipelineRequest {
    /// The only purely-structural, no-I/O check this request needs:
    /// an empty input list. Whether `preset_id` actually names a known
    /// preset requires reading the presets file, so
    /// [`crate::runtime::ArclainApp::start_pipeline`] resolves that
    /// separately (see `processing_ops::resolve_preset_pipeline`)
    /// after this passes.
    pub(crate) fn validate(&self) -> Result<(), ApplicationError> {
        if self.inputs.is_empty() {
            return Err(empty_inputs_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ApplicationErrorKind;

    #[test]
    fn empty_inputs_are_rejected() {
        let request = PipelineRequest {
            inputs: vec![],
            destination: PathBuf::from("/dest"),
            preset_id: "RE Mod Cleanup".to_string(),
        };
        let err = request.validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("inputs"));
    }

    #[test]
    fn non_empty_inputs_pass_structural_validation() {
        let request = PipelineRequest {
            inputs: vec![PathBuf::from("a.rar")],
            destination: PathBuf::from("/dest"),
            preset_id: "RE Mod Cleanup".to_string(),
        };
        request.validate().expect("non-empty inputs must validate");
    }
}
