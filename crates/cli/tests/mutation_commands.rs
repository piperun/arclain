//! Integration tests for `arclain-cli`'s mutation surface (`extract`,
//! `convert`, `organize`, `archive add`/`delete`, `pipeline run`,
//! `plugins list`/`enable`/`disable`/`action`, `settings show`/`set-*`)
//! and its shared event-driving contract (JSON Lines framing, human
//! progress rendering, non-interactive challenge refusal, collision
//! handling, exit-code mapping). Drives the real compiled binary via
//! `std::process::Command`, exactly like `tests/read_commands.rs`.
//!
//! What this file does **not** attempt (documented, not silently
//! skipped -- see `crates/cli/src/events.rs`'s own test module doc
//! comment for the full rationale): a real Ctrl+C-during-extraction
//! test and a real interactive (pseudo-TTY) password/confirm prompt.
//! Reliably raising a real console Ctrl+C against a spawned child on
//! this workspace's Windows target requires the child to share a real
//! console with the sender and is prone to hanging or silently no-op'ing
//! in a sandboxed CI environment with no console at all; a real
//! interactive terminal read needs a pseudo-TTY this test harness does
//! not allocate. Both are covered **in-process**, deterministically, in
//! `crate::events`'s own test module instead, against the exact same
//! `drive_operation`/`handle_challenge` functions the commands below
//! call. What *is* covered here, for real, at the subprocess level: every
//! non-interactive refusal path (a piped/closed stdin is never a real
//! terminal, so `Interactive::is_interactive` genuinely reports `false`
//! for every test below, exactly as it would for a script or a CI job),
//! which is the far more security-relevant property (a password/
//! confirmation prompt must never silently hang or auto-answer when
//! nothing is there to answer it).
//!
//! Every test that performs a real bootstrap requires a real 7-Zip
//! executable on `PATH` (this workspace's established convention -- see
//! `tests/read_commands.rs`'s own module doc comment) and, for the
//! plugin tests, the real `ui-demo`/`facade-test-fixture` `.wasm`
//! fixtures already built via `just plugins`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_arclain-cli")
}

/// One isolated `--config-dir` an entire test bootstraps its own
/// `ArclainApp` instance against. Mirrors `tests/read_commands.rs::Env`.
struct Env {
    #[allow(dead_code)]
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

    /// Runs `arclain-cli --config-dir <this env> <args...>` with stdin
    /// closed (`.output()`'s own documented default) -- never a real
    /// terminal, matching this file's own module doc comment.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary_path())
            .arg("--config-dir")
            .arg(&self.config_dir)
            .args(args)
            .output()
            .expect("failed to spawn arclain-cli")
    }

    /// Installs a maintained workspace plugin fixture into this env's own
    /// `plugins/{name}/` folder before any command runs -- mirrors
    /// `crates/app/tests/plugin_sessions.rs::install_plugin_fixture`.
    fn install_plugin_fixture(&self, name: &str) {
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins")
            .join(name);
        let fixture_component = if name == "ui-demo" {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../plugins/tests/fixtures/wirt/ui-demo.wasm")
        } else {
            fixture_dir.join(format!("{name}.wasm"))
        };
        let dest_dir = self.config_dir.join("plugins").join(name);
        std::fs::create_dir_all(&dest_dir).expect("create plugin fixture directory");
        std::fs::copy(fixture_component, dest_dir.join(format!("{name}.wasm"))).unwrap_or_else(
            |error| {
                panic!(
                    "copy maintained {name}.wasm: {error} -- fixture dir was {}",
                    fixture_dir.display()
                )
            },
        );
        std::fs::copy(
            fixture_dir.join("plugin.toml"),
            dest_dir.join(format!("{name}.toml")),
        )
        .unwrap_or_else(|error| panic!("copy {name}.toml: {error}"));
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

/// Builds a ZIP fixture at `dir/name` containing `entries`. Mirrors
/// `tests/read_commands.rs::build_zip_fixture`.
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

/// Builds a *header-encrypted* 7z fixture (`-mhe=on`) -- see
/// `tests/read_commands.rs`'s own module doc comment for why this (and
/// not a plain per-entry-encrypted `.zip`) is what actually reaches this
/// CLI's password-challenge path.
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed to spawn the real 7z executable to build a test fixture");
    assert!(
        status.success(),
        "7z must succeed building the header-encrypted fixture"
    );
    archive
}

fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    stdout_text(output)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!(
                    "every stdout line must be its own valid JSON value: {error}\nline was: {line}"
                )
            })
        })
        .collect()
}

/// The final line of a `--json` mutation command's stdout -- the one
/// schema-versioned `CliEnvelope` line `crate::output::print_json_line`
/// prints, per `crate::events`'s own documented JSON Lines framing
/// (every event *before* it is a raw, unwrapped `OperationEvent` line;
/// this is the only line carrying `schema_version`). Every mutation
/// command's own `--json` assertions in this file use this, never a
/// bare `serde_json::from_str(&stdout_text(...))` over the whole
/// stream -- unlike `inspect`/`list`/`profiles`/`plugins list`/`settings
/// show` (which print exactly one JSON object and nothing else), a
/// mutation command's stdout is a multi-line JSON Lines stream, so
/// parsing the whole thing as one JSON value fails with "trailing
/// characters" the instant more than one event was observed.
fn final_envelope(output: &Output) -> serde_json::Value {
    json_lines(output)
        .into_iter()
        .last()
        .expect("a --json mutation command must print at least its own final envelope line")
}

/// Probes whether the real `7z` executable on `PATH` can pack an
/// extensionless destination file (the exact shape `arclain_core`'s own
/// staged-output machinery uses: `.arclain-output-XXXXXX/artifact`, with
/// no `.zip`/`.7z` suffix) -- some 7-Zip builds fail to read back their
/// own just-written archive's metadata in that shape. Mirrors
/// `crates/app/tests/processing_operations.rs::detect_unaffected_sevenzip`'s
/// own probe exactly (duplicated rather than shared: this crate cannot
/// depend on `arclain_core` directly -- see `scripts/frontend_boundary.py`'s
/// dependency boundary -- so there is no shared path to call that
/// helper from here). Every test that needs a real, successful
/// `Convert`/`Pipeline` output (not just a rejection path) gates on
/// this and skips (with a clear message), rather than failing, when the
/// installed `7z` is affected -- exactly like that facade test does.
fn sevenzip_handles_extensionless_output() -> bool {
    let Ok(probe) = tempfile::tempdir() else {
        return false;
    };
    let source = probe.path().join("src");
    if std::fs::create_dir_all(&source).is_err() {
        return false;
    }
    if std::fs::write(source.join("probe.bin"), b"probe").is_err() {
        return false;
    }
    let dest = probe.path().join("artifact"); // deliberately extensionless
    let status = Command::new("7z")
        .arg("a")
        .arg("-tzip")
        .arg("-bb0")
        .arg("-y")
        .arg(&dest)
        .arg(format!(
            "{}{}*",
            source.display(),
            std::path::MAIN_SEPARATOR
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(status) if status.success()) && dest.exists()
}

// ---------------------------------------------------------------------
// No command-line flag ever accepts a password.
// ---------------------------------------------------------------------

#[test]
fn no_subcommand_accepts_a_password_command_line_flag() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--password",
        "hunter2",
    ]);

    // clap itself rejects the unrecognized flag before any of this
    // crate's own code runs -- its own usage-error exit code.
    assert_eq!(
        exit_code(&output),
        2,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(!stderr_text(&output).contains("hunter2"));
}

// ---------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------

#[test]
fn extract_whole_archive_writes_every_file_to_disk() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"hello" as &[u8]), ("dir/b.txt", b"world")],
    );
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert_eq!(std::fs::read(destination.join("a.txt")).unwrap(), b"hello");
    assert_eq!(
        std::fs::read(destination.join("dir/b.txt")).unwrap(),
        b"world"
    );
    assert!(stdout_text(&output).contains("extraction complete"));
}

#[test]
fn extract_specific_entry_by_path_extracts_only_that_file() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"keep" as &[u8]), ("b.txt", b"skip")],
    );
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "a.txt",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(destination.join("a.txt").exists());
    assert!(!destination.join("b.txt").exists());
}

#[test]
fn extract_unknown_entry_path_exits_unsupported_input() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "does-not-exist.txt",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

#[test]
fn extract_of_a_password_protected_archive_exits_user_action_required_non_interactively() {
    let env = Env::new();
    let archive =
        build_header_encrypted_7z_fixture(env.fixture_dir(), "correct-horse-battery-staple");
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);

    assert_eq!(
        exit_code(&output),
        3,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(stderr_text(&output).to_lowercase().contains("password"));
}

