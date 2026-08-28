//! On-demand, real end-to-end smoke test for the open-file-from-archive
//! pipeline this fix round rewired: a real facade bootstrap, a real
//! archive fixture, the real `start_archive_open` -> `open_file_from_
//! archive` call chain, a real OS-level file open, and real shutdown-
//! driven cleanup.
//!
//! `#[ignore]`d rather than part of the routine `cargo test -p
//! arclain_ui` run: unlike every other test in this crate, this one
//! launches a real OS process on the machine running it (on Windows,
//! `explorer.exe <path>`, which opens `readme.txt` in whatever
//! application is registered as the default `.txt` handler -- almost
//! always Notepad). That is exactly the point (it proves the wiring a
//! headless facade call alone can't), but it is not something every
//! future `cargo test` run should do unattended. Run explicitly with:
//!
//!   cargo test -p arclain_ui --test open_file_from_archive_smoke_test -- --ignored --nocapture
//!
//! and expect a real window to open briefly -- close it manually if it
//! does not close on its own.
//!
//! Deliberately does NOT ask the OS to launch the fixture's `game.exe`/
//! `d3d9.dll` (fabricated, non-functional binaries): doing so risks an
//! unattended, non-dismissible "this app can't run on your PC" dialog,
//! and there is no window-automation tool available in this environment
//! to observe or close it. Their presence alongside `readme.txt` under
//! the whole-archive root fallback is instead proven by `arclain_app`'s
//! own `empty_entry_ids_materializes_the_whole_archive_so_a_root_level_
//! exes_sibling_dll_comes_along` test (`crates/app/tests/
//! materialization_leases.rs`), which checks the same content guarantee
//! without ever executing the fabricated binary.

mod common;

use std::path::Path;
use std::time::Duration;

use arclain_app::event::{OperationEvent, OperationKind, OperationResult, OperationState};
use arclain_app::materialization::MaterializationLease;
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::core::app_lifecycle::shutdown_facade_on_exit;
use arclain_ui::core::operation_bridge;
use arclain_ui::features::archive_operations::open_file_from_archive;
use arclain_ui::shared::SharedState;

fn temp_paths(root: &Path) -> AppPaths {
    AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        log_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
    }
}

fn bootstrap_real_facade(temp: &tempfile::TempDir) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(temp_paths(temp.path())),
        ..Default::default()
    })
    .expect("bootstrap a real ArclainApp for the open-file-from-archive smoke test")
}

/// Builds a real ZIP fixture with a root-level layout matching the I3
/// regression this fix round covers (an executable and its DLL sitting
/// directly at the archive root, alongside a self-contained text file).
fn build_zip_fixture(path: &Path) {
    let file = std::fs::File::create(path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, content) in [
        (
            "readme.txt",
            b"hello from the arclain open-file-from-archive smoke test".as_slice(),
        ),
        (
            "game.exe",
            b"not a real executable, only its path/name matter here".as_slice(),
        ),
        (
            "d3d9.dll",
            b"not a real dll, only its path/name matter here".as_slice(),
        ),
    ] {
        writer
            .start_file(name, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry content");
    }
    writer.finish().expect("finish zip fixture");
}

fn poll_until<T>(timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(value) = probe() {
            return Some(value);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drains `receiver` (a fresh runtime just for this bounded wait -- the
/// test's own top-level thread has no ambient runtime of its own) until
/// a `Materialize` operation's `Completed` event arrives, or `timeout`
/// elapses.
fn recv_materialized_lease(
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    timeout: Duration,
) -> Option<MaterializationLease> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let Ok(Ok(event)) = tokio::time::timeout(remaining, receiver.recv()).await else {
                return None;
            };
            if event.kind == OperationKind::Materialize {
                if let OperationState::Completed {
                    result: OperationResult::Materialized { lease },
                } = event.state
                {
                    return Some(lease);
                }
            }
        }
    })
}

#[test]
#[ignore = "launches a real OS process (explorer.exe / your default .txt handler) -- run explicitly, see module doc comment"]
fn open_file_from_archive_end_to_end_through_a_real_os_open_and_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    let facade = bootstrap_real_facade(&temp);

    let shared = SharedState {
        facade: Some(facade.clone()),
        ..common::create_test_shared_state()
    };
    // Mirrors `SharedState::new()`'s real startup: without this, nothing
    // ever turns `start_archive_open`'s or `open_file_from_archive`'s
    // fire-and-forget operations into populated tab signals / tracked
    // external-open leases.
    operation_bridge::spawn(&shared);

    let fixture_path = temp.path().join("fixture.zip");
    build_zip_fixture(&fixture_path);

    // --- Step 1: open the archive for real, through the exact function
    // every real call site (file dialog, hotkey, drop, tab restore) uses.
    let tab_id = shared.signals().tabs.get().active_id();
    arclain_ui::core::operations::archive::start_archive_open(
        &shared,
        tab_id,
        fixture_path.clone(),
        None,
    );

    let opened_session = poll_until(Duration::from_secs(10), || {
        shared
            .signals()
            .tabs
            .get()
            .active()
            .archive_session_id
            .get()
    })
    .expect("archive open did not complete within 10s");
    println!("[smoke] archive opened, session id = {opened_session:?}");

    // --- Step 2: drive the real open-file-from-archive path on a safe
    // target -- see the module doc comment for why `readme.txt`
    // (self-contained / "FileOnly") rather than the fixture's exe/dll.
    let mut operations = facade.subscribe_operations();
    open_file_from_archive(&shared, "readme.txt");

    let lease = recv_materialized_lease(&mut operations, Duration::from_secs(10))
        .expect("materialize for the external open did not complete within 10s");
    println!(
        "[smoke] materialized lease {:?} at {}",
        lease.id,
        lease.local_path.display()
    );
    assert!(
        lease.local_path.exists(),
        "the materialized lease's local_path must actually exist on disk"
    );
    assert_eq!(
        std::fs::read(&lease.local_path).unwrap(),
        b"hello from the arclain open-file-from-archive smoke test"
    );
    // FileOnly must not have pulled `game.exe`/`d3d9.dll` in alongside
    // it -- proves the narrow case doesn't over-materialize.
    let siblings: Vec<_> = std::fs::read_dir(lease.local_path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        siblings.len(),
        1,
        "FileOnly must materialize only the target file, got {siblings:?}"
    );

    // --- Step 3: confirm the OS-level open itself actually happened --
    // `handle_materialize_completed` only tracks the lease for renewal
    // once `open_extracted_file_via_signals`'s `explorer.exe` spawn
    // succeeds; poll the status bar signal it writes on success.
    let opened_message = poll_until(Duration::from_secs(5), || {
        let message = shared.signals().status_bar.get().message.clone();
        if message.starts_with("Opened:") {
            Some(message)
        } else {
            None
        }
    })
    .expect("status bar never reported the file as opened");
    println!("[smoke] {opened_message}");
    println!(
        "[smoke] a real window (Notepad or your OS's default .txt handler) should now be open \
         for readme.txt -- close it manually if it does not close on its own."
    );

    // --- Step 4: shut down exactly the way `on_exit` does in the real
    // app, and confirm the lease directory this session created is
    // actually gone afterward.
    shutdown_facade_on_exit(&shared);
    assert!(
        !lease.local_path.exists(),
        "shutdown must reclaim the external-open lease's directory"
    );
    println!("[smoke] shutdown reclaimed the lease directory -- end to end, all green.");
}
