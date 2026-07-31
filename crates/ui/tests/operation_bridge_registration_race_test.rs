//! Regression coverage for the operation bridge's reconciliation paths:
//!
//! - `crate::core::operation_bridge::register_operation`: a fast-
//!   failing operation can reach a terminal state before the caller's
//!   `origins.register` call ever runs, since the worker starts running
//!   concurrently with the caller resuming from `start_extract`/
//!   `start_open_archive`'s own `.await`. Reproduced deterministically
//!   here by driving the sequence by hand -- start the operation, poll
//!   until it is *already* terminal, and only then register -- rather
//!   than relying on timing luck to occasionally hit the race.
//! - `crate::core::operation_bridge::reconcile_after_lag`: a lagged
//!   broadcast receiver can drop a terminal event for an operation this
//!   bridge is still tracking. Exercised directly here (calling the
//!   reconciliation function the same way the bridge's own event loop
//!   does on `RecvError::Lagged`) rather than trying to manufacture an
//!   actual channel overflow, which `spawn`'s own single, production-
//!   only subscription is not set up to let a test observe.

mod common;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::event::OperationState;
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_core::archive::{ArchiveInfo, ArchiveKind};
use arclain_core::ArchiveBackend;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A backend whose every `list()` call fails immediately with a
/// non-password error, so `start_open_archive` reaches `Failed` almost
/// instantly -- fast enough that registering the operation's origin
/// only after polling for its terminal state reliably reproduces "the
/// operation already finished before registration" without needing any
/// artificial delay.
struct AlwaysFailsBackend;

impl ArchiveBackend for AlwaysFailsBackend {
    fn name(&self) -> &str {
        "always-fails"
    }
    fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
        arclain_core::archive::BackendCapabilities::read_only()
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
        Ok(ArchiveKind::Zip)
    }
    fn list(&self, _path: &Path, _password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
        Err(anyhow::anyhow!("disk read error: I/O failure"))
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

fn bootstrap_test_app(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    let backend: Arc<dyn ArchiveBackend> = Arc::new(AlwaysFailsBackend);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed against a bare temp-dir AppPaths")
}

/// Reproduces the registration race for an archive-open: the operation
/// reaches `Failed` before `register_operation` (which registers, then
/// immediately reconciles) is ever called. The reconciliation must
/// still forget the origin and clear `pending_open_operation` -- not
/// leave the tab stuck believing an operation is still running.
#[test]
fn register_operation_reconciles_an_operation_that_already_finished_before_registration() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_test_app(&temp);
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.handle().clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;
    tab.pending_open_operation
        .set(Some(arclain_app::ids::OperationId::from_raw(999999)));

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: temp.path().join("bad.zip"),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");

        // Poll until the operation is *already* terminal -- simulating
        // the worst case of the race: registration happens strictly
        // after the operation finished, not merely concurrently with it.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = app.operation(operation_id).await.unwrap();
            if matches!(snapshot.state, OperationState::Failed { .. }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "operation did not fail within the test deadline"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Only now register -- well after the operation already
        // finished, exactly the race `register_operation` exists to
        // recover from.
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;

        assert!(
            tab.pending_open_operation.get().is_none(),
            "reconciliation must clear pending_open_operation for an operation that already \
             finished before registration, not leave the tab believing it is still running"
        );
    });
}

/// Same race, but for an operation id the registry has never heard of
/// at all (`app.operation` returns `NotFound`) -- covers
/// `reconcile_one`'s defensive branch for an operation whose history
/// was already evicted (or that never existed) by the time
/// reconciliation runs. The tab's tracking signal must still be forced
/// back to `None` rather than staying stuck at `Some` forever, since
/// nothing will ever tell this bridge about that operation again.
#[test]
fn register_operation_clears_tracking_even_when_the_operation_is_entirely_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_test_app(&temp);
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
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

    assert!(
        tab.active_extraction_operation.get().is_none(),
        "an entirely-unknown operation must still force the tab's tracking signal back to None, \
         not leave it stuck at Some forever"
    );
}

/// Reproduces what a lagged broadcast receiver would otherwise lose: an
/// operation that finished (and was already correctly registered
/// beforehand, unlike the registration-race tests above) while the
/// bridge's event loop was not reading -- `reconcile_after_lag` must
/// still notice it reached a terminal state and clear the tab's
/// tracking signal, exactly as if the live event had been delivered
/// normally.
#[test]
fn reconcile_after_lag_catches_up_every_tracked_operation_to_its_current_state() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_test_app(&temp);
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.handle().clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: temp.path().join("bad.zip"),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");

        // Registered normally (unlike the registration-race tests
        // above) -- this models an operation the bridge already knew
        // about before the lag occurred.
        shared.operation_origins.register(operation_id, tab_id);
        tab.pending_open_operation.set(Some(operation_id));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = app.operation(operation_id).await.unwrap();
            if matches!(snapshot.state, OperationState::Failed { .. }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "operation did not fail within the test deadline"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // The event that would have told the bridge about this in the
        // normal flow is gone by construction (nothing ever read it) --
        // `reconcile_after_lag` is what a `RecvError::Lagged` on the
        // real channel drives the bridge to do instead.
        arclain_ui::core::operation_bridge::reconcile_after_lag(
            &shared,
            &shared.operation_origins,
            &runtime,
            &app,
            1,
        )
        .await;

        assert!(
            tab.pending_open_operation.get().is_none(),
            "a lagged terminal event must still be caught up on, not leave the tab believing the \
             operation is still running forever"
        );
    });
}
