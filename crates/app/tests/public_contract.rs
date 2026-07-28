//! Public-contract tests for `arclain_app`.
//!
//! This file is an integration test: it compiles against `arclain_app` as
//! an external crate, so it can only see what the crate actually exports as
//! `pub`. That is the point -- it proves Task 2's DTOs are constructible
//! and readable from outside the crate (CLI, bridge, frontend) using only
//! `from_raw`/`into_raw` for opaque IDs, with no way to reach a private
//! field even by accident.
//!
//! Deferred by design: this task does not test "rejection of a
//! reconstructed ID that does not exist in its owning store" -- the
//! archive-session, entry, operation, plugin-session, challenge, and lease
//! stores that would reject an unknown ID do not exist until later tasks
//! (5, 7, 11). Only the raw `from_raw`/`into_raw` round trip is covered
//! here.

use std::path::PathBuf;

use arclain_app::archive::{
    ArchiveEntryDto, ArchivePath, ArchiveSnapshot, EntryKind, EntryPage, EntrySortKey,
    ListEntriesRequest, SortDirection,
};
use arclain_app::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use arclain_app::ids::{
    ArchiveSessionId, ChallengeId, CorrelationId, EntryId, MaterializationLeaseId, OperationId,
    PluginSessionId,
};
use arclain_app::{ApplicationApiVersion, APPLICATION_API_VERSION};

/// Builds one instance of every public DTO this task introduces. Compiling
/// this function is most of the test: it proves the public surface is
/// constructible and readable from outside the crate.
#[test]
fn constructs_every_public_dto() {
    let archive_session_id = ArchiveSessionId::from_raw(1);
    let challenge_id = ChallengeId::from_raw(2);
    let correlation_id = CorrelationId::from_raw(3);
    let entry_id = EntryId::from_raw(4);
    let lease_id = MaterializationLeaseId::from_raw(5);
    let operation_id = OperationId::from_raw(6);
    let plugin_session_id = PluginSessionId::from_raw(7);
    assert_eq!(archive_session_id.into_raw(), 1);
    assert_eq!(challenge_id.into_raw(), 2);
    assert_eq!(correlation_id.into_raw(), 3);
    assert_eq!(entry_id.into_raw(), 4);
    assert_eq!(lease_id.into_raw(), 5);
    assert_eq!(operation_id.into_raw(), 6);
    assert_eq!(plugin_session_id.into_raw(), 7);

    assert_eq!(
        APPLICATION_API_VERSION,
        ApplicationApiVersion { major: 1, minor: 0 }
    );

    let error = ApplicationError::new(ApplicationErrorKind::NotFound, "not found")
        .with_diagnostic("diagnostic")
        .with_recoverability(Recoverability::UserAction)
        .with_retryable(false)
        .with_suggested_action(SuggestedAction::Retry)
        .with_operation_id(operation_id)
        .with_archive_session_id(archive_session_id)
        .with_entry_id(entry_id)
        .with_path(PathBuf::from("some/path"))
        .with_field("name");
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);

    let root = ArchivePath::root();
    let path = ArchivePath::parse("dir/file.txt").unwrap();
    assert_eq!(root.as_str(), "");

    let entry = ArchiveEntryDto {
        id: entry_id,
        path: path.clone(),
        name: "file.txt".to_string(),
        kind: EntryKind::File,
        compressed_size: Some(10),
        uncompressed_size: 20,
        modified_at_unix_ms: Some(0),
        encrypted: false,
        crc32: Some("deadbeef".to_string()),
    };

    let snapshot = ArchiveSnapshot {
        session_id: archive_session_id,
        revision: 1,
        source_path: PathBuf::from("archive.zip"),
        archive_type: "zip".to_string(),
        entry_count: 1,
        total_uncompressed_size: 20,
        comment: None,
        metadata: None,
    };

    let request = ListEntriesRequest {
        directory: root.clone(),
        sort_key: EntrySortKey::Name,
        sort_direction: SortDirection::Ascending,
        name_filter: None,
        offset: 0,
        limit: 50,
    };

    let page = EntryPage {
        session_id: archive_session_id,
        revision: 1,
        directory: root,
        total: 1,
        entries: vec![entry],
    };

    assert_eq!(request.sort_key, EntrySortKey::Name);
    assert_eq!(page.entries.len(), 1);
    assert_eq!(snapshot.archive_type, "zip");
}

