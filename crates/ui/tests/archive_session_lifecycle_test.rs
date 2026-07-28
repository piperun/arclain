//! Regression coverage for `close_archive_session`/`cancel_archive_open`:
//! the UI-side wiring that releases a tab's facade-side archive session
//! (and cancels an in-flight open) whenever that tab closes or its
//! active archive gets replaced by a different one. Before this, every
//! `archive_session_id` overwrite/discard silently leaked the facade's
//! session -- there was no `close_archive` call site anywhere in this
//! crate.
//!
//! Covers both the guard clauses (nothing to act on; no facade
//! available -- dispatcher-style fixtures) and, against a real
//! bootstrapped facade, that a closed session actually becomes
//! unreachable through the facade afterward.

mod common;
use common::create_test_shared_state;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A `None` session id (nothing was ever open in the tab being closed
/// or replaced) must return immediately without touching the facade at
/// all.
#[test]
fn close_archive_session_is_a_no_op_when_nothing_was_ever_open() {
    let shared = create_test_shared_state();
    arclain_ui::core::operations::archive::close_archive_session(&shared, None);
}

/// A `Some` session id with no facade available (dispatcher-style test
/// fixtures skip a full `ArclainApp::bootstrap` -- see `SharedState::
/// facade`'s own doc comment) must also return without panicking,
/// mirroring every other facade call site's "missing service -> no-op"
/// convention.
#[test]
fn close_archive_session_is_a_no_op_without_a_facade() {
    let shared = create_test_shared_state();
    assert!(
        shared.facade.is_none(),
        "this fixture must not have a facade for this test to mean anything"
    );
    let session_id = arclain_app::ids::ArchiveSessionId::from_raw(1);
    arclain_ui::core::operations::archive::close_archive_session(&shared, Some(session_id));
}

/// A tab with no archive-open operation currently tracked must return
/// immediately without touching the facade at all.
#[test]
fn cancel_archive_open_is_a_no_op_when_nothing_is_pending() {
    let shared = create_test_shared_state();
    let tab = Arc::new(arclain_ui::core::tabs::TabState::new(
        arclain_ui::core::tabs::TabId(1),
    ));
    assert!(tab.pending_open_operation.get().is_none());
    arclain_ui::core::operations::archive::cancel_archive_open(&shared, &tab);
}

/// A tracked archive-open operation with no facade available must also
/// return without panicking.
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

/// A backend whose `list()` always succeeds with one fake entry, purely
/// so `start_open_archive` can reach `Completed` and mint a real
/// session for `close_archive_session` to release.
struct AlwaysSucceedsBackend;

impl arclain_core::ArchiveBackend for AlwaysSucceedsBackend {
    fn name(&self) -> &str {
        "always-succeeds"
    }
    fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
        arclain_core::archive::BackendCapabilities::read_only()
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
        Ok(arclain_core::archive::ArchiveKind::Zip)
    }
    fn list(
        &self,
        _path: &Path,
        _password: Option<&str>,
    ) -> anyhow::Result<arclain_core::archive::ArchiveInfo> {
        Ok(arclain_core::archive::ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: arclain_core::archive::ArchiveKind::Zip,
            entries: vec![arclain_core::archive::ArchiveEntry {
                path: "a.txt".to_string(),
                size: 1,
                packed_size: 1,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
            encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
        })
    }
    fn extract_all(&self, _: &Path, _: &Path, _: Option<&str>) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_files(
        &self,
        _: &Path,
        _: &Path,
        _: &[String],
        _: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_directory(
        &self,
        _: &Path,
        _: &Path,
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn recompress_7z(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_files(&self, _: &Path, _: &[PathBuf]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn create_archive(&self, _: &Path, _: &[PathBuf], _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn read_text_file(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
    fn delete_files(&self, _: &Path, _: &[String]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_or_update_file_from_str(&self, _: &Path, _: &str, _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn convert_to_7z(&self, _: &arclain_core::Archive, _: &Path, _: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn crc32_of_entry(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
}

/// End-to-end: `close_archive_session` against a real bootstrapped
/// facade actually releases the session -- it does not just fire an
/// operation the facade silently drops. Mirrors `arclain_app`'s own
/// `archive_sessions.rs` "close then probe for NotFound" pattern.
#[test]
fn close_archive_session_actually_releases_the_session_through_a_real_facade() {
    let temp = tempfile::tempdir().unwrap();
    let paths = arclain_app::AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(AlwaysSucceedsBackend);
    let app = arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: None,
    })
    .expect("bootstrap must succeed against a bare temp-dir AppPaths");

    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();

    let session_id = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: temp.path().join("fixture.zip"),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = app.operation(operation_id).await.unwrap();
            if let arclain_app::event::OperationState::Completed {
                result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
            } = snapshot.state
            {
                break snapshot.session_id;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "open did not complete within the test deadline"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    // Sanity check: the session is really open before the close.
    runtime.block_on(async {
        app.archive_snapshot(session_id)
            .await
            .expect("the session must be reachable before it is closed");
    });

    arclain_ui::core::operations::archive::close_archive_session(&shared, Some(session_id));

    runtime.block_on(async {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match app.archive_snapshot(session_id).await {
                Err(error) if error.kind == arclain_app::error::ApplicationErrorKind::NotFound => {
                    break
                }
                _ if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!(
                    "close_archive_session did not release the session within the test \
                     deadline: {other:?}"
                ),
            }
        }
    });
}
