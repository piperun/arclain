//! Integration tests for `arclain-cli`'s read surface: `inspect`, `list`,
//! and `profiles list`/`profiles show`. Drives the real compiled binary
//! via `std::process::Command`, using Cargo's own `CARGO_BIN_EXE_
//! arclain-cli` environment variable to locate it -- sufficient for every
//! scenario below, so no `assert_cmd`-style dev-dependency is needed.
//!
//! Every test that performs a real bootstrap passes `--config-dir`
//! pointed at a fresh `tempfile::TempDir`, so no test here ever touches a
//! real user profile, and every test gets its own independent, freshly
//! seeded configuration.
//!
//! Two facts about that fresh configuration are relied on but not (and,
//! per this crate's own dependency boundary -- it may depend on
//! `arclain_app` only -- cannot be) asserted against `arclain_core`
//! directly:
//!
//! - A brand new configuration database always seeds exactly three
//!   default archive/organization profiles, with ids `"1"`/`"2"`/`"3"`
//!   (`"Maximum Compression (7z)"`/`"Fast Compression (7z)"`/`"Zip
//!   Compatible"`) -- see `arclain_db`'s `seed_default_archive_profiles`,
//!   run unconditionally the first time `ArclainApp::bootstrap` opens a
//!   fresh `config.sqlite`. `crates/app/tests/settings_facade.rs`'s own
//!   `organization_profiles_lists_seeded_system_defaults` test relies on
//!   the same fact from the `app`-crate side.
//! - Bootstrap itself requires a real 7-Zip executable on `PATH` (fails
//!   outright otherwise) -- matching this workspace's established test
//!   convention (see `crates/app/tests/bootstrap.rs`'s own module doc
//!   comment). Every test here needs it regardless of which command it
//!   drives.
//!
//! `password_required_exits_with_user_action_required` additionally
//! shells out to the same real `7z` executable to build a *header-
//! encrypted* `.7z` fixture (`-mhe=on`): unlike a plain per-entry-
//! encrypted `.zip` (whose entries the native `zip` backend can list
//! without ever needing a password -- raw metadata access never touches
//! ciphertext), a header-encrypted 7z archive's directory itself is
//! ciphertext, so listing it at all requires the password, deterministically
//! reaching this task's read-surface Challenge/exit-3 path through the
//! exact backend chain (native 7z reader fails closed, falls back to the
//! real 7z CLI, which reports "Cannot open encrypted archive. Wrong
//! password?" -- a string `arclain_app::runtime::archive_ops::
//! is_password_error` already recognizes) production traffic would hit.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_arclain-cli")
}

/// One isolated `--config-dir` an entire test bootstraps its own
/// `ArclainApp` instance against.
struct Env {
    #[allow(dead_code)] // kept alive for the lifetime of the temp dir
    temp: tempfile::TempDir,
    config_dir: PathBuf,
}

impl Env {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create temp dir");
        let config_dir = temp.path().join("app-home");
        Self { temp, config_dir }
    }

    fn fixture_dir(&self) -> &Path {
        self.temp.path()
    }

    /// Runs `arclain-cli --config-dir <this env> <args...>`.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary_path())
            .arg("--config-dir")
            .arg(&self.config_dir)
            .args(args)
            .output()
            .expect("failed to spawn arclain-cli")
    }
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout must be valid UTF-8")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr must be valid UTF-8")
}

fn exit_code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("process must exit with a code, not a signal")
}

/// Builds a ZIP fixture at `dir/name` containing `entries` (archive-
/// relative path -> content). Mirrors `crates/app/tests/
/// archive_sessions.rs::build_zip_fixture`, duplicated here (rather than
/// shared) since this crate cannot depend on `arclain_app`'s own
/// dev-only test-support module.
fn build_zip_fixture(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_path, content) in entries {
        writer
            .start_file(*entry_path, options)
            .expect("start zip fixture entry");
        writer
            .write_all(content)
            .expect("write zip fixture entry content");
    }
    writer.finish().expect("finish zip fixture");
    path
}

/// Builds a *header-encrypted* 7z fixture (`-mhe=on`) by shelling out to
/// the real `7z` executable this whole test suite already requires on
/// `PATH` for bootstrap to succeed at all -- see this file's own module
/// doc comment for why this (and not a plain encrypted `.zip`) is what
/// actually reaches the password-challenge path.
fn build_header_encrypted_7z_fixture(dir: &Path, password: &str) -> PathBuf {
    let source = dir.join("secret.txt");
    std::fs::write(&source, b"classified contents").expect("write source file");
    let archive = dir.join("encrypted.7z");
    let status = Command::new("7z")
        .arg("a")
        .arg("-mhe=on")
        .arg(format!("-p{password}"))
        .arg(&archive)
        .arg(&source)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("failed to spawn the real 7z executable to build a test fixture");
    assert!(
        status.success(),
        "7z must succeed building the header-encrypted fixture"
    );
    archive
}

