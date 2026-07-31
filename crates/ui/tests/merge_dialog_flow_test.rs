//! End-to-end coverage for the UI-side split-archive merge flow: what a
//! dropped multi-part archive does to the merge dialog, and what
//! `crate::core::operations::merge::start_merge` plus
//! `crate::core::operation_bridge` do with the resulting operation.
//!
//! Driven through a real bootstrapped `ArclainApp` (the same way
//! `archive_mutation_ui_test.rs` drives mutations), because the merge
//! has no injectable runner seam: `arclain_core::services::MergeService`
//! reaches for a real 7-Zip twice and consults no application setting on
//! the way. The merge tests here are therefore gated on one being
//! installed and skip with a message otherwise; the dialog-state tests
//! need nothing external.
//!
//! Fixture names use placeholder product codes (`RJ123456`), never real
//! catalogue entries.

mod common;
use common::create_test_shared_state;

use std::path::{Path, PathBuf};
use std::time::Duration;

use arclain_app::archive::detect_multipart;
use arclain_app::operations::{MergeCompressionLevel, MergeOutputFormat};
use arclain_ui::shared::dialogs::MergeDialogState;

fn wait_until(message: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if condition() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn real_bootstrap_config(temp: &Path) -> arclain_app::BootstrapConfig {
    arclain_app::BootstrapConfig {
        paths_override: Some(arclain_app::AppPaths {
            config_dir: temp.join("config"),
            data_dir: temp.join("data"),
            cache_dir: temp.join("cache"),
            log_dir: temp.join("logs"),
            plugins_dir: temp.join("plugins"),
        }),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    }
}

fn bootstrap_real_app(temp: &Path) -> arclain_app::ArclainApp {
    arclain_app::ArclainApp::bootstrap(real_bootstrap_config(temp))
        .expect("bootstrap must succeed against a bare temp-dir AppPaths")
}

fn detect_real_sevenzip() -> Option<PathBuf> {
    let temp = tempfile::tempdir().ok()?;
    let app = arclain_app::ArclainApp::bootstrap(real_bootstrap_config(temp.path())).ok()?;
    let runtime = tokio::runtime::Runtime::new().ok()?;
    runtime
        .block_on(app.capabilities())
        .ok()?
        .external_tools
        .into_iter()
        .find(|tool| tool.tool == "7z" && tool.available)
        .and_then(|tool| tool.resolved_path)
}

/// A scratch root on a normal, persistent filesystem rather than system
/// temp -- see `crates/app/tests/merge_operation.rs`'s identical note on
/// why a real 7-Zip child process and a RAM disk have raced before.
fn scratch_dir(prefix: &str) -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    std::fs::create_dir_all(&root).expect("create test scratch root");
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(&root)
        .expect("create scratch tempdir")
}

/// Builds a real three-volume 7-Zip set under `root/sets`, returning that
/// directory.
fn build_split_set(sevenzip: &Path, root: &Path, base_name: &str) -> PathBuf {
    let source = root.join("source");
    std::fs::create_dir_all(&source).expect("create fixture source dir");
    for index in 1..=3u8 {
        std::fs::write(
            source.join(format!("part-{index}.bin")),
            vec![b'a' + index; 120_000],
        )
        .expect("write fixture source file");
    }

    let sets = root.join("sets");
    std::fs::create_dir_all(&sets).expect("create fixture sets dir");
    let status = std::process::Command::new(sevenzip)
        .arg("a")
        .arg("-t7z")
        .arg("-mx=0")
        .arg("-v128k")
        .arg("-bb0")
        .arg("-y")
        .arg(sets.join(format!("{base_name}.7z")))
        .arg(format!(
            "{}{}*",
            source.display(),
            std::path::MAIN_SEPARATOR
        ))
        .arg("-r")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run 7-Zip to build the fixture set");
    assert!(status.success(), "building the fixture split set failed");

    std::fs::remove_dir_all(&source).expect("remove fixture source dir");
    sets
}

/// The drop/file-picker branch: a member of a real set becomes dialog
/// state whose part count is the one detection actually found. (The
/// pre-facade dialog held core's bare `detect` result, whose part list is
/// always empty, so this row always read `0`.)
#[test]
fn a_detected_set_populates_the_dialog_with_its_real_part_count() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping a_detected_set_populates_the_dialog_with_its_real_part_count: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-ui-detect-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456");

    // Entering from a middle member, the way a drop usually does.
    let detected = detect_multipart(&sets.join("rj123456.7z.002"))
        .expect("a member of a real split set must be detected");

    let mut dialog = MergeDialogState::default();
    dialog.open(detected);

    assert!(dialog.show);
    let multipart = dialog.multipart.as_ref().expect("the dialog holds the set");
    assert_eq!(multipart.base_name, "rj123456");
    assert_eq!(multipart.parts.len(), 3);
    assert_eq!(multipart.first_part, sets.join("rj123456.7z.001"));
    assert_eq!(
        dialog.preview_output_name().as_deref(),
        Some("rj123456.7z"),
        "the dialog previews the file the merge will actually write"
    );
}

