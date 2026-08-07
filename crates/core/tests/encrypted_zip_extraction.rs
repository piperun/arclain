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
//! The chain has two shapes depending on the machine, and both are
//! covered here: with a 7-Zip CLI installed the native ZIP tier hands off
//! to it, and without one the native tier is the whole chain and has to
//! name the problem by itself. The fixture is built with the `zip` crate
//! rather than a CLI precisely so the second case needs no 7-Zip to set
//! itself up.

use arclain_core::backends::selector::BackendSelector;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::ArchiveBackend;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const FIXTURE_PASSWORD: &str = "SECRET";
const FIXTURE_ENTRY: &str = "secret.txt";
const FIXTURE_CONTENTS: &[u8] = b"classified payload";

/// The phrase the application layer looks for when deciding whether an
/// extraction failure deserves a password prompt. Asserted here rather
/// than a vaguer "mentions a password" so that rewording the backend's
/// refusal out of the set it recognizes fails this test instead of
/// silently removing the prompt.
const RECOGNIZED_PASSWORD_PHRASE: &str = "Password for encrypted archive not specified";

/// Serializes every test here, because one of them replaces `PATH`.
/// `std::env::set_var` is process-global while cargo runs this binary's
/// tests on parallel threads: two concurrent swaps could restore each
/// other's emptied value, and a test that merely *reads* `PATH` (any
/// `SevenZipCli::detect` call) could observe another's empty one.
static PATH_LOCK: Mutex<()> = Mutex::new(());

fn lock_path() -> MutexGuard<'static, ()> {
    // A failed assert poisons the lock but breaks no shared state --
    // `EmptyPath` restores `PATH` while unwinding -- so a poisoned lock
    // is not a reason to fail every later test too.
    PATH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Points `PATH` at an empty directory for as long as it is alive and
/// restores the original on drop, including while unwinding from a failed
/// assert. Total isolation for 7-Zip specifically: `SevenZipCli::detect`
/// resolves through a `which` lookup over `PATH` and nothing else.
struct EmptyPath {
    _guard: MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    original: Option<OsString>,
}

impl EmptyPath {
    fn set(guard: MutexGuard<'static, ()>) -> Self {
        let dir = tempfile::tempdir().expect("create an empty directory to point PATH at");
        let original = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        Self {
            _guard: guard,
            _dir: dir,
            original,
        }
    }
}

impl Drop for EmptyPath {
    fn drop(&mut self) {
        match self.original.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

/// Scratch space under the crate's own checkout rather than the system
/// temp directory: on a machine where that resolves to a RAM disk, a real
/// 7-Zip child process's writes there have raced this suite's own
/// filesystem checks before.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-scratch")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test's scratch directory");
    dir
}

/// Builds a ZIP whose entry contents are AES-encrypted but whose central
/// directory is not -- so it lists without a password and only fails when
/// something tries to read the entry.
fn build_content_encrypted_zip(dir: &Path) -> PathBuf {
    use std::io::Write as _;

    let path = dir.join("content-encrypted.zip");
    let file = std::fs::File::create(&path).expect("create the encrypted fixture");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .with_aes_encryption(zip::AesMode::Aes256, FIXTURE_PASSWORD);
    writer
        .start_file(FIXTURE_ENTRY, options)
        .expect("start the encrypted entry");
    writer
        .write_all(FIXTURE_CONTENTS)
        .expect("write the encrypted entry");
    writer.finish().expect("finish the encrypted fixture");
    path
}

/// Lists the fixture and confirms the backend chain sees the entry as
/// encrypted -- the control every assertion below depends on.
fn assert_lists_as_encrypted(backend: &dyn ArchiveBackend, archive: &Path) {
    let info = backend.list(archive, None).expect("the fixture lists");
    let entry = info
        .entries
        .iter()
        .find(|entry| entry.path == FIXTURE_ENTRY)
        .expect("the fixture's entry is listed");
    assert!(entry.encrypted, "control: the entry must be encrypted");
}

fn assert_named_the_password(error: &anyhow::Error, dest: &Path) {
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains(RECOGNIZED_PASSWORD_PHRASE),
        "the failure must name password protection in the wording the \
         application layer acts on, got: {diagnostic}"
    );
    let output = dest.join(FIXTURE_ENTRY);
    assert!(
        !output.exists() || std::fs::read(&output).unwrap() != FIXTURE_CONTENTS,
        "nothing decrypted may be written without the password"
    );
}

/// The full chain: the native ZIP tier refuses, the 7-Zip CLI tier tries
/// and fails too, and the caller gets one failure naming the cause.
/// Reporting success instead writes nothing, and the caller then trips
/// over a missing file with no idea a password was ever involved.
#[test]
fn extracting_an_encrypted_entry_without_a_password_fails_naming_the_password() {
    let guard = lock_path();
    if SevenZipCli::detect(None).is_err() {
        eprintln!(
            "skipping extracting_an_encrypted_entry_without_a_password_fails_naming_the_password: \
             no 7-Zip CLI on this machine (the no-CLI chain is covered unconditionally by \
             extracting_an_encrypted_entry_names_the_password_with_no_sevenzip_at_all)"
        );
        return;
    }

    let dir = scratch_dir("encrypted-entry-extraction");
    let archive = build_content_encrypted_zip(&dir);
    let backend = BackendSelector::new_native()
        .select(&archive)
        .expect("a zip selects a backend");
    assert_lists_as_encrypted(backend.as_ref(), &archive);

    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).expect("create the destination");

    let error = backend
        .extract_files_with_progress(
            &archive,
            &dest,
            &[FIXTURE_ENTRY.to_string()],
            None,
            None,
            None,
        )
        .expect_err("extracting an entry this chain cannot decrypt must fail");

    assert_named_the_password(&error, &dest);
    drop(guard);
}

/// The same archive on a machine with no 7-Zip anywhere, which is a
/// supported configuration: the selector hands back the native tier
/// alone, so that tier's own refusal is the only wording the caller ever
/// sees. If it does not name the password, nothing downstream can tell a
/// password problem from any other failure, and the user is asked for
/// nothing.
///
/// Actively exercised rather than skipped -- the fixture is built by the
/// `zip` crate, so this test needs no 7-Zip to set up, and `PATH` is
/// emptied for its duration so none can be found.
#[test]
fn extracting_an_encrypted_entry_names_the_password_with_no_sevenzip_at_all() {
    let dir = scratch_dir("encrypted-entry-extraction-no-cli");
    let archive = build_content_encrypted_zip(&dir);

    let _path = EmptyPath::set(lock_path());
    assert!(
        SevenZipCli::detect(None).is_err(),
        "control: 7-Zip must be undetectable for this test to mean anything"
    );

    let backend = BackendSelector::new_native()
        .select(&archive)
        .expect("a zip still selects a backend with no 7-Zip present");
    assert_lists_as_encrypted(backend.as_ref(), &archive);

    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).expect("create the destination");

    let error = backend
        .extract_files_with_progress(
            &archive,
            &dest,
            &[FIXTURE_ENTRY.to_string()],
            None,
            None,
            None,
        )
        .expect_err("the native tier alone must refuse an entry it cannot decrypt");

    assert_named_the_password(&error, &dest);
}

/// The 7-Zip CLI answers "Everything is Ok" with exit code 0 when its
/// file arguments match no entry at all, so an exit status alone cannot
/// tell extraction apart from extracting nothing. Whatever was asked for
/// has to be on disk afterwards, or the call failed.
#[test]
fn the_cli_reports_failure_when_it_exits_zero_having_written_nothing() {
    let guard = lock_path();
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
    drop(guard);
}
