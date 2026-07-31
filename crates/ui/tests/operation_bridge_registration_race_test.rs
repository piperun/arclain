//! Regression coverage for operation-bridge reconciliation when a terminal
//! event arrives before registration or is lost to a lagged subscriber.

mod common;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::event::OperationState;
use common::{build_zip_fixture, create_test_shared_state_with_facade};
use std::time::Duration;

async fn wait_for_terminal(
    app: &arclain_app::ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = app.operation(operation_id).await.unwrap();
        if matches!(
            &snapshot.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        ) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation did not finish within the test deadline; last state: {:?}",
            snapshot.state,
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[test]
fn register_operation_reconciles_an_operation_that_already_finished_before_registration() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let archive = build_zip_fixture(temp.path(), "already-finished.zip");
    let app = shared.facade.as_ref().unwrap().clone();
    let runtime = shared.services.tokio_runtime.handle().clone();
    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;
    tab.pending_open_operation
        .set(Some(arclain_app::ids::OperationId::from_raw(999_999)));

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive,
                password: None,
            })
            .await
            .expect("start archive open");
        wait_for_terminal(&app, operation_id).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
    });

    assert!(tab.pending_open_operation.get().is_none());
}

#[test]
fn register_operation_clears_tracking_even_when_the_operation_is_entirely_unknown() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let runtime = shared.services.tokio_runtime.handle().clone();
    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;
    let bogus_operation_id = arclain_app::ids::OperationId::from_raw(123_456_789);
    tab.active_extraction_operation
        .set(Some(bogus_operation_id));

    runtime.block_on(async {
        arclain_ui::core::operation_bridge::register_operation(&shared, bogus_operation_id, tab_id)
            .await;
    });

    assert!(tab.active_extraction_operation.get().is_none());
}

#[test]
fn reconcile_after_lag_catches_up_every_tracked_operation_to_its_current_state() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let archive = build_zip_fixture(temp.path(), "lagged.zip");
    let app = shared.facade.as_ref().unwrap().clone();
    let runtime = shared.services.tokio_runtime.handle().clone();
    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive,
                password: None,
            })
            .await
            .expect("start archive open");
        shared.operation_origins.register(operation_id, tab_id);
        tab.pending_open_operation.set(Some(operation_id));
        wait_for_terminal(&app, operation_id).await;

        arclain_ui::core::operation_bridge::reconcile_after_lag(
            &shared,
            &shared.operation_origins,
            &runtime,
            &app,
            1,
        )
        .await;
    });

    assert!(tab.pending_open_operation.get().is_none());
}
