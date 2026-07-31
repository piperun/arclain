//! Regression coverage for the UI wiring that closes facade-owned archive
//! sessions when tabs close, archives are replaced, or completion arrives
//! after the target tab has disappeared.

mod common;

use common::{build_zip_fixture, create_test_shared_state, create_test_shared_state_with_facade};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn close_archive_session_is_a_no_op_when_nothing_was_ever_open() {
    let shared = create_test_shared_state();
    arclain_ui::core::operations::archive::close_archive_session(&shared, None);
}

#[test]
fn close_archive_session_is_a_no_op_without_a_facade() {
    let shared = create_test_shared_state();
    assert!(shared.facade.is_none());
    let session_id = arclain_app::ids::ArchiveSessionId::from_raw(1);
    arclain_ui::core::operations::archive::close_archive_session(&shared, Some(session_id));
}

#[test]
fn cancel_archive_open_is_a_no_op_when_nothing_is_pending() {
    let shared = create_test_shared_state();
    let tab = Arc::new(arclain_ui::core::tabs::TabState::new(
        arclain_ui::core::tabs::TabId(1),
    ));
    assert!(tab.pending_open_operation.get().is_none());
    arclain_ui::core::operations::archive::cancel_archive_open(&shared, &tab);
}

#[test]
fn cancel_archive_open_is_a_no_op_without_a_facade() {
    let shared = create_test_shared_state();
    let tab = Arc::new(arclain_ui::core::tabs::TabState::new(
        arclain_ui::core::tabs::TabId(1),
    ));
    tab.pending_open_operation
        .set(Some(arclain_app::ids::OperationId::from_raw(1)));
    arclain_ui::core::operations::archive::cancel_archive_open(&shared, &tab);
}

async fn wait_for_open_completion(
    app: &arclain_app::ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::archive::ArchiveSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = app.operation(operation_id).await.unwrap();
        if let arclain_app::event::OperationState::Completed {
            result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
        } = snapshot.state
        {
            return snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "open did not complete within the test deadline: {:?}",
            snapshot.state
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn assert_session_eventually_closed(
    app: &arclain_app::ArclainApp,
    session_id: arclain_app::ids::ArchiveSessionId,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match app.archive_snapshot(session_id).await {
            Err(error) if error.kind == arclain_app::error::ApplicationErrorKind::NotFound => {
                return;
            }
            _ if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            other => panic!(
                "session {session_id:?} was not released within the test deadline: {other:?}"
            ),
        }
    }
}

#[test]
fn close_archive_session_actually_releases_the_session_through_a_real_facade() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let archive = build_zip_fixture(temp.path(), "fixture.zip");
    let app = shared.facade.as_ref().unwrap().clone();
    let runtime = shared.services.tokio_runtime.handle().clone();

    let session_id = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: archive,
                password: None,
            })
            .await
            .expect("start archive open");
        wait_for_open_completion(&app, operation_id)
            .await
            .session_id
    });

    runtime.block_on(async {
        app.archive_snapshot(session_id)
            .await
            .expect("session must exist before close");
    });
    arclain_ui::core::operations::archive::close_archive_session(&shared, Some(session_id));
    runtime.block_on(assert_session_eventually_closed(&app, session_id));
}

#[test]
fn a_second_open_on_the_same_tab_releases_the_prior_session() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let first = build_zip_fixture(temp.path(), "first.zip");
    let second = build_zip_fixture(temp.path(), "second.zip");
    let app = shared.facade.as_ref().unwrap().clone();
    let runtime = shared.services.tokio_runtime.handle().clone();
    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    let (session_a, session_b) = runtime.block_on(async {
        let operation_a = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: first,
                password: None,
            })
            .await
            .expect("start first archive open");
        wait_for_open_completion(&app, operation_a).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_a, tab_id).await;
        let session_a = tab.archive_session_id.get().expect("stamp first session");

        let operation_b = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: second,
                password: None,
            })
            .await
            .expect("start second archive open");
        wait_for_open_completion(&app, operation_b).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_b, tab_id).await;
        let session_b = tab.archive_session_id.get().expect("stamp second session");
        assert_ne!(session_a, session_b);
        (session_a, session_b)
    });

    runtime.block_on(assert_session_eventually_closed(&app, session_a));
    runtime.block_on(async {
        app.archive_snapshot(session_b)
            .await
            .expect("current session must remain open");
    });
}

#[test]
fn a_session_minted_for_a_tab_that_is_already_gone_is_closed_not_leaked() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let archive = build_zip_fixture(temp.path(), "fixture.zip");
    let app = shared.facade.as_ref().unwrap().clone();
    let runtime = shared.services.tokio_runtime.handle().clone();
    let tab_id = shared.signals().tabs.get().active_id();

    {
        let mut tabs = shared.signals().tabs.get();
        tabs.open(None);
        shared.signals().tabs.set(tabs);
    }

    let session_id = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: archive,
                password: None,
            })
            .await
            .expect("start archive open");
        let snapshot = wait_for_open_completion(&app, operation_id).await;
        {
            let mut tabs = shared.signals().tabs.get();
            tabs.force_close(tab_id);
            shared.signals().tabs.set(tabs);
        }
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
        snapshot.session_id
    });

    runtime.block_on(assert_session_eventually_closed(&app, session_id));
}