mod api_version {
    use arclain_app::{ApplicationApiVersion, APPLICATION_API_VERSION};

    #[test]
    fn application_api_version_is_one_dot_zero() {
        assert_eq!(
            APPLICATION_API_VERSION,
            ApplicationApiVersion { major: 1, minor: 0 }
        );
    }
}

mod ids {
    use std::collections::HashSet;

    use arclain_app::ids::{
        ArchiveSessionId, ChallengeId, CorrelationId, EntryId, MaterializationLeaseId, OperationId,
        PluginSessionId,
    };

    /// Generates the same battery of round-trip/equality/ordering/hash/
    /// serialization tests for one opaque ID type. Every opaque ID shares
    /// the same `opaque_id!`-generated shape, so testing them through one
    /// macro keeps the seven types honestly identical instead of risking
    /// seven hand-copied blocks quietly drifting apart.
    macro_rules! opaque_id_tests {
        ($module:ident, $ty:ty) => {
            mod $module {
                use super::*;

                #[test]
                fn raw_round_trips() {
                    for raw in [0_u64, 1, 42, u64::MAX] {
                        assert_eq!(<$ty>::from_raw(raw).into_raw(), raw);
                    }
                }

                #[test]
                fn equality_and_ordering_follow_the_raw_value() {
                    assert_eq!(<$ty>::from_raw(1), <$ty>::from_raw(1));
                    assert_ne!(<$ty>::from_raw(1), <$ty>::from_raw(2));
                    assert!(<$ty>::from_raw(1) < <$ty>::from_raw(2));
                }

                #[test]
                fn hashable_for_set_membership() {
                    let mut set = HashSet::new();
                    set.insert(<$ty>::from_raw(1));
                    set.insert(<$ty>::from_raw(1));
                    set.insert(<$ty>::from_raw(2));
                    assert_eq!(set.len(), 2);
                }

                #[test]
                fn serializes_as_a_transparent_number() {
                    let value = serde_json::to_value(<$ty>::from_raw(7)).unwrap();
                    assert_eq!(value, serde_json::json!(7));
                    let round_tripped: $ty = serde_json::from_value(value).unwrap();
                    assert_eq!(round_tripped, <$ty>::from_raw(7));
                }
            }
        };
    }

    opaque_id_tests!(archive_session_id, ArchiveSessionId);
    opaque_id_tests!(challenge_id, ChallengeId);
    opaque_id_tests!(correlation_id, CorrelationId);
    opaque_id_tests!(entry_id, EntryId);
    opaque_id_tests!(materialization_lease_id, MaterializationLeaseId);
    opaque_id_tests!(operation_id, OperationId);
    opaque_id_tests!(plugin_session_id, PluginSessionId);

    #[test]
    fn correlation_id_generate_is_unique_across_many_calls() {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(CorrelationId::generate().into_raw()));
        }
    }
}

mod error_envelope {
    use std::collections::HashSet;

    use arclain_app::error::{
        ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction,
    };
    use arclain_app::ids::{ArchiveSessionId, EntryId, OperationId};

    #[test]
    fn diagnostic_over_4kib_is_truncated_and_marked() {
        let long = "a".repeat(10_000);
        let error =
            ApplicationError::new(ApplicationErrorKind::Backend, "summary").with_diagnostic(long);
        let diagnostic = error.diagnostic.expect("diagnostic set");
        assert!(diagnostic.len() <= 4096);
        assert!(diagnostic.ends_with("... [truncated]"));
    }

    #[test]
    fn diagnostic_under_4kib_is_stored_verbatim() {
        let error = ApplicationError::new(ApplicationErrorKind::Backend, "summary")
            .with_diagnostic("short diagnostic");
        assert_eq!(error.diagnostic.as_deref(), Some("short diagnostic"));
    }

