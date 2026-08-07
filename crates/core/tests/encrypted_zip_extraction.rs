//! Extracting one entry out of a content-encrypted ZIP, through the real
//! backend chain a caller materializing an archive entry drives.
//!
//! A ZIP's central directory stays plaintext when only the entry
//! *contents* are encrypted, so such an archive opens and lists like any
//! other and the failure only appears at extraction time -- where a
//! backend that reports success without writing anything hands the caller
//! a path that is not there, and the caller reports a bare "file not
//! found" that says nothing about the password it actually needs.
//!
//! Every test needs a real 7-Zip CLI (it both builds the encrypted
//! fixture and is the fallback tier under test) and skips without one,
//! so the suite never depends on one being installed.

use arclain_core::backends::selector::BackendSelector;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::ArchiveBackend;
use std::path::{Path, PathBuf};

const FIXTURE_PASSWORD: &str = "SECRET";
const FIXTURE_ENTRY: &str = "secret.txt";
const FIXTURE_CONTENTS: &[u8] = b"classified payload";

/// Scratch space under the crate's own checkout rather than the system
/// temp directory: on a machine where that resolves to a RAM disk, a
/// real 7-Zip child process's writes there have raced this suite's own
/// filesystem checks before.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-scratch")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test's scratch directory");
    dir
}

/// Builds a ZIP whose entry contents are password-encrypted but whose
/// central directory is not, and returns its path -- `None` when no
/// 7-Zip CLI is installed to build it with.
fn build_content_encrypted_zip(dir: &Path) -> Option<PathBuf> {
    let cli = SevenZipCli::detect(None).ok()?;
    let plain = dir.join(FIXTURE_ENTRY);
    std::fs::write(&plain, FIXTURE_CONTENTS).expect("write the fixture's plaintext");

    let archive = dir.join("content-encrypted.zip");
    let status = std::process::Command::new(cli.exe_path())
        .arg("a")
        .arg("-tzip")
        .arg(format!("-p{FIXTURE_PASSWORD}"))
        .arg(&archive)
        .arg(&plain)
        .output()
        .expect("run 7-Zip to build the fixture");
    assert!(
        status.status.success(),
        "building the encrypted fixture failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    std::fs::remove_file(&plain).expect("remove the fixture's plaintext");
    Some(archive)
}

/// Extracting an entry whose contents are encrypted, with no password,
/// must fail and say so. Reporting success writes nothing, and the
/// caller then trips over a missing file with no idea a password was
/// ever involved.
#[test]
fn extracting_an_encrypted_entry_without_a_password_fails_naming_the_password() {
    let dir = scratch_dir("encrypted-entry-extraction");
    let Some(archive) = build_content_encrypted_zip(&dir) else {
        eprintln!(
            "skipping extracting_an_encrypted_entry_without_a_password_fails_naming_the_password: \
             no 7-Zip CLI on this machine"
        );
        return;
    };

    let backend = BackendSelector::new_native()
        .select(&archive)
        .expect("a zip selects a backend");

    // The archive lists like any other -- the failure is extraction's.
    let info = backend.list(&archive, None).expect("the fixture lists");
    let entry = info
        .entries
        .iter()
        .find(|entry| entry.path == FIXTURE_ENTRY)
        .expect("the fixture's entry is listed");
    assert!(entry.encrypted, "control: the entry must be encrypted");

    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).expect("create the destination");
    let files = vec![FIXTURE_ENTRY.to_string()];

    let error = backend
        .extract_files_with_progress(&archive, &dest, &files, None, None, None)
        .expect_err("extracting an entry this chain cannot decrypt must fail");

    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.to_lowercase().contains("password"),
        "the failure must name the password protection that caused it, got: {diagnostic}"
    );
    assert!(
        !dest.join(FIXTURE_ENTRY).exists()
            || std::fs::read(dest.join(FIXTURE_ENTRY)).unwrap() != FIXTURE_CONTENTS,
        "nothing decrypted may be written without the password"
    );
}

/// The 7-Zip CLI answers "Everything is Ok" with exit code 0 when its
/// file arguments match no entry at all, so an exit status alone cannot
/// tell extraction apart from extracting nothing. Whatever was asked for
/// has to be on disk afterwards, or the call failed.
#[test]
fn the_cli_reports_failure_when_it_exits_zero_having_written_nothing() {
    let Ok(cli) = SevenZipCli::detect(None) else {
        eprintln!(
            "skipping the_cli_reports_failure_when_it_exits_zero_having_written_nothing: \
             no 7-Zip CLI on this machine"
        );
        return;
    };

    let dir = scratch_dir("cli-exit-zero-no-output");
    let plain = dir.join("present.txt");
    std::fs::write(&plain, b"present").expect("write the fixture's plaintext");
    let archive = dir.join("plain.zip");
    let built = std::process::Command::new(cli.exe_path())
        .arg("a")
        .arg("-tzip")
        .arg(&archive)
        .arg(&plain)
        .output()
        .expect("run 7-Zip to build the fixture");
    assert!(built.status.success(), "building the fixture failed");

    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).expect("create the destination");

    let error = cli
        .extract_files(&archive, &dest, &["absent.txt".to_string()], None)
        .expect_err("extracting an entry the archive does not hold must fail");

    assert!(
        !dest.join("absent.txt").exists(),
        "control: nothing was written"
    );
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("absent.txt"),
        "the failure must name what never arrived, got: {diagnostic}"
    );
}
