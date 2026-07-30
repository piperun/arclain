//! Integration tests for merging a split multi-part archive as an
//! application operation: `ArclainApp::start_merge`'s lifecycle
//! (`Accepted` -> `Started` -> `Progress`* -> optional `Challenge` ->
//! exactly one terminal state), driven through the public facade the way
//! a real frontend does.
//!
//! Two groups of tests live here:
//!
//! - **Pre-flight tests** need no external tool at all. Everything they
//!   assert (a structurally invalid request, a set that vanished, a set
//!   whose identity changed, an incomplete set, an occupied output path)
//!   is decided before the operation touches 7-Zip.
//! - **Real-merge tests** drive an actual 7-Zip through
//!   `arclain_core::services::MergeService`, because that is the only
//!   seam a merge has: unlike extraction (`ExtractRunner`) there is no
//!   injectable runner to fake, so a genuine round-trip, a genuine
//!   password challenge, and a genuine cancellation are only observable
//!   against a real tool. Each is gated on one being installed and skips
//!   with a message otherwise, exactly as
//!   `processing_operations.rs`'s own real-7z tests do.
//!
//! Every test is a plain (synchronous) `#[test]`: dropping `ArclainApp`
//! from inside an async context panics, so each builds `app` in sync
//! code, awaits facade calls through one `runtime.block_on` that only
//! borrows it, and lets it drop after `block_on` returns -- the same
//! shape `archive_mutation.rs`/`extract_operation.rs` use.
//!
//! Fixture names use placeholder product codes (`RJ123456`), never real
//! catalogue entries.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use arclain_app::archive::{detect_multipart, MultiPartArchiveDto, MultiPartFormat};
use arclain_app::challenge::{Challenge, ChallengeResponse, SecretInput};
use arclain_app::error::{ApplicationErrorKind, SuggestedAction};
use arclain_app::event::{OperationEvent, OperationKind, OperationResult, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::operations::{MergeCompressionLevel, MergeOutputFormat, MergeRequest};
use arclain_app::{ArclainApp, BootstrapConfig};
use arclain_core::ArchiveBackend;

const TEST_PASSWORD: &str = "correct-horse-battery-staple";

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn bootstrap_app(paths: arclain_app::AppPaths) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed")
}

/// An app whose 7-Zip path resolves to a dummy file: enough for
/// `bootstrap` to be deterministic on any machine, and never invoked by
/// the pre-flight tests (which fail before any tool runs).
fn bootstrap_without_a_real_tool(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    let sevenzip = support::create_dummy_executable(&temp.path().join("bin"), "7z.exe");
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    bootstrap_app(paths)
}

fn request(archive: MultiPartArchiveDto) -> MergeRequest {
    MergeRequest {
        archive,
        output_format: MergeOutputFormat::SevenZip,
        compression_level: MergeCompressionLevel::Store,
        output_path: None,
        delete_originals: false,
        password: None,
    }
}

/// Waits for `operation_id` to reach a terminal state, returning it.
async fn wait_for_terminal(app: &ArclainApp, operation_id: OperationId) -> OperationState {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let snapshot = app
            .operation(operation_id)
            .await
            .expect("operation must exist");
        if matches!(
            snapshot.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        ) {
            return snapshot.state;
        }
        if std::time::Instant::now() >= deadline {
            panic!("merge did not reach a terminal state within the test deadline");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Drains `receiver` until `operation_id` reaches a terminal state,
/// returning every event that operation produced, in order.
async fn collect_until_terminal(
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    operation_id: OperationId,
) -> Vec<OperationEvent> {
    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "merge did not reach a terminal state within the test deadline; saw {events:?}"
        );
        let event = match tokio::time::timeout(remaining, receiver.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                panic!("the operation event stream closed before a terminal state")
            }
            Err(_) => panic!("merge did not reach a terminal state within the test deadline"),
        };
        if event.operation_id != operation_id {
            continue;
        }
        let terminal = matches!(
            event.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        );
        events.push(event);
        if terminal {
            return events;
        }
    }
}