    #[test]
    fn diagnostic_truncation_respects_utf8_char_boundaries() {
        // 4-byte UTF-8 characters straddling the 4096-byte cut point must
        // not panic and must not produce invalid UTF-8 (String::len is a
        // byte length, not a char count, so a naive byte-index cut can
        // otherwise land mid-character).
        let long = "\u{1F600}".repeat(2000); // 4 bytes each => 8000 bytes total
        let error =
            ApplicationError::new(ApplicationErrorKind::Backend, "summary").with_diagnostic(long);
        let diagnostic = error.diagnostic.expect("diagnostic set");
        assert!(diagnostic.len() <= 4096);
        assert!(diagnostic.ends_with("... [truncated]"));
    }

    #[test]
    fn path_is_none_by_default_and_set_only_when_attached() {
        let without_path = ApplicationError::new(ApplicationErrorKind::NotFound, "summary");
        assert_eq!(without_path.path, None);

        let with_path = ApplicationError::new(ApplicationErrorKind::NotFound, "summary")
            .with_path("archive/report.pdf");
        assert_eq!(
            with_path.path.as_deref(),
            Some(std::path::Path::new("archive/report.pdf"))
        );
    }

    #[test]
    fn recoverability_and_retryable_default_conservatively_then_round_trip() {
        let default_error = ApplicationError::new(ApplicationErrorKind::Internal, "summary");
        assert_eq!(default_error.recoverability, Recoverability::Fatal);
        assert!(!default_error.retryable);

        let retryable_error = ApplicationError::new(ApplicationErrorKind::Busy, "summary")
            .with_recoverability(Recoverability::Retry)
            .with_retryable(true);
        assert_eq!(retryable_error.recoverability, Recoverability::Retry);
        assert!(retryable_error.retryable);
    }

    #[test]
    fn suggested_action_is_none_by_default_and_round_trips_when_set() {
        let default_error =
            ApplicationError::new(ApplicationErrorKind::PasswordRequired, "summary");
        assert_eq!(default_error.suggested_action, None);

        let with_action = ApplicationError::new(ApplicationErrorKind::PasswordRequired, "summary")
            .with_suggested_action(SuggestedAction::SupplyPassword);
        assert_eq!(
            with_action.suggested_action,
            Some(SuggestedAction::SupplyPassword)
        );
    }

    #[test]
    fn correlation_ids_are_unique_per_error() {
        let mut seen = HashSet::new();
        for _ in 0..500 {
            let error = ApplicationError::new(ApplicationErrorKind::Internal, "summary");
            assert!(seen.insert(error.correlation_id.into_raw()));
        }
    }

    #[test]
    fn backend_chain_diagnostic_redacts_path_like_tokens() {
        let chain = [
            "failed to open archive".to_string(),
            r"caused by: C:\Users\alice\Documents\secret-project\archive.zip: Access is denied"
                .to_string(),
        ];
        let error = ApplicationError::new(ApplicationErrorKind::Backend, "could not open archive")
            .with_diagnostic(chain.join(" "));
        let diagnostic = error.diagnostic.expect("diagnostic set");
        assert!(!diagnostic.contains("alice"));
        assert!(!diagnostic.contains("secret-project"));
        assert!(diagnostic.contains("<redacted-path>"));
        assert!(diagnostic.contains("Access is denied"));
    }

    #[test]
    fn operation_and_session_and_entry_ids_attach_when_set() {
        let error = ApplicationError::new(ApplicationErrorKind::Conflict, "summary")
            .with_operation_id(OperationId::from_raw(9))
            .with_archive_session_id(ArchiveSessionId::from_raw(3))
            .with_entry_id(EntryId::from_raw(4))
            .with_field("entries[3]");
        assert_eq!(error.operation_id, Some(OperationId::from_raw(9)));
        assert_eq!(
            error.archive_session_id,
            Some(ArchiveSessionId::from_raw(3))
        );
        assert_eq!(error.entry_id, Some(EntryId::from_raw(4)));
        assert_eq!(error.field.as_deref(), Some("entries[3]"));
    }
}

