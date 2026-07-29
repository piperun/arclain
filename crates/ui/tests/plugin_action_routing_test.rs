//! Routing coverage for `OperationKind::PluginAction` events reaching
//! `crate::core::operation_bridge`, and for the dispatcher that starts
//! them.
//!
//! Complements `plugin_session_facade_test.rs` (which drives real WASM
//! fixtures through the registry): these exercise the *bridge* half
//! against a minimal `SharedState`, where an arbitrary terminal event can
//! be constructed directly — including the ones no fixture plugin can
//! currently produce, which is exactly why they need pinning here rather
//! than waiting for a fixture that can.

mod common;

use common::create_test_shared_state;

use arclain_app::error::{ApplicationError, ApplicationErrorKind};
use arclain_app::event::{OperationEvent, OperationKind, OperationResult, OperationState};
use arclain_app::ids::{OperationId, PluginSessionId};
use arclain_app::plugins::{
    PluginActionDto, PluginExtensionPointDto, PluginUiDocument, PluginUiNodeDto, PluginUiNodeKind,
    PluginUiUpdate,
};
use arclain_ui::core::operation_bridge::handle_plugin_action_event;
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::PluginSlot;
use arclain_ui::features::plugins::presentation::document_dispatch;

fn panel_slot() -> PluginSlot {
    PluginSlot::Panel {
        plugin_id: "demo".to_string(),
        tab: TabId(1),
        archive_session: Some(arclain_app::ids::ArchiveSessionId::from_raw(1)),
    }
}

fn document(session: u64, revision: u64) -> PluginUiDocument {
    PluginUiDocument {
        session_id: PluginSessionId::from_raw(session),
        plugin_id: "demo".to_string(),
        region_id: "panel".to_string(),
        extension_point: PluginExtensionPointDto::Panel,
        revision,
        root: PluginUiNodeDto {
            id: "#root".to_string(),
            kind: PluginUiNodeKind::Single {
                children: Vec::new(),
            },
            visible: true,
            enabled: true,
        },
    }
}

fn event(operation_id: u64, state: OperationState) -> OperationEvent {
    OperationEvent {
        operation_id: OperationId::from_raw(operation_id),
        sequence: 1,
        kind: OperationKind::PluginAction,
        state,
    }
}

/// A terminal completion carrying a result this path does not recognize
/// must still drain the registry.
///
/// `OperationResult` is a shared enum that grows with every task that adds
/// an operation kind, so "completed with something other than
/// `PluginUiUpdated`" is a state the bridge must survive rather than
/// assume away. Holding the entry would leak the operation->slot index and
/// the slot's `inflight` list for the life of the process.
#[test]
fn a_terminal_completion_without_a_document_drains_the_registry() {
    let shared = create_test_shared_state();
    let slot = panel_slot();
    shared
        .plugin_sessions
        .adopt_for_test(&slot, PluginSessionId::from_raw(5), document(5, 1));
    shared
        .plugin_sessions
        .track(&slot, OperationId::from_raw(70));
    assert!(shared.plugin_sessions.tracks(OperationId::from_raw(70)));

    handle_plugin_action_event(
        &shared,
        event(
            70,
            OperationState::Completed {
                result: OperationResult::None,
            },
        ),
    );

    assert!(
        !shared.plugin_sessions.tracks(OperationId::from_raw(70)),
        "any terminal completion must drain, not only PluginUiUpdated"
    );
    assert!(shared.plugin_sessions.tracked_ids().is_empty());
}

/// A recognized completion applies its document *and* drains.
#[test]
fn a_document_completion_applies_and_drains() {
    let shared = create_test_shared_state();
    let slot = panel_slot();
    shared
        .plugin_sessions
        .adopt_for_test(&slot, PluginSessionId::from_raw(5), document(5, 1));
    shared
        .plugin_sessions
        .track(&slot, OperationId::from_raw(70));

    handle_plugin_action_event(
        &shared,
        event(
            70,
            OperationState::Completed {
                result: OperationResult::PluginUiUpdated {
                    update: PluginUiUpdate {
                        document: document(5, 2),
                        intents: Vec::new(),
                    },
                },
            },
        ),
    );

    assert!(shared.plugin_sessions.tracked_ids().is_empty());
    assert_eq!(
        shared.plugin_sessions.session_id(&slot),
        Some(PluginSessionId::from_raw(5))
    );
}