/// Awaits `operation_id`'s first `Challenge::Password`, returning the
/// challenge and every event seen up to and including it.
async fn wait_for_password_challenge(
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    operation_id: OperationId,
) -> (arclain_app::ids::ChallengeId, Vec<OperationEvent>) {
    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "no password challenge arrived within the test deadline; saw {events:?}"
        );
        let event = match tokio::time::timeout(remaining, receiver.recv()).await {
            Ok(Ok(event)) => event,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                panic!("the operation event stream closed before a password challenge")
            }
            Err(_) => panic!("no password challenge arrived within the test deadline"),
        };
        if event.operation_id != operation_id {
            continue;
        }
        let challenge_id = match &event.state {
            OperationState::Challenge {
                challenge: Challenge::Password { id, .. },
            } => Some(*id),
            OperationState::Completed { .. }
            | OperationState::Cancelled
            | OperationState::Failed { .. } => panic!(
                "the merge reached a terminal state instead of raising a password challenge: \
                 {:?}",
                event.state
            ),
            _ => None,
        };
        events.push(event);
        if let Some(challenge_id) = challenge_id {
            return (challenge_id, events);
        }
    }
}

// ========================= pre-flight (no tool) =========================

/// Placeholder-named parts, created empty: the pre-flight checks under
/// test never read their contents.
fn touch_parts(dir: &Path, names: &[&str]) {
    std::fs::create_dir_all(dir).expect("create fixture dir");
    for name in names {
        std::fs::write(dir.join(name), b"").expect("write fixture part");
    }
}

#[test]
fn a_structurally_invalid_request_is_rejected_without_registering_an_operation() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let app = bootstrap_without_a_real_tool(&temp);
    let runtime = foreign_runtime();

    runtime.block_on(async {
        let before = app
            .recent_operations(50)
            .await
            .expect("recent_operations must succeed");

        let mut archive = MultiPartArchiveDto {
            first_part: temp.path().join("rj123456.part1.rar"),
            base_name: String::new(),
            format: MultiPartFormat::RarPart,
            parts: Vec::new(),
        };
        archive.base_name = "   ".to_string();

        let error = app
            .start_merge(request(archive))
            .await
            .expect_err("a blank base name must be refused");
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("archive.base_name"));

        let after = app
            .recent_operations(50)
            .await
            .expect("recent_operations must succeed");
        assert_eq!(
            after.len(),
            before.len(),
            "a request refused at validation must leave no phantom operation behind"
        );
    });
}

#[test]
fn a_set_that_no_longer_exists_fails_as_not_found() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let app = bootstrap_without_a_real_tool(&temp);
    let runtime = foreign_runtime();
    let sets = temp.path().join("sets");
    std::fs::create_dir_all(&sets).expect("create sets dir");

    runtime.block_on(async {
        // A `.rar` sequence only counts as a set while its `.r00` sibling
        // exists -- the exact "the user deleted a part while the merge
        // dialog was open" case.
        let archive = MultiPartArchiveDto {
            first_part: sets.join("rj123456.rar"),
            base_name: "rj123456".to_string(),
            format: MultiPartFormat::RarSequence,
            parts: vec![sets.join("rj123456.rar")],
        };
        let operation_id = app
            .start_merge(request(archive))
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound);
            }
            other => panic!("expected a NotFound failure, got {other:?}"),
        }
    });
}

#[test]
fn a_set_whose_identity_changed_fails_as_a_conflict() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let app = bootstrap_without_a_real_tool(&temp);
    let runtime = foreign_runtime();
    let sets = temp.path().join("sets");
    touch_parts(&sets, &["rj123456.part1.rar", "rj123456.part2.rar"]);

    runtime.block_on(async {
        // The set on disk is `rj123456`; the request claims the same
        // first part belongs to a set named `rj999999`. Re-detection
        // disagrees, so the merge refuses rather than merging the set it
        // found under a name the caller did not approve.
        let archive = MultiPartArchiveDto {
            first_part: sets.join("rj123456.part1.rar"),
            base_name: "rj999999".to_string(),
            format: MultiPartFormat::RarPart,
            parts: vec![sets.join("rj123456.part1.rar")],
        };
        let operation_id = app
            .start_merge(request(archive))
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Conflict);
                assert!(error.retryable);
            }
            other => panic!("expected a Conflict failure, got {other:?}"),
        }
    });
}

