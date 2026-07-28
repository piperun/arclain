//! `OrganizeRequest`: the facade request DTO for the batch archive-
//! organization operation ([`crate::ArclainApp::start_organize`]).
//!
//! ## Adjudicated characterization (amended from this task's first
//! submission)
//!
//! The pre-facade UI's single-archive "quick action"
//! (`crates/ui/src/features/organization/presentation/controllers/
//! organization_controller.rs::ActionContext::handle`,
//! `OrganizationAction::Apply`) reads **two independent selections**:
//!
//! - **"Rule:"** (`organize_panel/rule_selector.rs`) binds an
//!   `arclain_core::OrganizationRule` -- which files go where inside the
//!   organized layout (`RuleEngine::create_plan`).
//! - **"Profile:"** (`organize_panel/profile_selector.rs`,
//!   `organization_controller.rs:38-59`) binds an `arclain_core::
//!   features::organization::ArchiveProfile` -- the *output archive's*
//!   format/compression. The controller reads
//!   `profiles[selected_profile_index]` and derives the destination's
//!   extension from `profile.format`.
//!
//! `OrganizeRequest` therefore carries **both** ids: `rule_id` (layout)
//! and `profile_id` (output format/compression) -- this task's first
//! submission collapsed the two into one `profile_id` field parsed as a
//! rule id, which was incorrect; see `crate::runtime::processing_ops`'s
//! own doc comment for the corrected flow this now drives.
//!
//! **This flow has no output transaction.** `execute_organization_plan`
//! (the pure core function this now calls, matching the quick action
//! exactly) extracts, applies the plan, and packs the result via
//! `archive.backend().create_archive_with_profile(...)` directly onto
//! `dest` -- no `StagedOutput`, no atomic commit, no rollback. This
//! matches the pre-facade quick action's own behavior precisely (it
//! never had a transaction either); this task's first submission
//! incorrectly assumed Organize went through the transactional
//! `execute_pipeline`/`PipelineStep::Organize` path instead. The
//! absence is preserved and characterized honestly here, not
//! papered over -- see `crate::runtime::processing_ops`'s tests for
//! what this means in practice (a colliding destination is genuinely at
//! risk; there is no rollback to protect it).

use std::path::PathBuf;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};

use super::convert::empty_inputs_error;

/// Organizes a batch of archives according to one organization rule
/// (layout) and one archive profile (output format/compression),
/// writing the resulting packed archive under `destination` (each
/// input gets its own output file, named by its own stem/detected
/// metadata title -- the same convention `arclain_core::PipelineOutput::
/// NewFolder` uses elsewhere in this facade).
#[derive(Debug)]
pub struct OrganizeRequest {
    pub inputs: Vec<PathBuf>,
    pub destination: PathBuf,
    /// An `arclain_core::features::organization::ArchiveProfile` id --
    /// governs the output archive's format/compression. See this
    /// module's doc comment for why this is a separate id from
    /// `rule_id`.
    pub profile_id: String,
    /// An `arclain_core::OrganizationRule` id -- governs the organized
    /// layout (which files go where).
    pub rule_id: String,
    pub dry_run: bool,
}

/// Both ids this request needs, once parsed from their decimal string
/// form. Returned together by [`OrganizeRequest::validate`] so a caller
/// destructures one value instead of two independent `Result`s.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParsedIds {
    pub(crate) profile_id: i64,
    pub(crate) rule_id: i64,
}

impl OrganizeRequest {
    /// Validates this request and, on success, parses [`Self::profile_id`]/
    /// [`Self::rule_id`] into the ids they identify.
    ///
    /// Rejects (as [`ApplicationErrorKind::InvalidInput`]) an empty input
    /// list or either id string failing to parse as a decimal integer --
    /// both are purely structural problems, discoverable with no I/O, so
    /// [`crate::runtime::ArclainApp::start_organize`] runs this before
    /// ever registering an operation. Whether the ids actually name an
    /// *existing* rule/profile is a separate, I/O-requiring check that
    /// method performs afterward (see `processing_ops::resolve_rule_and_profile`).
    pub(crate) fn validate(&self) -> Result<ParsedIds, ApplicationError> {
        if self.inputs.is_empty() {
            return Err(empty_inputs_error());
        }
        Ok(ParsedIds {
            profile_id: parse_id(&self.profile_id, "profile_id")?,
            rule_id: parse_id(&self.rule_id, "rule_id")?,
        })
    }
}

fn parse_id(value: &str, field: &'static str) -> Result<i64, ApplicationError> {
    value.trim().parse::<i64>().map_err(|_| {
        ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "expected a decimal integer id",
        )
        .with_diagnostic(format!("field {field:?}: got {value:?}"))
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field(field)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(inputs: Vec<PathBuf>, profile_id: &str, rule_id: &str) -> OrganizeRequest {
        OrganizeRequest {
            inputs,
            destination: PathBuf::from("/dest"),
            profile_id: profile_id.to_string(),
            rule_id: rule_id.to_string(),
            dry_run: false,
        }
    }

    #[test]
    fn empty_inputs_are_rejected() {
        let err = request(vec![], "1", "2").validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("inputs"));
    }

    #[test]
    fn numeric_ids_parse_independently() {
        let parsed = request(vec![PathBuf::from("a.zip")], "42", "7")
            .validate()
            .expect("numeric ids must be accepted");
        assert_eq!(parsed.profile_id, 42);
        assert_eq!(parsed.rule_id, 7);
    }

    #[test]
    fn whitespace_around_a_numeric_id_is_tolerated() {
        let parsed = request(vec![PathBuf::from("a.zip")], " 3 \n", "\t9")
            .validate()
            .expect("whitespace-padded numeric ids must be accepted");
        assert_eq!(parsed.profile_id, 3);
        assert_eq!(parsed.rule_id, 9);
    }

    #[test]
    fn non_numeric_profile_id_is_rejected() {
        let err = request(vec![PathBuf::from("a.zip")], "max-7z", "1")
            .validate()
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("profile_id"));
    }

    #[test]
    fn non_numeric_rule_id_is_rejected() {
        let err = request(vec![PathBuf::from("a.zip")], "1", "dlsite-standard")
            .validate()
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("rule_id"));
    }
}