#[test]
fn extract_json_mode_emits_json_lines_with_the_envelope_last_and_increasing_sequence() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--json",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let lines = json_lines(&output);
    assert!(
        lines.len() >= 2,
        "expected at least Accepted and a terminal event, got {lines:?}"
    );

    let (last, progress_lines) = lines.split_last().unwrap();
    assert!(
        last.get("schema_version").is_some(),
        "the final line must be the schema-versioned envelope: {last:?}"
    );
    assert_eq!(last["data"]["status"], "completed");

    let mut previous_sequence = 0u64;
    for line in progress_lines {
        assert!(
            line.get("schema_version").is_none(),
            "a raw event line must never carry schema_version: {line:?}"
        );
        let sequence = line["sequence"]
            .as_u64()
            .expect("every event line has a sequence");
        assert!(
            sequence > previous_sequence,
            "sequence must strictly increase"
        );
        previous_sequence = sequence;
    }
}

#[test]
fn extract_collision_ask_exits_user_action_required_non_interactively() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"new" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("a.txt"), b"already here").unwrap();

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--collision",
        "ask",
    ]);

    assert_eq!(
        exit_code(&output),
        3,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert_eq!(
        std::fs::read(destination.join("a.txt")).unwrap(),
        b"already here"
    );
}

#[test]
fn extract_collision_skip_preserves_the_existing_file() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"new" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("a.txt"), b"already here").unwrap();

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--collision",
        "skip",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert_eq!(
        std::fs::read(destination.join("a.txt")).unwrap(),
        b"already here"
    );
}

#[test]
fn extract_collision_overwrite_replaces_the_existing_file() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"new" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("a.txt"), b"already here").unwrap();

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--collision",
        "overwrite",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert_eq!(std::fs::read(destination.join("a.txt")).unwrap(), b"new");
}

#[test]
fn extract_collision_rename_keeps_the_existing_file_and_writes_a_renamed_copy() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"new" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("a.txt"), b"already here").unwrap();

    let output = env.run(&[
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--collision",
        "rename",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    // The pre-existing file is left completely untouched...
    assert_eq!(
        std::fs::read(destination.join("a.txt")).unwrap(),
        b"already here"
    );
    // ...and the extracted content lands under a renamed copy instead of
    // being dropped or silently overwriting it (see
    // `arclain_app::operations::extract`'s own `move_with_rename_on_collision`
    // doc comment for the exact `" (n)"` suffix convention).
    assert_eq!(
        std::fs::read(destination.join("a (1).txt")).unwrap(),
        b"new"
    );
}

// ---------------------------------------------------------------------
// --timeout -- the barrier-stalled-operation cases (times out and exits
// `OPERATION_FAILURE`; without the flag, a released operation completes
// normally) are covered deterministically in-process, against a fake
// `ExtractRunner` this crate cannot inject into the *compiled binary*
// this file drives as a subprocess -- see `crate::events::tests::
// timeout_cancels_a_stalled_operation_and_exits_operation_failure`/
// `without_a_timeout_a_released_operation_completes_normally`. This is
// the smoke-level counterpart: proves the real `--timeout` flag itself
// parses and threads through the real binary end to end, generous
// enough that a real (fast) extraction is never actually at risk of
// tripping it.
// ---------------------------------------------------------------------

#[test]
fn extract_with_a_generous_timeout_still_completes_normally() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "--timeout",
        "60",
        "extract",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(destination.join("a.txt").exists());
}

// ---------------------------------------------------------------------
// convert
// ---------------------------------------------------------------------

#[test]
fn convert_to_zip_writes_an_output_archive() {
    if !sevenzip_handles_extensionless_output() {
        eprintln!(
            "skipping convert_to_zip_writes_an_output_archive: the installed 7z cannot read back \
             an extensionless staged archive (a known 7-Zip compatibility gap this workspace's \
             own facade tests also gate on -- see detect_unaffected_sevenzip in \
             crates/app/tests/processing_operations.rs)"
        );
        return;
    }
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "convert",
        input.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--format",
        "zip",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let produced: Vec<_> = std::fs::read_dir(&destination)
        .expect("destination must exist")
        .collect();
    assert!(
        !produced.is_empty(),
        "convert must write at least one output file"
    );
}

