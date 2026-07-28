//! `OrganizeRequest`: the facade request DTO for the batch archive-
//! organization operation ([`crate::ArclainApp::start_organize`]).
//!
//! Characterization (pre-facade flows this replaces, and the naming
//! decision this file makes): the pre-facade UI actually had *two*
//! independent organize code paths --
//!
//! - `crates/ui/src/features/organization/presentation/controllers/
//!   organization_controller.rs::ActionContext::handle` (the single-
//!   archive "Organize" quick action): builds one `OrganizationPlan`
//!   interactively (rule chosen via a dropdown,
//!   `arclain_core::features::organization::engine::RuleEngine::
//!   create_plan`), then calls `crate::features::archive_operations::
//!   run_organization_plan`, which repacks the result using an
//!   `arclain_core::features::organization::ArchiveProfile` (an output
//!   *archive format/compression* preset -- ZIP/7z, compression level,
//!   solid/encrypt-headers). This path has no transactional output
//!   commit/rollback at all: it repacks directly over `dest`.
//! - The batch "Process" page's `PipelineStep::Organize { rule_id }`,
//!   run through `arclain_core::execute_pipeline`: a numeric
//!   `arclain_core::OrganizationRule` id picks the layout rule for the
//!   *whole* batch, output defaults to a plain folder unless a later
//!   `Convert` step packs it, and every input/output goes through
//!   `arclain_core`'s `StagedOutput` transaction (atomic promote,
//!   rollback-preserves-pre-existing-output -- see
//!   `crates/core/src/features/pipeline/output_transaction.rs`'s own
//!   exhaustive test suite).
//!
//! **Decision:** `OrganizeRequest` wraps the *second* flow (`PipelineStep::
//! Organize` via `execute_pipeline`), not the first. The brief's own test
//! list requires proving output-transaction rollback for Organize, and
//! only the `execute_pipeline` path has that property at all -- the quick-
//! action repack has no staged/atomic commit to characterize. `profile_id`
//! is therefore parsed as the decimal string form of an
//! `arclain_core::OrganizationRule` id (`PipelineStep::Organize::rule_id`),
//! *not* an `ArchiveProfile` id, despite the field's name. This is a
//! judgment call on an underspecified contract field, called out
//! explicitly in this task's report -- the alternative (treating
//! `profile_id` as an `ArchiveProfile` and reimplementing the quick-
//! action's direct-repack flow instead) was rejected for the reason
//! above.
//!
//! `dry_run` computes `arclain_core::preview_pipeline_with_metadata`
//! (pure, already exists) instead of running `execute_pipeline`, and
//! reports the result via `OperationState::Progress` messages -- see
//! `crate::runtime::processing_ops::run_dry_run_preview`.

use std::path::PathBuf;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};

use super::convert::empty_inputs_error;

/// Organizes a batch of archives according to one organization rule,
/// writing the resulting folders under `destination` (each input gets
/// its own subfolder/file, named by its own stem/detected metadata
/// title -- the same convention `arclain_core::PipelineOutput::
/// NewFolder` already uses).
///
/// No archive-format/compression fields exist here (unlike
/// [`super::ConvertRequest`]): organizing produces a plain folder on
/// disk (`arclain_core::OutputArtifact::Folder`), never a repacked
/// archive. See this module's doc comment for why.
#[derive(Debug)]
pub struct OrganizeRequest {
    pub inputs: Vec<PathBuf>,
    pub destination: PathBuf,
    pub profile_id: String,
    pub dry_run: bool,
}

impl OrganizeRequest {
    /// Validates this request and, on success, parses [`Self::profile_id`]
    /// into the `arclain_core::OrganizationRule` id it identifies (see
    /// this module's doc comment). Purely structural -- no I/O, so this
    /// runs before [`crate::runtime::ArclainApp::start_organize`]
    /// registers an operation. Whether the id actually names an
    /// *existing, enabled* rule is a separate, I/O-requiring check that
    /// method performs afterward (see `processing_ops::resolve_rule`).
    pub(crate) fn validate(&self) -> Result<i64, ApplicationError> {
        if self.inputs.is_empty() {
            return Err(empty_inputs_error());
        }
        parse_rule_id(&self.profile_id)
    }
}

fn parse_rule_id(profile_id: &str) -> Result<i64, ApplicationError> {
    profile_id.trim().parse::<i64>().map_err(|_| {
        ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "profile_id must be an organization rule id",
        )
        .with_diagnostic(format!(
            "expected a decimal integer (an arclain_core::OrganizationRule id), got {profile_id:?}"
        ))
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field("profile_id")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(inputs: Vec<PathBuf>, profile_id: &str) -> OrganizeRequest {
        OrganizeRequest {
            inputs,
            destination: PathBuf::from("/dest"),
            profile_id: profile_id.to_string(),
            dry_run: false,
        }
    }

    #[test]
    fn empty_inputs_are_rejected() {
        let err = request(vec![], "1").validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("inputs"));
    }

    #[test]
    fn numeric_profile_id_parses_as_a_rule_id() {
        let rule_id = request(vec![PathBuf::from("a.zip")], "42")
            .validate()
            .expect("numeric profile_id must be accepted");
        assert_eq!(rule_id, 42);
    }

    #[test]
    fn whitespace_around_a_numeric_profile_id_is_tolerated() {
        let rule_id = request(vec![PathBuf::from("a.zip")], "  7 \n")
            .validate()
            .expect("whitespace-padded numeric profile_id must be accepted");
        assert_eq!(rule_id, 7);
    }

    #[test]
    fn non_numeric_profile_id_is_rejected() {
        let err = request(vec![PathBuf::from("a.zip")], "dlsite-standard")
            .validate()
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("profile_id"));
    }
}