#[test]
fn a_set_missing_its_first_part_fails_as_not_found() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let app = bootstrap_without_a_real_tool(&temp);
    let runtime = foreign_runtime();
    let sets = temp.path().join("sets");
    // Part 1 deliberately absent: the naming convention still matches, so
    // detection succeeds, but enumeration starts at part 1 and finds
    // nothing.
    touch_parts(&sets, &["rj123456.part2.rar", "rj123456.part3.rar"]);

    runtime.block_on(async {
        let archive = detect_multipart(&sets.join("rj123456.part2.rar"))
            .expect("the naming convention still matches");
        assert!(archive.parts.is_empty());

        let operation_id = app
            .start_merge(request(archive))
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound);
            }
            other => panic!("expected a NotFound failure, got {other:?}"),
        }
    });
}

#[test]
fn an_occupied_output_path_is_a_conflict_and_the_existing_file_is_untouched() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let app = bootstrap_without_a_real_tool(&temp);
    let runtime = foreign_runtime();
    let sets = temp.path().join("sets");
    touch_parts(&sets, &["rj123456.part1.rar", "rj123456.part2.rar"]);
    let output_path = sets.join("rj123456.7z");
    std::fs::write(
        &output_path,
        b"a pre-existing file the merge must not replace",
    )
    .expect("seed the occupied output path");

    runtime.block_on(async {
        let archive =
            detect_multipart(&sets.join("rj123456.part1.rar")).expect("the set is detected");
        let operation_id = app
            .start_merge(request(archive))
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Conflict);
                assert_eq!(
                    error.suggested_action,
                    Some(SuggestedAction::ChooseDestination)
                );
                assert_eq!(error.path.as_deref(), Some(output_path.as_path()));
            }
            other => panic!("expected a Conflict failure, got {other:?}"),
        }
    });

    assert_eq!(
        std::fs::read(&output_path).unwrap(),
        b"a pre-existing file the merge must not replace",
        "a refused merge must never touch the file already at its output path"
    );
}

/// The destructive path's failure invariant on the **pre-flight** branches
/// (a request refused before any tool runs): `delete_originals: true` must
/// not make them delete anything. Uses the occupied-output refusal because
/// it needs no external tool, so this guard runs on every machine.
///
/// Its sibling `a_merge_that_fails_after_running_with_delete_originals_
/// keeps_every_part` covers the *post-attempt* failure arm, which this one
/// cannot reach — `run_merge` returns from the pre-flight checks before
/// the attempt loop, so a regression in the failure arm would slip past
/// this test alone.
#[test]
fn a_merge_refused_before_running_with_delete_originals_keeps_every_part() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let app = bootstrap_without_a_real_tool(&temp);
    let runtime = foreign_runtime();
    let sets = temp.path().join("sets");
    let part_names = ["rj123456.part1.rar", "rj123456.part2.rar"];
    touch_parts(&sets, &part_names);
    let output_path = sets.join("rj123456.7z");
    std::fs::write(&output_path, b"occupied").expect("seed the occupied output path");

    runtime.block_on(async {
        let archive =
            detect_multipart(&sets.join("rj123456.part1.rar")).expect("the set is detected");
        assert_eq!(archive.parts.len(), 2);
        let mut merge = request(archive);
        merge.delete_originals = true;

        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Conflict);
            }
            other => panic!("expected a Conflict failure, got {other:?}"),
        }
    });

    for name in part_names {
        assert!(
            sets.join(name).exists(),
            "a merge that failed must leave {name} in place even with delete_originals set"
        );
    }
}

// ========================== real 7-Zip merges ==========================