#[test]
fn convert_missing_input_exits_unsupported_input() {
    let env = Env::new();
    let missing = env.fixture_dir().join("does-not-exist.zip");
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "convert",
        missing.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--format",
        "zip",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

#[test]
fn convert_unrecognized_format_exits_unsupported_input() {
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "convert",
        input.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--format",
        "rar",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

// ---------------------------------------------------------------------
// organize -- this workspace's fresh-bootstrap config seeds three
// archive profiles (see `tests/read_commands.rs`'s own module doc
// comment) but no organization rules, so there is no rule id this test
// suite can rely on existing out of the box. Only the "rule/profile
// does not exist" rejection path is exercised here; the successful
// pack-and-apply-plan path is already covered directly against the
// facade in `crates/app/tests/processing_operations.rs`.
// ---------------------------------------------------------------------

#[test]
fn organize_with_an_unknown_rule_id_exits_unsupported_input() {
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "organize",
        input.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--profile",
        "1",
        "--rule",
        "999999",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

#[test]
fn organize_with_a_non_numeric_id_exits_unsupported_input() {
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);
    let destination = env.fixture_dir().join("out");

    let output = env.run(&[
        "organize",
        input.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--profile",
        "not-a-number",
        "--rule",
        "1",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

// ---------------------------------------------------------------------
// archive add / delete
// ---------------------------------------------------------------------

#[test]
fn archive_add_then_delete_round_trips_through_separate_invocations() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );
    let new_file = env.fixture_dir().join("new.txt");
    std::fs::write(&new_file, b"brand new content").unwrap();

    let add_output = env.run(&[
        "archive",
        "add",
        archive.to_str().unwrap(),
        new_file.to_str().unwrap(),
    ]);
    assert_eq!(
        exit_code(&add_output),
        0,
        "stderr was: {}",
        stderr_text(&add_output)
    );
    assert!(stdout_text(&add_output).contains("archive updated to revision"));

    let list_output = env.run(&["list", archive.to_str().unwrap(), "--json"]);
    assert_eq!(
        exit_code(&list_output),
        0,
        "stderr was: {}",
        stderr_text(&list_output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&list_output)).unwrap();
    let names: Vec<&str> = json["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"new.txt"), "names were: {names:?}");

    let delete_output = env.run(&["archive", "delete", archive.to_str().unwrap(), "new.txt"]);
    assert_eq!(
        exit_code(&delete_output),
        0,
        "stderr was: {}",
        stderr_text(&delete_output)
    );

    let list_after = env.run(&["list", archive.to_str().unwrap(), "--json"]);
    let json_after: serde_json::Value = serde_json::from_str(&stdout_text(&list_after)).unwrap();
    let names_after: Vec<&str> = json_after["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert!(
        !names_after.contains(&"new.txt"),
        "names were: {names_after:?}"
    );
}

#[test]
fn archive_add_missing_source_exits_unsupported_input() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );
    let missing = env.fixture_dir().join("does-not-exist.txt");

    let output = env.run(&[
        "archive",
        "add",
        archive.to_str().unwrap(),
        missing.to_str().unwrap(),
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

#[test]
fn archive_delete_unknown_entry_exits_unsupported_input() {
    let env = Env::new();
    let archive = build_zip_fixture(
        env.fixture_dir(),
        "fixture.zip",
        &[("a.txt", b"1" as &[u8])],
    );

    let output = env.run(&[
        "archive",
        "delete",
        archive.to_str().unwrap(),
        "does-not-exist.txt",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

// ---------------------------------------------------------------------
// pipeline run -- `--preset "Convert to 7z (Max)"` is one of this
// workspace's own builtin presets (`arclain_core::builtin_presets`),
// resolvable on a fresh bootstrap with no `pipeline_presets.json` ever
// written -- see `arclain_core::features::pipeline::presets::load_presets`'s
// own fallback.
// ---------------------------------------------------------------------

#[test]
fn pipeline_run_with_a_builtin_preset_produces_a_7z_output() {
    if !sevenzip_handles_extensionless_output() {
        eprintln!(
            "skipping pipeline_run_with_a_builtin_preset_produces_a_7z_output: the installed 7z \
             cannot read back an extensionless staged archive -- see \
             sevenzip_handles_extensionless_output's own doc comment"
        );
        return;
    }
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);

    let output = env.run(&[
        "pipeline",
        "run",
        input.to_str().unwrap(),
        "--same-folder",
        "--preset",
        "Convert to 7z (Max)",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(
        env.fixture_dir().join("input.7z").exists(),
        "expected input.7z next to input.zip"
    );
}

#[test]
fn pipeline_run_requires_exactly_one_of_destination_or_same_folder() {
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);

    let neither = env.run(&[
        "pipeline",
        "run",
        input.to_str().unwrap(),
        "--preset",
        "Convert to 7z (Max)",
    ]);
    assert_eq!(
        exit_code(&neither),
        4,
        "stderr was: {}",
        stderr_text(&neither)
    );

    let both = env.run(&[
        "pipeline",
        "run",
        input.to_str().unwrap(),
        "--same-folder",
        "--destination",
        env.fixture_dir().join("out").to_str().unwrap(),
        "--preset",
        "Convert to 7z (Max)",
    ]);
    assert_eq!(exit_code(&both), 4, "stderr was: {}", stderr_text(&both));
}

#[test]
fn pipeline_run_unknown_preset_exits_unsupported_input() {
    let env = Env::new();
    let input = build_zip_fixture(env.fixture_dir(), "input.zip", &[("a.txt", b"1" as &[u8])]);

    let output = env.run(&[
        "pipeline",
        "run",
        input.to_str().unwrap(),
        "--same-folder",
        "--preset",
        "does-not-exist",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

// ---------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------

#[test]
fn settings_show_json_reports_no_raw_secret_values() {
    let env = Env::new();

    let output = env.run(&["settings", "show", "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["schema_version"], 1);
    // The DTOs this facade returns structurally never carry a secret
    // value -- only a `*_configured`/`vault_available` boolean -- so
    // this is a schema-shape assertion, not a heuristic content scan.
    assert!(json["data"]["security"]["vault_available"].is_boolean());
    assert!(json["data"]["network"]["socks5_password_configured"].is_boolean());
    assert!(json["data"]["network"]["gameta_api_key_configured"].is_boolean());
    assert!(json["data"]["security"].get("socks5_password").is_none());
    assert!(json["data"]["network"].get("gameta_api_key").is_none());
}

#[test]
fn settings_set_sevenzip_path_updates_the_stored_value() {
    let env = Env::new();
    let new_path = env.fixture_dir().join("custom-7z.exe");
    std::fs::write(&new_path, b"not a real binary, just a path to store").unwrap();

    let set_output = env.run(&["settings", "set-sevenzip-path", new_path.to_str().unwrap()]);
    assert_eq!(
        exit_code(&set_output),
        0,
        "stderr was: {}",
        stderr_text(&set_output)
    );

    let show_output = env.run(&["settings", "show", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&show_output)).unwrap();
    let stored = json["data"]["archive"]["sevenzip_path"].as_str().unwrap();
    assert!(
        Path::new(stored).ends_with("custom-7z.exe"),
        "stored path was: {stored}"
    );
}

#[test]
fn settings_set_backend_mode_rejects_an_unknown_mode() {
    let env = Env::new();

    let output = env.run(&["settings", "set-backend-mode", "not-a-real-mode"]);

    // clap's own `ValueEnum` parsing rejects this before any of this
    // crate's own code runs -- its own usage-error exit code.
    assert_eq!(
        exit_code(&output),
        2,
        "stderr was: {}",
        stderr_text(&output)
    );
}

// ---------------------------------------------------------------------
// plugins
// ---------------------------------------------------------------------

#[test]
fn plugins_list_with_nothing_installed_reports_empty() {
    let env = Env::new();

    let output = env.run(&["plugins", "list", "--json"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&output)).unwrap();
    assert_eq!(json["data"].as_array().unwrap().len(), 0);
}

#[test]
fn plugins_enable_and_disable_each_succeed_against_a_real_installed_plugin() {
    let env = Env::new();
    env.install_plugin_fixture("ui-demo");

    let list_output = env.run(&["plugins", "list", "--json"]);
    assert_eq!(
        exit_code(&list_output),
        0,
        "stderr was: {}",
        stderr_text(&list_output)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&list_output)).unwrap();
    let plugins = json["data"].as_array().unwrap();
    let ui_demo = plugins
        .iter()
        .find(|p| p["id"] == "ui-demo")
        .expect("ui-demo must be listed");
    assert_eq!(
        ui_demo["enabled"], true,
        "a freshly discovered plugin is enabled by default"
    );

    let disable_output = env.run(&["plugins", "disable", "ui-demo"]);
    assert_eq!(
        exit_code(&disable_output),
        0,
        "stderr was: {}",
        stderr_text(&disable_output)
    );
    assert!(stdout_text(&disable_output).contains("disabled"));

    let enable_output = env.run(&["plugins", "enable", "ui-demo"]);
    assert_eq!(
        exit_code(&enable_output),
        0,
        "stderr was: {}",
        stderr_text(&enable_output)
    );
    assert!(stdout_text(&enable_output).contains("enabled"));
}

/// `ArclainApp::set_plugin_enabled` persists a full snapshot of every
/// plugin's enabled state to `UserConfig::enabled_plugins` (see
/// `arclain_app::runtime::settings_ops::run_set_plugin_enabled`'s own doc
/// comment), and `runtime::bootstrap::run` reconciles a freshly-discovered
/// `PluginManager`'s default-enabled plugins against it on every
/// bootstrap. A CLI invocation is a brand-new process with a brand-new
/// `PluginManager` every single time, but both invocations here share the
/// same `--config-dir`, so the disable a separate, later process reads
/// back is exactly the one this test just persisted.
#[test]
fn plugins_disable_persists_and_is_observed_by_a_separate_invocation() {
    let env = Env::new();
    env.install_plugin_fixture("ui-demo");

    let disable_output = env.run(&["plugins", "disable", "ui-demo"]);
    assert_eq!(
        exit_code(&disable_output),
        0,
        "stderr was: {}",
        stderr_text(&disable_output)
    );

    let list_output = env.run(&["plugins", "list", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout_text(&list_output)).unwrap();
    let ui_demo = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "ui-demo")
        .unwrap();
    assert_eq!(
        ui_demo["enabled"], false,
        "a separate CLI invocation must observe the previous process's persisted disable"
    );
}

#[test]
fn plugins_enable_unknown_id_exits_unsupported_input() {
    let env = Env::new();

    let output = env.run(&["plugins", "enable", "does-not-exist"]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
}

#[test]
fn plugins_action_rejects_an_unknown_node_and_never_echoes_the_supplied_value() {
    let env = Env::new();
    env.install_plugin_fixture("ui-demo");

    let output = env.run(&[
        "plugins",
        "action",
        "ui-demo",
        "does-not-exist-node",
        "--value-json",
        "\"correct horse battery staple\"",
    ]);

    assert_eq!(
        exit_code(&output),
        4,
        "stderr was: {}",
        stderr_text(&output)
    );
    assert!(
        !stderr_text(&output).contains("correct horse battery staple"),
        "the rejected value must never be echoed back: {}",
        stderr_text(&output)
    );
}

#[test]
fn plugins_action_dispatches_a_real_button_and_reports_its_intents() {
    let env = Env::new();
    env.install_plugin_fixture("facade-test-fixture");

    let output = env.run(&[
        "plugins",
        "action",
        "facade-test-fixture",
        "multi-action",
        "--json",
    ]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let envelope = final_envelope(&output);
    assert_eq!(envelope["schema_version"], 1);
    let intents = envelope["data"]["intents"].as_array().unwrap();
    assert_eq!(
        intents.len(),
        3,
        "the fixture's multi-action button returns three intents"
    );
    assert_eq!(intents[0]["type"], "show_toast");
    assert_eq!(intents[0]["message"], "first");
}

#[test]
fn plugins_action_human_mode_never_prints_the_full_node_tree() {
    let env = Env::new();
    env.install_plugin_fixture("facade-test-fixture");

    let output = env.run(&["plugins", "action", "facade-test-fixture", "multi-action"]);

    assert_eq!(
        exit_code(&output),
        0,
        "stderr was: {}",
        stderr_text(&output)
    );
    let stdout = stdout_text(&output);
    assert!(stdout.contains("toast"));
    assert!(stdout.contains("action dispatched"));
    // The updated document's own label text (`"layout-call-N"`) is part
    // of the full `PluginUiUpdate` this command's `--json` mode would
    // include, but must never leak into the terse human-mode summary.
    assert!(!stdout.contains("layout-call"));
}