mod serialization_snapshots {
    use arclain_app::archive::{
        ArchiveEntryDto, ArchivePath, ArchiveSnapshot, EntryKind, EntryPage, EntrySortKey,
    };
    use arclain_app::error::{
        ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction,
    };
    use arclain_app::ids::{ArchiveSessionId, EntryId, OperationId};

    #[test]
    fn application_error_json_field_names_and_snake_case_enums() {
        let error = ApplicationError::new(ApplicationErrorKind::PermissionDenied, "denied")
            .with_diagnostic("diagnostic text")
            .with_recoverability(Recoverability::UserAction)
            .with_retryable(true)
            .with_suggested_action(SuggestedAction::CheckPermissions)
            .with_operation_id(OperationId::from_raw(1))
            .with_archive_session_id(ArchiveSessionId::from_raw(2))
            .with_entry_id(EntryId::from_raw(3))
            .with_path("safe/relative/path.txt")
            .with_field("archive_password");

        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "permission_denied",
                "summary": "denied",
                "diagnostic": "diagnostic text",
                "recoverability": "user_action",
                "retryable": true,
                "suggested_action": "check_permissions",
                "correlation_id": error.correlation_id.into_raw(),
                "operation_id": 1,
                "archive_session_id": 2,
                "entry_id": 3,
                "path": "safe/relative/path.txt",
                "field": "archive_password",
            })
        );
    }

    #[test]
    fn application_error_json_uses_null_for_absent_optional_fields() {
        let error = ApplicationError::new(ApplicationErrorKind::Cancelled, "cancelled");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "cancelled",
                "summary": "cancelled",
                "diagnostic": null,
                "recoverability": "fatal",
                "retryable": false,
                "suggested_action": null,
                "correlation_id": error.correlation_id.into_raw(),
                "operation_id": null,
                "archive_session_id": null,
                "entry_id": null,
                "path": null,
                "field": null,
            })
        );
    }

    #[test]
    fn archive_snapshot_json_field_names() {
        let snapshot = ArchiveSnapshot {
            session_id: ArchiveSessionId::from_raw(11),
            revision: 4,
            source_path: std::path::PathBuf::from("archive.zip"),
            archive_type: "zip".to_string(),
            entry_count: 2,
            total_uncompressed_size: 4096,
            comment: Some("a comment".to_string()),
            metadata: Some(serde_json::json!({"created_by": "arclain"})),
        };

        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "session_id": 11,
                "revision": 4,
                "source_path": "archive.zip",
                "archive_type": "zip",
                "entry_count": 2,
                "total_uncompressed_size": 4096,
                "comment": "a comment",
                "metadata": {"created_by": "arclain"},
            })
        );
    }

    #[test]
    fn entry_page_json_field_names() {
        let page = EntryPage {
            session_id: ArchiveSessionId::from_raw(1),
            revision: 1,
            directory: ArchivePath::root(),
            total: 1,
            entries: vec![ArchiveEntryDto {
                id: EntryId::from_raw(9),
                path: ArchivePath::parse("dir/file.txt").unwrap(),
                name: "file.txt".to_string(),
                kind: EntryKind::File,
                compressed_size: Some(10),
                uncompressed_size: 20,
                modified_at_unix_ms: Some(1_700_000_000_000),
                encrypted: true,
                crc32: Some("deadbeef".to_string()),
            }],
        };

        let value = serde_json::to_value(&page).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "session_id": 1,
                "revision": 1,
                "directory": "",
                "total": 1,
                "entries": [{
                    "id": 9,
                    "path": "dir/file.txt",
                    "name": "file.txt",
                    "kind": "file",
                    "compressed_size": 10,
                    "uncompressed_size": 20,
                    "modified_at_unix_ms": 1_700_000_000_000i64,
                    "encrypted": true,
                    "crc32": "deadbeef",
                }],
            })
        );
    }

    #[test]
    fn entry_kind_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(EntryKind::Directory).unwrap(),
            serde_json::json!("directory")
        );
        assert_eq!(
            serde_json::to_value(EntryKind::File).unwrap(),
            serde_json::json!("file")
        );
        assert_eq!(
            serde_json::to_value(EntryKind::Symlink).unwrap(),
            serde_json::json!("symlink")
        );
    }

    #[test]
    fn archive_path_serializes_as_a_plain_string() {
        let path = ArchivePath::parse("dir/file.txt").unwrap();
        assert_eq!(
            serde_json::to_value(&path).unwrap(),
            serde_json::json!("dir/file.txt")
        );
    }

    #[test]
    fn entry_sort_key_variants_serialize_snake_case() {
        let cases = [
            (EntrySortKey::Compressed, "compressed"),
            (EntrySortKey::Crc32, "crc32"),
            (EntrySortKey::Encrypted, "encrypted"),
            (EntrySortKey::Kind, "kind"),
            (EntrySortKey::Modified, "modified"),
            (EntrySortKey::Name, "name"),
            (EntrySortKey::Ratio, "ratio"),
            (EntrySortKey::Size, "size"),
        ];
        for (variant, expected) in cases {
            assert_eq!(
                serde_json::to_value(variant).unwrap(),
                serde_json::json!(expected)
            );
        }
    }
}

