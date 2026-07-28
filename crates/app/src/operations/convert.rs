//! `ConvertRequest`: the facade request DTO for the batch archive-
//! conversion operation ([`crate::ArclainApp::start_convert`]).
//!
//! Characterization (pre-facade flow this replaces): `crates/ui/src/
//! core/operations/process_runner.rs::spawn_run` built an
//! `arclain_core::Pipeline` from whatever steps the Process page's UI
//! had accumulated and ran it via `arclain_core::execute_pipeline` on
//! the shared tokio runtime, reporting progress into a `Signal`.
//! `ConvertRequest` captures the single-purpose subset of that flow --
//! "convert these files to this format, optionally flattening nested
//! archives first" -- as its own stable request shape; the general
//! multi-step case stays [`crate::operations::PipelineRequest`]. See
//! `crate::runtime::processing_ops` for how this becomes a `Pipeline`
//! and is executed/characterized (output transactions, cancellation,
//! collision handling).

use std::path::PathBuf;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};

/// Converts a batch of archives to a target format, writing results
/// under `destination`. Each input keeps its own name (by stem/detected
/// metadata title, matching `arclain_core::PipelineOutput::resolve*`),
/// so `destination` is the containing folder, not a single output file.
#[derive(Debug)]
pub struct ConvertRequest {
    pub inputs: Vec<PathBuf>,
    pub destination: PathBuf,
    pub format: String,
    pub flatten: bool,
}

impl ConvertRequest {
    /// Validates this request and, on success, parses [`Self::format`]
    /// into the `arclain_core` type the pipeline executor needs.
    ///
    /// Rejects (as [`ApplicationErrorKind::InvalidInput`]) an empty
    /// input list or an unrecognized format string -- both are purely
    /// structural problems, discoverable with no I/O, so
    /// [`crate::runtime::ArclainApp::start_convert`] runs this before
    /// ever registering an operation: a malformed request never leaves
    /// a phantom `OperationId` behind.
    pub(crate) fn validate(&self) -> Result<arclain_core::ConvertFormat, ApplicationError> {
        if self.inputs.is_empty() {
            return Err(empty_inputs_error());
        }
        parse_convert_format(&self.format)
    }
}

/// Shared by every request kind in this module family (`convert`,
/// `organize`, `pipeline`): an empty `inputs` list is always the same
/// kind of structural mistake.
pub(crate) fn empty_inputs_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "no input files were supplied",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::ChooseDestination)
    .with_field("inputs")
}

/// Parses a convert-format string. Case-insensitive; accepts the same
/// vocabulary [`arclain_core::ConvertFormat::extension`] produces
/// ("zip", "7z") plus "sevenz" as a friendlier alias for a bridge
/// consumer that may not know the internal enum's exact spelling.
pub(crate) fn parse_convert_format(
    format: &str,
) -> Result<arclain_core::ConvertFormat, ApplicationError> {
    match format.to_ascii_lowercase().as_str() {
        "zip" => Ok(arclain_core::ConvertFormat::Zip),
        "7z" | "sevenz" => Ok(arclain_core::ConvertFormat::SevenZ),
        _ => Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "unrecognized convert format",
        )
        .with_diagnostic(format!(
            "supported formats are \"zip\" and \"7z\"; got {format:?}"
        ))
        .with_recoverability(Recoverability::UserAction)
        .with_field("format")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(inputs: Vec<PathBuf>, format: &str) -> ConvertRequest {
        ConvertRequest {
            inputs,
            destination: PathBuf::from("/dest"),
            format: format.to_string(),
            flatten: false,
        }
    }

    #[test]
    fn empty_inputs_are_rejected() {
        let err = request(vec![], "zip").validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("inputs"));
    }

    #[test]
    fn zip_and_7z_are_recognized_case_insensitively() {
        for (input, expected) in [
            ("zip", arclain_core::ConvertFormat::Zip),
            ("ZIP", arclain_core::ConvertFormat::Zip),
            ("7z", arclain_core::ConvertFormat::SevenZ),
            ("7Z", arclain_core::ConvertFormat::SevenZ),
            ("sevenz", arclain_core::ConvertFormat::SevenZ),
        ] {
            let format = request(vec![PathBuf::from("a.rar")], input)
                .validate()
                .unwrap_or_else(|_| panic!("{input:?} must be accepted"));
            assert_eq!(format, expected);
        }
    }

    #[test]
    fn unknown_format_is_rejected() {
        let err = request(vec![PathBuf::from("a.rar")], "rar")
            .validate()
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("format"));
    }
}