/// An ordinary single-file archive must not open the merge dialog.
#[test]
fn an_ordinary_archive_is_not_detected_as_a_split_set() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let archive = temp.path().join("rj123456.zip");
    std::fs::write(&archive, b"not really a zip, only the name matters here")
        .expect("write fixture");
    assert!(
        detect_multipart(&archive).is_none(),
        "a lone .zip with no .z01 sibling must not trigger the merge dialog"
    );
}

/// The whole dialog flow: dialog state in, `start_merge` dispatched, the
/// tab's progress dialog opened, the bridge carrying progress and
/// completion back onto the tab's signals, and a real merged archive at
/// the end.
#[test]
fn starting_a_merge_drives_the_progress_dialog_and_reports_completion() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping starting_a_merge_drives_the_progress_dialog_and_reports_completion: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-ui-flow-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456");
    let output_path = sets.join("rj123456.7z");

    let app = bootstrap_real_app(&temp.path().join("profile"));
    let mut shared = create_test_shared_state();
    shared.facade = Some(app);
    // The bridge is what turns the operation's events back into dialog
    // and status-bar state -- `create_test_shared_state` deliberately
    // does not start it (see `archive_mutation_ui_test.rs`'s own note).
    arclain_ui::core::operation_bridge::spawn(&shared);

    let tab = shared.signals().tabs.get().active().clone();

    let mut dialog = MergeDialogState::default();
    dialog.open(detect_multipart(&sets.join("rj123456.7z.001")).expect("the set is detected"));
    assert_eq!(dialog.output_format, MergeOutputFormat::SevenZip);
    assert_eq!(dialog.compression_level, MergeCompressionLevel::Normal);

    arclain_ui::core::operations::merge::start_merge(
        &shared,
        &tab,
        dialog
            .multipart
            .clone()
            .expect("the dialog holds the detected set"),
        dialog.output_format,
        dialog.compression_level,
        dialog.delete_originals,
    );
    dialog.close();

    // The progress dialog opens synchronously, before the dispatch even
    // reaches the facade, so the user never sees a dead frame.
    let opened = tab.extraction_dialog().get();
    assert!(opened.show);
    assert_eq!(opened.title, "Merging Archive");
    assert_eq!(opened.file_action, "Merging 3 parts...");
    assert!(
        opened.can_cancel,
        "a merge is a registered operation now, so its dialog can cancel it"
    );
    assert!(!opened.can_pause && !opened.can_minimize);

    wait_until(
        "start_merge never registered an operation on the tab",
        || tab.active_extraction_operation.get().is_some(),
    );

    wait_until(
        "the merge never reported completion through the bridge",
        || {
            shared
                .signals()
                .status_bar
                .get()
                .message
                .starts_with("Merge complete:")
        },
    );

    assert_eq!(
        shared.signals().status_bar.get().message,
        "Merge complete: rj123456.7z"
    );
    let finished = tab.extraction_dialog().get();
    assert!(
        !finished.show,
        "a finished merge must close the progress dialog"
    );
    assert_eq!(
        finished.status,
        arclain_ui::shared::dialogs::ExtractionStatus::Completed
    );
    assert!(
        tab.active_extraction_operation.get().is_none(),
        "a finished merge must release the tab's operation slot"
    );
    assert!(
        output_path.exists(),
        "the merged archive must exist on disk"
    );
    for index in 1..=3 {
        assert!(
            sets.join(format!("rj123456.7z.{index:03}")).exists(),
            "the dialog did not ask to delete originals"
        );
    }
}

/// The merge and an extraction share one per-tab progress dialog, so the
/// second one to start must be refused rather than overwrite the first's
/// progress -- something the pre-facade merge (which tracked no operation
/// at all) could not do.
#[test]
fn a_merge_is_refused_while_the_tabs_operation_slot_is_occupied() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let sets = temp.path().join("sets");
    std::fs::create_dir_all(&sets).expect("create sets dir");
    for index in 1..=2 {
        std::fs::write(sets.join(format!("rj123456.part{index}.rar")), b"")
            .expect("write fixture part");
    }

    let app = bootstrap_real_app(temp.path());
    let mut shared = create_test_shared_state();
    shared.facade = Some(app);
    let tab = shared.signals().tabs.get().active().clone();
    // Stand in for an extraction (or an earlier merge) already running.
    tab.active_extraction_operation
        .set(Some(arclain_app::ids::OperationId::from_raw(4242)));

    let detected =
        detect_multipart(&sets.join("rj123456.part1.rar")).expect("the naming pattern matches");
    arclain_ui::core::operations::merge::start_merge(
        &shared,
        &tab,
        detected,
        MergeOutputFormat::SevenZip,
        MergeCompressionLevel::Normal,
        false,
    );

    assert_eq!(
        shared.signals().status_bar.get().message,
        "Another archive operation is already running"
    );
    assert!(
        !tab.extraction_dialog().get().show,
        "a refused merge must not open the progress dialog over the running operation's"
    );
    assert_eq!(
        tab.active_extraction_operation.get(),
        Some(arclain_app::ids::OperationId::from_raw(4242)),
        "a refused merge must leave the running operation's slot alone"
    );
}