mod archive_path_tests {
    use arclain_app::archive::ArchivePath;
    use arclain_app::error::ApplicationErrorKind;

    #[test]
    fn root_is_the_empty_path() {
        assert_eq!(ArchivePath::root().as_str(), "");
    }

    #[test]
    fn parse_accepts_relative_slash_paths() {
        let path = ArchivePath::parse("dir/sub/file.txt").unwrap();
        assert_eq!(path.as_str(), "dir/sub/file.txt");
    }

    #[test]
    fn parse_of_empty_string_equals_root() {
        assert_eq!(ArchivePath::parse("").unwrap(), ArchivePath::root());
    }

    #[test]
    fn parse_normalizes_backslashes_to_forward_slashes() {
        let path = ArchivePath::parse("dir\\sub\\file.txt").unwrap();
        assert_eq!(path.as_str(), "dir/sub/file.txt");
    }

    #[test]
    fn parse_rejects_parent_traversal() {
        let err = ArchivePath::parse("../secret").unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);

        let err = ArchivePath::parse("dir/../../secret").unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn parse_rejects_absolute_unix_paths() {
        let err = ArchivePath::parse("/etc/passwd").unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn parse_rejects_absolute_windows_paths() {
        let err = ArchivePath::parse("C:\\Windows\\System32").unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);

        let err = ArchivePath::parse("C:/Windows/System32").unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn parse_rejects_nul_bytes() {
        let err = ArchivePath::parse("dir/\0/file").unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    }
}

/// Exhaustive serialization coverage for the new `PipelineRequest`
/// enums (`PipelineSpecDto`/`PipelineDestinationDto`/
/// `OutputCollisionPolicyDto`/`PipelineStepDto`/`CompressionLevelDto`),
/// matching `serialization_snapshots`'s own per-variant style above.
mod pipeline_request_dtos {
    use arclain_app::operations::pipeline::{
        CompressionLevelDto, OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto,
        PipelineSpecDto, PipelineStepDto,
    };
    use std::path::PathBuf;