/// Locates a real 7-Zip CLI, if any. `MergeService` reaches for one twice
/// (once through `BackendSelector` for extraction, once directly for
/// compression) and neither consults application settings, so a merge is
/// only exercisable end to end on a machine that actually has the tool.
fn detect_real_sevenzip() -> Option<PathBuf> {
    let cli = arclain_core::backends::sevenz_cli::SevenZipCli::detect(None).ok()?;
    let exe = cli.exe_path().to_path_buf();
    exe.exists().then_some(exe)
}

/// A scratch root on a normal, persistent filesystem rather than the
/// system temp directory: on a machine where temp resolves to a RAM disk,
/// a real 7-Zip child process's writes there have raced test assertions
/// before (see `processing_operations.rs`'s identical note).
fn scratch_dir(prefix: &str) -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    std::fs::create_dir_all(&root).expect("create test scratch root");
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(&root)
        .expect("create scratch tempdir")
}

/// Runs 7-Zip to build a volumed archive of everything under `source`,
/// writing `<base_name>.7z.NNN` into `root/sets` and returning that
/// directory. `source` is removed afterwards so nothing but the set is
/// left for the assertions to trip over.
fn pack_split_set(
    sevenzip: &Path,
    root: &Path,
    base_name: &str,
    source: &Path,
    password: Option<&str>,
) -> PathBuf {
    let sets = root.join("sets");
    std::fs::create_dir_all(&sets).expect("create fixture sets dir");

    let mut command = std::process::Command::new(sevenzip);
    command
        .arg("a")
        .arg("-t7z")
        .arg("-mx=0")
        .arg("-v128k")
        .arg("-bb0")
        .arg("-y");
    if let Some(password) = password {
        command.arg(format!("-p{password}"));
        command.arg("-mhe=on");
    }
    command
        .arg(sets.join(format!("{base_name}.7z")))
        .arg(format!(
            "{}{}*",
            source.display(),
            std::path::MAIN_SEPARATOR
        ))
        .arg("-r")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = command
        .status()
        .expect("run 7-Zip to build the fixture set");
    assert!(status.success(), "building the fixture split set failed");

    std::fs::remove_dir_all(source).expect("remove fixture source dir");
    sets
}

/// Builds a real multi-part 7-Zip set from three small files, returning
/// the directory holding it. `password` encrypts both contents and
/// headers, so extracting without one fails the way a real encrypted set
/// does.
fn build_split_set(
    sevenzip: &Path,
    root: &Path,
    base_name: &str,
    password: Option<&str>,
) -> PathBuf {
    let source = root.join("source");
    std::fs::create_dir_all(&source).expect("create fixture source dir");
    for index in 1..=3u8 {
        // Stored uncompressed (`-mx=0`), so each file's byte count is
        // exactly its volume footprint and three 128 KiB volumes really
        // result. Distinct fill bytes keep the three files distinguishable
        // if a round-trip ever swaps content between them.
        let bytes = vec![b'a' + index; 120_000];
        std::fs::write(source.join(format!("part-{index}.bin")), &bytes)
            .expect("write fixture source file");
    }

    pack_split_set(sevenzip, root, base_name, &source, password)
}

/// Builds a real split set whose only member is an empty directory. Its
/// extraction succeeds and yields no *files*, which is how
/// `MergeService::merge` is made to fail on its own ("No files were
/// extracted from the archive") without an exit code in the message —
/// i.e. reaching the post-attempt failure arm rather than being
/// misclassified as a password problem.
fn build_fileless_split_set(sevenzip: &Path, root: &Path, base_name: &str) -> PathBuf {
    let source = root.join("source");
    std::fs::create_dir_all(source.join("empty")).expect("create fixture source dir");
    pack_split_set(sevenzip, root, base_name, &source, None)
}