// ---------------------------------------------------------------------
// help / version / missing input -- no bootstrap needed: clap's own
// derive handles all three before any of this crate's code runs.
// ---------------------------------------------------------------------

#[test]
fn help_exits_zero_and_lists_every_subcommand() {
    let output = Command::new(binary_path()).arg("--help").output().unwrap();
    assert_eq!(exit_code(&output), 0);
    let stdout = stdout_text(&output);
    assert!(stdout.contains("inspect"), "stdout was: {stdout}");
    assert!(stdout.contains("list"), "stdout was: {stdout}");
    assert!(stdout.contains("profiles"), "stdout was: {stdout}");
}

#[test]
fn version_exits_zero_and_prints_the_binary_name_and_version() {
    let output = Command::new(binary_path())
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(exit_code(&output), 0);
    let stdout = stdout_text(&output);
    assert!(stdout.contains("arclain-cli"), "stdout was: {stdout}");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "stdout was: {stdout}"
    );
}

#[test]
fn missing_required_argument_exits_with_invocation_error() {
    // `inspect` with no ARCHIVE positional at all -- clap's own
    // required-argument enforcement must reject this before any of this
    // crate's code (bootstrap included) ever runs.
    let output = Command::new(binary_path()).arg("inspect").output().unwrap();
    assert_eq!(exit_code(&output), 2);
    assert!(!stderr_text(&output).is_empty());
}

// ---------------------------------------------------------------------
// inspect / list -- each of these bootstraps a real, isolated app.
// ---------------------------------------------------------------------

#[test]
fn inspect_of_a_nonexistent_archive_exits_unsupported_input() {
    let env = Env::new();
    let missing = env.fixture_dir().join("does-not-exist.zip");

    let output = env.run(&["inspect", missing.to_str().unwrap()]);

    assert_eq!(exit_code(&output), 4);
    assert!(stderr_text(&output).contains("not found"));
}

#[test]
fn inspect_json_reports_schema_version_one_and_archive_metadata() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[
            ("readme.txt", b"hello" as &[u8]),
            ("game/Game.exe", b"binary-content"),
        ],
    );

    let output = env.run(&["inspect", archive.to_str().unwrap(), "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["data"]["archive_type"], "zip");
    assert_eq!(
        json["data"]["entry_count"], 3,
        "2 files + 1 synthesized 'game' folder"
    );
    assert_eq!(json["data"]["total_uncompressed_size"], 5 + 14);
}

#[test]
fn list_of_a_nested_directory_returns_only_its_direct_children() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[
            ("readme.txt", b"hello" as &[u8]),
            ("game/Game.exe", b"binary-content"),
            ("game/data/save.dat", b"01234567890123456789"),
        ],
    );

    let output = env.run(&["list", archive.to_str().unwrap(), "game", "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["schema_version"], 1);
    let names: Vec<&str> = json["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    // Case-insensitive alphabetical: "data" < "Game.exe" ('d' < 'g'),
    // matching arclain_app's own established sort convention (see
    // crates/app/tests/archive_sessions.rs).
    assert_eq!(names, ["data", "Game.exe"]);
    assert_eq!(json["data"]["directory"], "game");
}

#[test]
fn list_pagination_returns_the_requested_page_and_the_full_total() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[
            ("a.txt", b"1" as &[u8]),
            ("b.txt", b"2"),
            ("c.txt", b"3"),
            ("d.txt", b"4"),
            ("e.txt", b"5"),
        ],
    );

    let output = env.run(&[
        "list",
        archive.to_str().unwrap(),
        "--offset",
        "2",
        "--limit",
        "2",
        "--json",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["data"]["total"], 5);
    let names: Vec<&str> = json["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["c.txt", "d.txt"]);
}

#[test]
fn list_rejects_an_invalid_in_archive_path() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );

    let output = env.run(&["list", archive.to_str().unwrap(), ".."]);

    assert_eq!(exit_code(&output), 4);
    assert!(!stderr_text(&output).is_empty());
}

// ---------------------------------------------------------------------
// profiles -- rely on the three system-default profiles a fresh
// configuration database always seeds (see this file's own module doc
// comment).
// ---------------------------------------------------------------------

