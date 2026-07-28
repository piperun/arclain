//! Public-contract tests for the operation-event and challenge types Task
//! 3 introduces.
//!
//! `OperationRegistry` itself is `pub(crate)` (crate-private): an
//! integration test under `tests/` is compiled as its own separate crate
//! and, like any other external consumer, cannot name a `pub(crate)` item
//! -- the same way it cannot reach any other crate-private type in any
//! crate. Its own inline test suite lives next to it in
//! `crates/app/src/operations/registry.rs`, matching how every other
//! `pub(crate)` type in this workspace is tested (see e.g.
//! `crates/plugins/src/manager/mod.rs`'s `#[cfg(test)] mod tests;`).
//!
//! This file instead exercises everything Task 3 adds that IS public: the
//! `Challenge`/`ChallengeResponse`/`OperationKind`/`OperationState`/
//! `OperationEvent`/`OperationSnapshot`/`OperationResult` read models a
//! frontend or bridge actually receives.
//!
//! The "a supplied password cannot leak into the event/snapshot stream"
//! regression test also lives in `registry.rs`, not here, for the same
//! reason: proving that end-to-end requires actually driving a
//! `resolve_challenge` call with a real `ChallengeResponse::Password`, which
//! needs the `pub(crate)` registry. A version of that test written against
//! only the types reachable from this file could never fail regardless of
//! the registry's behavior -- `Challenge` (the only thing that flows
//! through `OperationEvent`) structurally never carries a secret, so
//! serializing `every_operation_state()` and checking for a marker string
//! would hold trivially no matter what the registry does with a response's
//! actual secret.

use arclain_app::challenge::Challenge;
use arclain_app::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use arclain_app::event::{
    OperationEvent, OperationKind, OperationResult, OperationSnapshot, OperationState,
};
use arclain_app::ids::{ArchiveSessionId, ChallengeId, OperationId};

/// Builds one instance of every `OperationState` variant, cycling through
/// every `Challenge` variant for the `Challenge` state, so the tests below
/// can iterate a complete set without each repeating the constructions.
fn every_operation_state() -> Vec<OperationState> {
    vec![
        OperationState::Accepted,
        OperationState::Started,
        OperationState::Progress {
            completed_units: 3,
            total_units: Some(10),
            message: Some("copying".to_string()),
        },
        OperationState::Challenge {
            challenge: Challenge::Password {
                id: ChallengeId::from_raw(1),
                archive_name: "archive.zip".to_string(),
                attempt: 1,
            },
        },
        OperationState::Challenge {
            challenge: Challenge::ConfirmOverwrite {
                id: ChallengeId::from_raw(2),
                destination: std::path::PathBuf::from("out/archive.zip"),
            },
        },
        OperationState::Challenge {
            challenge: Challenge::ConfirmDestructiveAction {
                id: ChallengeId::from_raw(3),
                summary: "delete 400 files".to_string(),
            },
        },
        OperationState::Challenge {
            challenge: Challenge::MissingExternalTool {
                id: ChallengeId::from_raw(4),
                tool: "unrar".to_string(),
            },
        },
        OperationState::Challenge {
            challenge: Challenge::RetryPermission {
                id: ChallengeId::from_raw(5),
                path: std::path::PathBuf::from("locked.txt"),
            },
        },
        OperationState::SnapshotChanged {
            session_id: ArchiveSessionId::from_raw(1),
            revision: 2,
        },
        OperationState::Completed {
            result: OperationResult::None,
        },
        OperationState::Cancelled,
        OperationState::Failed {
            error: ApplicationError::new(ApplicationErrorKind::Backend, "boom")
                .with_recoverability(Recoverability::Retry),
        },
    ]
}

#[test]
fn constructs_every_operation_state_and_wraps_it_in_an_event() {
    for (index, state) in every_operation_state().into_iter().enumerate() {
        let event = OperationEvent {
            operation_id: OperationId::from_raw(1),
            sequence: index as u64 + 1,
            kind: OperationKind::Extract,
            state: state.clone(),
        };
        assert_eq!(event.state, state);
    }
}

mod serialization_snapshots {
    use super::*;