/// Every entry path in `archive`, sorted -- what a round-trip must
/// preserve.
fn entry_paths(archive: &Path, password: Option<&str>) -> Vec<String> {
    let cli = arclain_core::backends::sevenz_cli::SevenZipCli::detect(None)
        .expect("a real 7-Zip was already detected");
    let info = cli.list(archive, password).expect("list the archive");
    let mut paths: Vec<String> = info
        .entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.path.replace('\\', "/"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn a_real_split_set_merges_into_one_archive_with_the_same_contents() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping a_real_split_set_merges_into_one_archive_with_the_same_contents: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-roundtrip-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", None);
    let first_part = sets.join("rj123456.7z.001");
    let expected_entries = entry_paths(&first_part, None);
    assert_eq!(expected_entries.len(), 3);

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    let output_path = runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let archive = detect_multipart(&first_part).expect("the set is detected");
        assert_eq!(archive.parts.len(), 3, "the fixture must really be split");

        let operation_id = app
            .start_merge(request(archive))
            .await
            .expect("the request is structurally valid");
        let events = collect_until_terminal(&mut receiver, operation_id).await;

        assert!(
            events
                .iter()
                .all(|event| event.kind == OperationKind::Merge),
            "every event must be reported as a Merge"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.state, OperationState::Started)),
            "the merge must report Started: {events:?}"
        );
        let progress: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.state {
                OperationState::Progress {
                    completed_units,
                    total_units,
                    message,
                } => Some((*completed_units, *total_units, message.clone())),
                _ => None,
            })
            .collect();
        assert!(
            progress.len() >= 2,
            "a real merge must report live progress, not a single frozen frame: {events:?}"
        );
        assert!(
            progress
                .iter()
                .all(|(completed, total, _)| *completed <= 100 && *total == Some(100)),
            "merge progress is a percent out of 100: {progress:?}"
        );
        assert!(
            progress.iter().any(|(completed, _, _)| *completed == 100),
            "a completed merge must report 100%: {progress:?}"
        );

        match events.last().map(|event| &event.state) {
            Some(OperationState::Completed {
                result: OperationResult::Merged { output_path },
            }) => output_path.clone(),
            other => panic!("expected Completed with a merged output path, got {other:?}"),
        }
    });

    assert_eq!(
        output_path,
        sets.join("rj123456.7z"),
        "the default output sits beside the set's first part, named after it"
    );
    assert!(output_path.exists(), "the merged archive must exist");
    assert_eq!(
        entry_paths(&output_path, None),
        expected_entries,
        "the merged archive must hold exactly the split set's own entries"
    );
    for index in 1..=3 {
        assert!(
            sets.join(format!("rj123456.7z.{index:03}")).exists(),
            "delete_originals was not requested, so every part must survive"
        );
    }
}

#[test]
fn delete_originals_removes_the_enumerated_parts_but_never_a_caller_supplied_path() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping delete_originals_removes_the_enumerated_parts_but_never_a_caller_supplied_path: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-delete-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", None);
    let first_part = sets.join("rj123456.7z.001");

    // A file that is emphatically not part of the set. A request whose
    // `parts` list named it must not be able to get it deleted: the merge
    // always re-enumerates from disk, so `delete_originals` can only ever
    // remove real members.
    let bystander = temp.path().join("not-a-part.bin");
    std::fs::write(&bystander, b"this file belongs to somebody else").expect("write bystander");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    runtime.block_on(async {
        let mut archive = detect_multipart(&first_part).expect("the set is detected");
        // Forge the informational part list: identity fields stay honest
        // (otherwise the request is refused as a Conflict before any of
        // this matters), only `parts` lies.
        archive.parts = vec![bystander.clone()];

        let mut merge = request(archive);
        merge.delete_originals = true;
        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Completed {
                result: OperationResult::Merged { .. },
            } => {}
            other => panic!("expected a completed merge, got {other:?}"),
        }
    });

    assert!(
        bystander.exists(),
        "a caller-supplied part list must never be able to direct file deletion"
    );
    assert_eq!(
        std::fs::read(&bystander).unwrap(),
        b"this file belongs to somebody else"
    );
    for index in 1..=3 {
        assert!(
            !sets.join(format!("rj123456.7z.{index:03}")).exists(),
            "delete_originals must remove every part the merge enumerated"
        );
    }
    assert!(sets.join("rj123456.7z").exists());
}