    #[test]
    fn compression_level_dto_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(CompressionLevelDto::Fast).unwrap(),
            serde_json::json!("fast")
        );
        assert_eq!(
            serde_json::to_value(CompressionLevelDto::Normal).unwrap(),
            serde_json::json!("normal")
        );
        assert_eq!(
            serde_json::to_value(CompressionLevelDto::Max).unwrap(),
            serde_json::json!("max")
        );
    }

    #[test]
    fn output_collision_policy_dto_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(OutputCollisionPolicyDto::Fail).unwrap(),
            serde_json::json!("fail")
        );
        assert_eq!(
            serde_json::to_value(OutputCollisionPolicyDto::Skip).unwrap(),
            serde_json::json!("skip")
        );
        assert_eq!(
            serde_json::to_value(OutputCollisionPolicyDto::Overwrite).unwrap(),
            serde_json::json!("overwrite")
        );
        assert_eq!(
            serde_json::to_value(OutputCollisionPolicyDto::Smart).unwrap(),
            serde_json::json!("smart")
        );
    }

    #[test]
    fn output_artifact_dto_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(OutputArtifactDto::Archive).unwrap(),
            serde_json::json!("archive")
        );
        assert_eq!(
            serde_json::to_value(OutputArtifactDto::Folder).unwrap(),
            serde_json::json!("folder")
        );
    }

    #[test]
    fn output_artifact_dto_omitted_from_json_deserializes_to_the_archive_default() {
        // Pins the "defaulting to Archive" contract at the wire level,
        // not just in a doc comment: a `Steps` payload from an older
        // bridge build that never learned about `output_artifact` must
        // still deserialize, as `Archive` -- matching the Process page's
        // own dropdown default -- rather than failing to parse.
        let steps = PipelineSpecDto::Steps {
            steps: vec![PipelineStepDto::Flatten {
                strip_common_prefix: false,
                max_depth: 1,
            }],
            output_artifact: OutputArtifactDto::Archive,
        };
        let mut value = serde_json::to_value(&steps).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("output_artifact")
            .expect("output_artifact must be present in a normal serialization");
        let deserialized: PipelineSpecDto = serde_json::from_value(value)
            .expect("a Steps payload omitting output_artifact must still deserialize");
        match deserialized {
            PipelineSpecDto::Steps {
                output_artifact, ..
            } => {
                assert_eq!(output_artifact, OutputArtifactDto::Archive);
            }
            other => panic!("expected Steps, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_destination_dto_variants_round_trip() {
        assert_eq!(
            serde_json::to_value(PipelineDestinationDto::SameFolder).unwrap(),
            serde_json::json!({"type": "same_folder"})
        );
        let folder = PipelineDestinationDto::Folder {
            path: PathBuf::from("/out/dir"),
        };
        let value = serde_json::to_value(&folder).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "folder", "path": "/out/dir"})
        );
        let round_tripped: PipelineDestinationDto = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, folder);
    }

    #[test]
    fn pipeline_step_dto_variants_round_trip() {
        let flatten = PipelineStepDto::Flatten {
            strip_common_prefix: true,
            max_depth: 2,
        };
        let value = serde_json::to_value(&flatten).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "flatten", "strip_common_prefix": true, "max_depth": 2})
        );
        assert_eq!(
            serde_json::from_value::<PipelineStepDto>(value).unwrap(),
            flatten
        );

        let organize = PipelineStepDto::Organize {
            rule_id: "42".to_string(),
        };
        let value = serde_json::to_value(&organize).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "organize", "rule_id": "42"})
        );
        assert_eq!(
            serde_json::from_value::<PipelineStepDto>(value).unwrap(),
            organize
        );

        let convert = PipelineStepDto::Convert {
            format: "zip".to_string(),
            compression: CompressionLevelDto::Normal,
        };
        let value = serde_json::to_value(&convert).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "convert", "format": "zip", "compression": "normal"})
        );
        assert_eq!(
            serde_json::from_value::<PipelineStepDto>(value).unwrap(),
            convert
        );
    }

    #[test]
    fn pipeline_spec_dto_variants_round_trip() {
        let preset = PipelineSpecDto::Preset {
            id: "RE Mod Cleanup".to_string(),
        };
        let value = serde_json::to_value(&preset).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "preset", "id": "RE Mod Cleanup"})
        );
        assert_eq!(
            serde_json::from_value::<PipelineSpecDto>(value).unwrap(),
            preset
        );

        // Explicitly non-default (`Folder`), so this round-trip actually
        // exercises serializing/deserializing the field's real value,
        // not just its passively-matching default.
        let steps = PipelineSpecDto::Steps {
            steps: vec![PipelineStepDto::Flatten {
                strip_common_prefix: false,
                max_depth: 1,
            }],
            output_artifact: OutputArtifactDto::Folder,
        };
        let value = serde_json::to_value(&steps).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "steps",
                "steps": [
                    {"type": "flatten", "strip_common_prefix": false, "max_depth": 1}
                ],
                "output_artifact": "folder"
            })
        );
        assert_eq!(
            serde_json::from_value::<PipelineSpecDto>(value).unwrap(),
            steps
        );
    }
}