/// A failed action drains the registry and surfaces a toast, while leaving
/// the slot's last good document in place — a failed interaction is not a
/// reason to blank a panel the user is looking at.
#[test]
fn a_failed_action_drains_the_registry_and_toasts_without_clearing_the_slot() {
    let shared = create_test_shared_state();
    let slot = panel_slot();
    shared
        .plugin_sessions
        .adopt_for_test(&slot, PluginSessionId::from_raw(5), document(5, 1));
    shared
        .plugin_sessions
        .track(&slot, OperationId::from_raw(70));

    handle_plugin_action_event(
        &shared,
        event(
            70,
            OperationState::Failed {
                error: ApplicationError::new(
                    ApplicationErrorKind::Plugin,
                    "plugin execution failed",
                ),
            },
        ),
    );

    assert!(shared.plugin_sessions.tracked_ids().is_empty());
    assert_eq!(
        shared.plugin_sessions.session_id(&slot),
        Some(PluginSessionId::from_raw(5)),
        "a failed action keeps the slot's session and last document"
    );
    assert_eq!(shared.toaster.lock().len(), 1);
}

/// Non-terminal states must neither drain nor apply.
#[test]
fn non_terminal_states_leave_the_registry_untouched() {
    let shared = create_test_shared_state();
    let slot = panel_slot();
    shared
        .plugin_sessions
        .adopt_for_test(&slot, PluginSessionId::from_raw(5), document(5, 1));
    shared
        .plugin_sessions
        .track(&slot, OperationId::from_raw(70));

    for state in [
        OperationState::Accepted,
        OperationState::Started,
        OperationState::Progress {
            completed_units: 1,
            total_units: None,
            message: None,
        },
    ] {
        handle_plugin_action_event(&shared, event(70, state));
        assert!(
            shared.plugin_sessions.tracks(OperationId::from_raw(70)),
            "a non-terminal state must not drain the registry"
        );
    }
}

/// An event for an operation this registry never started is ignored
/// entirely — another part of the application may legitimately be running
/// plugin actions of its own.
#[test]
fn an_untracked_plugin_action_event_is_ignored() {
    let shared = create_test_shared_state();

    handle_plugin_action_event(
        &shared,
        event(
            999,
            OperationState::Failed {
                error: ApplicationError::new(ApplicationErrorKind::Plugin, "not ours"),
            },
        ),
    );

    assert!(
        shared.toaster.lock().is_empty(),
        "an untracked operation must not surface a toast"
    );
}

/// `document_dispatch::dispatch_action` is the function the registration
/// race fix lives in, so it needs coverage of its own guards rather than
/// only being exercised indirectly.
///
/// With no facade wired (the minimal test `SharedState`), it must return
/// without spawning anything and without touching the registry — the
/// branch every hand-built test fixture in this crate takes.
#[test]
fn dispatch_action_is_inert_without_a_facade() {
    let shared = create_test_shared_state();
    assert!(shared.facade.is_none(), "precondition for this branch");
    let slot = panel_slot();
    shared
        .plugin_sessions
        .adopt_for_test(&slot, PluginSessionId::from_raw(5), document(5, 1));

    document_dispatch::dispatch_action(&shared, &slot, "go".to_string(), PluginActionDto::Activate);

    assert!(shared.plugin_sessions.tracked_ids().is_empty());
    assert_eq!(
        shared.plugin_sessions.session_id(&slot),
        Some(PluginSessionId::from_raw(5)),
        "an inert dispatch must not disturb the slot"
    );
}