#[test]
fn an_encrypted_set_raises_a_password_challenge_and_completes_once_answered() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping an_encrypted_set_raises_a_password_challenge_and_completes_once_answered: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-challenge-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", Some(TEST_PASSWORD));
    let first_part = sets.join("rj123456.7z.001");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let operation_id = app
            .start_merge(request(archive))
            .await
            .expect("the request is structurally valid");

        let (challenge_id, before) = wait_for_password_challenge(&mut receiver, operation_id).await;
        match before.last().map(|event| &event.state) {
            Some(OperationState::Challenge {
                challenge:
                    Challenge::Password {
                        archive_name,
                        attempt,
                        ..
                    },
            }) => {
                assert_eq!(archive_name, "rj123456.7z.001");
                assert_eq!(*attempt, 1);
            }
            other => panic!("expected a password challenge, got {other:?}"),
        }

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::Password {
                id: challenge_id,
                value: SecretInput::new(TEST_PASSWORD.to_string()),
            },
        )
        .await
        .expect("answering the merge's own pending challenge must be accepted");

        let after = collect_until_terminal(&mut receiver, operation_id).await;
        let all: Vec<&OperationEvent> = before.iter().chain(after.iter()).collect();
        assert!(
            !format!("{all:?}").contains(TEST_PASSWORD),
            "no event may ever carry the supplied password"
        );
        match after.last().map(|event| &event.state) {
            Some(OperationState::Completed {
                result: OperationResult::Merged { .. },
            }) => {}
            other => panic!("expected the answered merge to complete, got {other:?}"),
        }
    });

    assert!(sets.join("rj123456.7z").exists());
}

/// A *wrong* seeded password is the only path where a real secret ever
/// reaches the failing 7-Zip invocation, so it is the only path that
/// could leak one back out through an error diagnostic. It must not: the
/// wrong password is rejected into a fresh challenge, and nothing on the
/// event stream carries either secret.
#[test]
fn a_wrong_seeded_password_reprompts_without_leaking_either_secret() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping a_wrong_seeded_password_reprompts_without_leaking_either_secret: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    const WRONG_PASSWORD: &str = "definitely-not-the-right-one";

    let temp = scratch_dir("merge-wrongpass-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", Some(TEST_PASSWORD));
    let first_part = sets.join("rj123456.7z.001");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let mut merge = request(archive);
        merge.password = Some(SecretInput::new(WRONG_PASSWORD.to_string()));

        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");

        let (challenge_id, before) = wait_for_password_challenge(&mut receiver, operation_id).await;
        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::Password {
                id: challenge_id,
                value: SecretInput::new(TEST_PASSWORD.to_string()),
            },
        )
        .await
        .expect("answering the merge's own pending challenge must be accepted");

        let after = collect_until_terminal(&mut receiver, operation_id).await;
        let rendered = format!(
            "{:?}",
            before.iter().chain(after.iter()).collect::<Vec<_>>()
        );
        assert!(
            !rendered.contains(WRONG_PASSWORD),
            "a rejected password must never reach the event stream"
        );
        assert!(
            !rendered.contains(TEST_PASSWORD),
            "nor may the accepted one"
        );
        match after.last().map(|event| &event.state) {
            Some(OperationState::Completed {
                result: OperationResult::Merged { .. },
            }) => {}
            other => panic!("expected the corrected merge to complete, got {other:?}"),
        }
    });
}

/// Pins a preserved `arclain_core` behavior with real confidentiality
/// consequences: the password unlocks the *source* parts and is never
/// applied to the archive the merge writes, so merging an encrypted set
/// leaves a plaintext archive beside it. See
/// `arclain_app::operations::merge`'s own module doc comment for why this
/// operation preserves that rather than silently changing it.
#[test]
fn merging_an_encrypted_set_writes_an_unencrypted_archive() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping merging_an_encrypted_set_writes_an_unencrypted_archive: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-plaintext-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", Some(TEST_PASSWORD));
    let first_part = sets.join("rj123456.7z.001");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    let output_path = runtime.block_on(async {
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let mut merge = request(archive);
        merge.password = Some(SecretInput::new(TEST_PASSWORD.to_string()));
        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Completed {
                result: OperationResult::Merged { output_path },
            } => output_path,
            other => panic!("expected the merge to complete, got {other:?}"),
        }
    });

    // Listing without a password succeeds and shows real entry names --
    // neither the contents nor the headers of the merged archive are
    // encrypted, even though every source part was.
    let listed = entry_paths(&output_path, None);
    assert_eq!(listed.len(), 3);
    assert!(
        listed.iter().all(|path| !path.is_empty()),
        "the merged archive lists in the clear: {listed:?}"
    );
}

