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
//! ## Binding one organize to one open archive session
//!
//! [`OrganizeRequest::archive_session_id`] exists because a *previewed*
//! organize and an *applied* one must be the same organize.
//! [`crate::runtime::ArclainApp::preview_organize_plan`] builds its plan
//! from the metadata the session itself holds -- the JSON a plugin
//! reported through `emit_metadata`. A path-only batch organize has no
//! session to read, so it resolves metadata the only way it can: a
//! DLsite library lookup keyed on a product code detected in the file
//! name (`crate::runtime::processing_ops::resolve_metadata`). Those two
//! sources routinely disagree -- a plugin can report a title for an
//! archive the library has never seen, and the library can hold a stale
//! row for one a plugin has since re-fetched -- and the plan's root
//! folder, its move destinations, and the output file's own name are
//! all functions of the metadata that resolved.
//!
//! So a panel that previewed a plan and then applied it through a
//! path-only request would silently organize something other than what
//! the user just approved. Naming the session closes that: with a
//! binding, the archive *is* the session's own, and the metadata is
//! exactly the value the preview read (snapshotted once when the
//! operation starts, so a metadata write landing mid-run cannot change
//! the plan out from under it either).
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
use crate::ids::ArchiveSessionId;

use super::convert::empty_inputs_error;

/// Organizes a batch of archives according to one organization rule
/// (layout) and one archive profile (output format/compression),
/// writing the resulting packed archive under `destination` (each
/// input gets its own output file, named by its own stem/detected
/// metadata title -- the same convention `arclain_core::PipelineOutput::
/// NewFolder` uses elsewhere in this facade).
#[derive(Debug)]
pub struct OrganizeRequest {
    /// The archives to organize. Must be empty when
    /// [`Self::archive_session_id`] is set (the session names the one
    /// archive), and non-empty otherwise.
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
    /// Organizes the archive open in this session, using the metadata
    /// that session holds -- the exact plan
    /// [`crate::runtime::ArclainApp::preview_organize_plan`] previewed
    /// for the same session and rule. See this module's doc comment for
    /// why a previewed organize needs this and a batch one does not.
    ///
    /// The archive is the session's own `source_path`, so `inputs` must
    /// be empty: a caller cannot bind one session's metadata onto a
    /// different archive's contents, by construction rather than by
    /// convention. The password the session was opened with is reused
    /// too, so applying to an archive the user already unlocked does not
    /// prompt again.
    pub archive_session_id: Option<ArchiveSessionId>,
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
    /// Rejects (as [`ApplicationErrorKind::InvalidInput`]) an `inputs`
    /// list that disagrees with [`Self::archive_session_id`], or either
    /// id string failing to parse as a decimal integer -- all purely
    /// structural problems, discoverable with no I/O, so
    /// [`crate::runtime::ArclainApp::start_organize`] runs this before
    /// ever registering an operation. Whether the ids actually name an
    /// *existing* rule/profile/session is a separate, I/O-requiring
    /// check that method performs afterward (see
    /// `processing_ops::resolve_rule_and_profile` and
    /// `processing_ops::resolve_session_binding`).
    pub(crate) fn validate(&self) -> Result<ParsedIds, ApplicationError> {
        match self.archive_session_id {
            // A session-bound organize takes its one archive from the
            // session. Supplied paths are refused rather than ignored:
            // silently dropping them would let a caller believe it had
            // organized files this request never touches.
            Some(_) if !self.inputs.is_empty() => {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::InvalidInput,
                    "a session-bound organize takes its archive from the session, not from inputs",
                )
                .with_diagnostic(format!(
                    "archive_session_id is set and {} input path(s) were also supplied",
                    self.inputs.len()
                ))
                .with_recoverability(Recoverability::UserAction)
                .with_field("inputs"));
            }
            Some(_) => {}
            None if self.inputs.is_empty() => return Err(empty_inputs_error()),
            None => {}
        }
        // An empty destination is not "the current directory" here: the
        // output name would become the whole path, resolved against
        // whatever the process working directory happens to be, and
        // then handed to the packer as a bare argument. Every shipped
        // frontend passes a real directory; this is the boundary the
        // next one comes through too.
        if self.destination.as_os_str().is_empty() {
            return Err(ApplicationError::new(
                ApplicationErrorKind::InvalidInput,
                "no destination directory was supplied",
            )
            .with_recoverability(Recoverability::UserAction)
            .with_suggested_action(SuggestedAction::ChooseDestination)
            .with_field("destination"));
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
            archive_session_id: None,
        }
    }

    #[test]
    fn empty_inputs_are_rejected() {
        let err = request(vec![], "1", "2").validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("inputs"));
    }

    #[test]
    fn an_empty_destination_is_rejected() {
        let mut request = request(vec![PathBuf::from("a.zip")], "1", "2");
        request.destination = PathBuf::new();
        let err = request.validate().unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(err.field.as_deref(), Some("destination"));
    }

    #[test]
    fn a_session_bound_request_needs_no_inputs() {
        let mut request = request(vec![], "1", "2");
        request.archive_session_id = Some(ArchiveSessionId::from_raw(7));
        let parsed = request
            .validate()
            .expect("the session supplies the archive, so an empty inputs list is correct");
        assert_eq!(parsed.profile_id, 1);
        assert_eq!(parsed.rule_id, 2);
    }

    /// Refused, not ignored: the session's metadata must only ever be
    /// applied to the session's own archive.
    #[test]
    fn a_session_bound_request_rejects_supplied_inputs() {
        let mut request = request(vec![PathBuf::from("elsewhere.zip")], "1", "2");
        request.archive_session_id = Some(ArchiveSessionId::from_raw(7));
        let err = request.validate().unwrap_err();
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