#[test]
fn profiles_list_json_reports_the_seeded_system_defaults() {
    let env = Env::new();

    let output = env.run(&["profiles", "list", "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["schema_version"], 1);
    let profiles = json["data"].as_array().unwrap();
    assert_eq!(profiles.len(), 3, "profiles were: {profiles:?}");
    assert!(profiles
        .iter()
        .any(|profile| profile["id"] == "1" && profile["name"] == "Maximum Compression (7z)"));
}

#[test]
fn profiles_show_json_returns_the_matching_profile() {
    let env = Env::new();

    let output = env.run(&["profiles", "show", "1", "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["data"]["id"], "1");
    assert_eq!(json["data"]["name"], "Maximum Compression (7z)");
    assert_eq!(json["data"]["output_format"], "7z");
}

#[test]
fn profiles_show_of_an_unknown_id_exits_unsupported_input() {
    let env = Env::new();

    let output = env.run(&["profiles", "show", "does-not-exist"]);

    assert_eq!(exit_code(&output), 4);
    assert!(stderr_text(&output).contains("no such profile"));
}

// ---------------------------------------------------------------------
// JSON output must never contain an ANSI escape sequence.
// ---------------------------------------------------------------------

#[test]
fn no_ansi_escape_codes_appear_in_json_output() {
    let env = Env::new();

    let output = env.run(&["profiles", "list", "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "JSON stdout must never contain an ESC byte"
    );
}

// ---------------------------------------------------------------------
// password challenge -- this task's read commands supply no interactive
// input, so any challenge (a password prompt above all) must exit 3.
// ---------------------------------------------------------------------

#[test]
fn password_required_exits_with_user_action_required() {
    let env = Env::new();
    let archive =
        build_header_encrypted_7z_fixture(env.fixture_dir(), "correct-horse-battery-staple");

    let output = env.run(&["inspect", archive.to_str().unwrap()]);

    assert_eq!(
        exit_code(&output),
        3,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(stderr_text(&output).to_lowercase().contains("password"));
}

// ---------------------------------------------------------------------
// Cross-check: the CLI's `list` output must match `arclain_app::
// ArclainApp::list_entries` called directly against the identical ZIP
// fixture. `arclain_app` is this crate's own sanctioned dependency (see
// scripts/frontend_boundary.py), so this is a legitimate in-process
// comparison -- the requested comparison against egui for the same
// fixture corpus is satisfied by comparing against the shared facade
// both consume instead, since egui itself is unreachable (and
// undesirable to depend on) from this crate's own test suite. Session
// ids/revisions are expected to differ (each side bootstraps its own,
// independent `ArclainApp` instance) and are deliberately excluded from
// the comparison; only the actual listed content is compared.
// ---------------------------------------------------------------------

#[test]
fn cli_list_output_matches_list_entries_called_directly_against_the_facade() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[
            ("readme.txt", b"hello" as &[u8]),
            ("game/Game.exe", b"binary-content"),
            ("game/data/save.dat", b"01234567890123456789"),
        ],
    );

    let output = env.run(&["list", archive.to_str().unwrap(), "--json"]);
    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    let cli_total = json["data"]["total"].as_u64().unwrap();
    let mut cli_names: Vec<String> = json["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    cli_names.sort();

    let facade_page = direct_facade_list(&archive);
    let mut facade_names: Vec<String> = facade_page
        .entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    facade_names.sort();

    assert_eq!(cli_total, facade_page.total, "entry counts must match");
    assert_eq!(cli_names, facade_names, "entry names must match");
}

/// Bootstraps a second, independent `ArclainApp` in-process (its own
/// temp profile, entirely separate from the CLI subprocess's own) and
/// lists the archive root directly through the facade -- the "ground
/// truth" the CLI subprocess's own JSON output above is compared
/// against.
fn direct_facade_list(archive: &Path) -> arclain_app::archive::EntryPage {
    let temp = tempfile::tempdir().unwrap();
    let paths = arclain_app::AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    let app = arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(paths),
        ..arclain_app::BootstrapConfig::system_default()
    })
    .expect("direct facade bootstrap must succeed (requires a real 7-Zip executable on PATH)");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let page = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: archive.to_path_buf(),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let session_id = loop {
            let snapshot = app
                .operation(operation_id)
                .await
                .expect("operation must exist");
            match snapshot.state {
                arclain_app::event::OperationState::Completed {
                    result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
                } => break snapshot.session_id,
                arclain_app::event::OperationState::Failed { error } => {
                    panic!("direct facade archive open unexpectedly failed: {error:?}")
                }
                _ if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                _ => panic!("direct facade archive open did not complete in time"),
            }
        };

        app.list_entries(
            session_id,
            arclain_app::archive::ListEntriesRequest {
                directory: arclain_app::archive::ArchivePath::root(),
                sort_key: arclain_app::archive::EntrySortKey::Name,
                sort_direction: arclain_app::archive::SortDirection::Ascending,
                name_filter: None,
                offset: 0,
                limit: 1000,
            },
        )
        .await
        .expect("list_entries must succeed")
    });

    runtime.block_on(app.shutdown()).ok();
    page
}