/// The destructive path's failure invariant on the **post-attempt** arm:
/// core ran, got far enough to extract, and then failed — and
/// `delete_originals: true` must still leave every part in place. The
/// pre-flight sibling cannot reach this arm (see its doc comment), so
/// without this test a regression that deleted on a genuine merge failure
/// would go unnoticed.
///
/// The failure is induced with a set whose only member is an empty
/// directory: extraction succeeds, yields no files, and core bails with
/// prose carrying no exit code — so it classifies as a real failure rather
/// than as a password problem.
#[test]
fn a_merge_that_fails_after_running_with_delete_originals_keeps_every_part() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping a_merge_that_fails_after_running_with_delete_originals_keeps_every_part: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-failpath-");
    let sets = build_fileless_split_set(&sevenzip, temp.path(), "rj123456");
    let first_part = sets.join("rj123456.7z.001");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    let enumerated = runtime.block_on(async {
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let enumerated = archive.parts.clone();
        assert!(!enumerated.is_empty());
        let mut merge = request(archive);
        merge.delete_originals = true;

        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");
        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Backend);
            }
            other => panic!("expected a Backend failure after the attempt ran, got {other:?}"),
        }
        enumerated
    });

    for part in enumerated {
        assert!(
            part.exists(),
            "a merge that ran and then failed must leave every part in place even with \
             delete_originals set"
        );
    }
    assert!(
        !sets.join("rj123456.7z").exists(),
        "and it must not have left an output archive behind"
    );
}

#[test]
fn a_seeded_password_merges_an_encrypted_set_without_prompting() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping a_seeded_password_merges_an_encrypted_set_without_prompting: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-seeded-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", Some(TEST_PASSWORD));
    let first_part = sets.join("rj123456.7z.001");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let mut merge = request(archive);
        merge.password = Some(SecretInput::new(TEST_PASSWORD.to_string()));

        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");
        let events = collect_until_terminal(&mut receiver, operation_id).await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.state, OperationState::Challenge { .. })),
            "a request that already carries the password must not prompt: {events:?}"
        );
        assert!(
            !format!("{events:?}").contains(TEST_PASSWORD),
            "no event may ever carry the supplied password"
        );
        match events.last().map(|event| &event.state) {
            Some(OperationState::Completed {
                result: OperationResult::Merged { .. },
            }) => {}
            other => panic!("expected the seeded merge to complete, got {other:?}"),
        }
    });

    assert!(sets.join("rj123456.7z").exists());
}

/// The one deterministically-reachable mid-merge cancellation point: the
/// merge is parked on its password challenge, so extraction has already
/// failed and nothing has been written. Cancelling there must leave the
/// output path empty and every original part in place.
///
/// Runs with `delete_originals: true` on purpose -- with the flag unset,
/// the surviving-parts assertion below could not fail no matter what the
/// deletion path did.
#[test]
fn cancelling_a_merge_parked_on_its_password_challenge_leaves_nothing_behind() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping cancelling_a_merge_parked_on_its_password_challenge_leaves_nothing_behind: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-cancel-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", Some(TEST_PASSWORD));
    let first_part = sets.join("rj123456.7z.001");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let mut merge = request(archive);
        merge.delete_originals = true;
        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");

        let (_challenge_id, _before) =
            wait_for_password_challenge(&mut receiver, operation_id).await;
        app.cancel_operation(operation_id)
            .await
            .expect("cancelling a parked merge must be accepted");

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Cancelled => {}
            other => panic!("expected the parked merge to end Cancelled, got {other:?}"),
        }
    });

    assert!(
        !sets.join("rj123456.7z").exists(),
        "a merge cancelled before it ever compressed must leave no output archive"
    );
    for index in 1..=3 {
        assert!(
            sets.join(format!("rj123456.7z.{index:03}")).exists(),
            "a cancelled merge never deletes originals"
        );
    }
}