    #[test]
    fn operation_kind_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(OperationKind::ArchiveModify).unwrap(),
            serde_json::json!("archive_modify")
        );
        assert_eq!(
            serde_json::to_value(OperationKind::OpenArchive).unwrap(),
            serde_json::json!("open_archive")
        );
        assert_eq!(
            serde_json::to_value(OperationKind::PluginAction).unwrap(),
            serde_json::json!("plugin_action")
        );
    }

    #[test]
    fn operation_result_none_serializes_as_an_adjacently_tagged_variant() {
        let value = serde_json::to_value(OperationResult::None).unwrap();
        assert_eq!(value, serde_json::json!({"type": "none"}));
    }

    #[test]
    fn operation_state_challenge_serializes_with_the_state_tag() {
        let state = OperationState::Challenge {
            challenge: Challenge::Password {
                id: ChallengeId::from_raw(9),
                archive_name: "archive.zip".to_string(),
                attempt: 2,
            },
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "state": "challenge",
                "challenge": {
                    "kind": "password",
                    "id": 9,
                    "archive_name": "archive.zip",
                    "attempt": 2,
                }
            })
        );
    }

    #[test]
    fn operation_state_progress_serializes_with_its_fields() {
        let state = OperationState::Progress {
            completed_units: 3,
            total_units: Some(10),
            message: Some("copying".to_string()),
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "state": "progress",
                "completed_units": 3,
                "total_units": 10,
                "message": "copying",
            })
        );
    }

    #[test]
    fn operation_event_json_field_names() {
        let event = OperationEvent {
            operation_id: OperationId::from_raw(1),
            sequence: 4,
            kind: OperationKind::Convert,
            state: OperationState::Started,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "operation_id": 1,
                "sequence": 4,
                "kind": "convert",
                "state": {"state": "started"},
            })
        );
    }

    #[test]
    fn operation_snapshot_json_field_names() {
        let snapshot = OperationSnapshot {
            operation_id: OperationId::from_raw(2),
            kind: OperationKind::Organize,
            last_sequence: 7,
            state: OperationState::Cancelled,
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "operation_id": 2,
                "kind": "organize",
                "last_sequence": 7,
                "state": {"state": "cancelled"},
            })
        );
    }
}

mod challenge_tests {
    use super::*;

    #[test]
    fn every_challenge_variant_round_trips_through_json() {
        let challenges = vec![
            Challenge::Password {
                id: ChallengeId::from_raw(1),
                archive_name: "archive.zip".to_string(),
                attempt: 1,
            },
            Challenge::ConfirmOverwrite {
                id: ChallengeId::from_raw(2),
                destination: std::path::PathBuf::from("out/archive.zip"),
            },
            Challenge::ConfirmDestructiveAction {
                id: ChallengeId::from_raw(3),
                summary: "delete 400 files".to_string(),
            },
            Challenge::MissingExternalTool {
                id: ChallengeId::from_raw(4),
                tool: "unrar".to_string(),
            },
            Challenge::RetryPermission {
                id: ChallengeId::from_raw(5),
                path: std::path::PathBuf::from("locked.txt"),
            },
        ];
        for challenge in challenges {
            let json = serde_json::to_string(&challenge).unwrap();
            let round_tripped: Challenge = serde_json::from_str(&json).unwrap();
            assert_eq!(round_tripped, challenge);
        }
    }

    #[test]
    fn challenge_json_uses_the_kind_tag() {
        let challenge = Challenge::MissingExternalTool {
            id: ChallengeId::from_raw(4),
            tool: "unrar".to_string(),
        };
        let value = serde_json::to_value(&challenge).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "missing_external_tool",
                "id": 4,
                "tool": "unrar",
            })
        );
    }

    #[test]
    fn challenge_id_accessor_matches_every_variant() {
        let id = ChallengeId::from_raw(11);
        let challenge = Challenge::RetryPermission {
            id,
            path: std::path::PathBuf::from("locked.txt"),
        };
        assert_eq!(challenge.id(), id);
    }
}

mod challenge_response_tests {
    use arclain_app::challenge::{ChallengeResponse, SecretInput};
    use arclain_app::ids::ChallengeId;

    #[test]
    fn challenge_response_id_accessor_matches_every_variant() {
        let id = ChallengeId::from_raw(3);
        let response = ChallengeResponse::ConfirmDestructiveAction {
            id,
            confirmed: true,
        };
        assert_eq!(response.id(), id);
    }

    #[test]
    fn password_response_exposes_the_secret_it_was_built_with() {
        let id = ChallengeId::from_raw(1);
        let response = ChallengeResponse::Password {
            id,
            value: SecretInput::new("hunter2".to_string()),
        };
        match &response {
            ChallengeResponse::Password { value, .. } => {
                assert_eq!(value.expose_secret(), "hunter2");
            }
            _ => panic!("expected a Password response"),
        }
    }

    #[test]
    fn debug_formatting_a_password_response_does_not_leak_the_secret() {
        let response = ChallengeResponse::Password {
            id: ChallengeId::from_raw(1),
            value: SecretInput::new("hunter2".to_string()),
        };
        let debug_output = format!("{response:?}");
        assert!(!debug_output.contains("hunter2"));
    }
}