/// The window a `MergeService`-owned deletion could not make safe: a
/// cancellation arriving around the moment the merge finishes writing.
///
/// The cancel is issued the instant core reports its own final progress
/// ("Merge complete", emitted immediately before `merge()` returns `Ok`),
/// which is as close to that moment as a caller can aim from outside. Two
/// interleavings are possible and **both are correct**:
///
/// - the cancellation transition wins the registry -> terminal
///   `Cancelled`, and the parts must all survive (the facade sequences its
///   own deletion after reading the terminal state back, so it never
///   runs);
/// - this operation's own `Completed { Merged }` wins -> the parts are
///   deleted, exactly as a plain successful merge would.
///
/// So the assertion is the implication, not one fixed outcome, which is
/// why this test can never flake. It is nonetheless the test that bites on
/// the regression: with deletion delegated to core, the first interleaving
/// reported `Cancelled` *with the parts already gone*, and the
/// surviving-parts branch below fails.
#[test]
fn cancelling_a_merge_as_it_finishes_never_loses_the_parts_silently() {
    let Some(sevenzip) = detect_real_sevenzip() else {
        eprintln!(
            "skipping cancelling_a_merge_as_it_finishes_never_loses_the_parts_silently: \
             no real 7-Zip CLI on this machine"
        );
        return;
    };
    let temp = scratch_dir("merge-cancel-late-");
    let sets = build_split_set(&sevenzip, temp.path(), "rj123456", None);
    let first_part = sets.join("rj123456.7z.001");
    let output_path = sets.join("rj123456.7z");

    let paths = support::temp_paths(&temp.path().join("profile"));
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    let app = bootstrap_app(paths);
    let runtime = foreign_runtime();

    let terminal = runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let archive = detect_multipart(&first_part).expect("the set is detected");
        let mut merge = request(archive);
        merge.delete_originals = true;
        let operation_id = app
            .start_merge(merge)
            .await
            .expect("the request is structurally valid");

        // Race the cancel against the very end of the merge: core emits
        // "Merge complete" after its own last cancellation check and
        // immediately before returning.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "the merge never reported completion");
            let event = match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Ok(event)) => event,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    panic!("the operation event stream closed early")
                }
                Err(_) => panic!("the merge never reported completion"),
            };
            if event.operation_id != operation_id {
                continue;
            }
            let reached_cores_last_report = matches!(
                &event.state,
                OperationState::Progress { message: Some(message), .. }
                    if message == "Merge complete"
            );
            let already_terminal = matches!(
                event.state,
                OperationState::Completed { .. }
                    | OperationState::Cancelled
                    | OperationState::Failed { .. }
            );
            if reached_cores_last_report || already_terminal {
                break;
            }
        }
        let _ = app.cancel_operation(operation_id).await;
        wait_for_terminal(&app, operation_id).await
    });

    let parts: Vec<PathBuf> = (1..=3)
        .map(|index| sets.join(format!("rj123456.7z.{index:03}")))
        .collect();
    match terminal {
        OperationState::Cancelled => {
            assert!(
                parts.iter().all(|part| part.exists()),
                "a merge reported as Cancelled must never have deleted its parts -- that is the \
                 whole reason this operation owns the deletion instead of delegating it"
            );
        }
        OperationState::Completed {
            result: OperationResult::Merged {
                output_path: written,
            },
        } => {
            assert_eq!(written, output_path);
            assert!(written.exists());
            assert!(
                parts.iter().all(|part| !part.exists()),
                "a merge reported as Completed with delete_originals must have removed the parts"
            );
        }
        other => panic!("expected Cancelled or Completed{{Merged}}, got {other:?}"),
    }
}
