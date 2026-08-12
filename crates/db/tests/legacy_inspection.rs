use std::fs;
use std::time::{Duration, Instant};

use arclain_db::{lock_and_inspect_legacy_socks5_password, LegacyInspectionErrorKind, SecretsDb};

const TEST_KEY: [u8; 32] = [0x42; 32];

fn seed_secrets(path: &std::path::Path, socks5_password: Option<&str>) {
    let db = SecretsDb::open(path, &TEST_KEY).expect("create legacy secrets database");
    if let Some(password) = socks5_password {
        db.set_secret("proxy:socks5", password)
            .expect("seed SOCKS5 password");
    }
    db.set_secret("unrelated", "must-never-be-returned")
        .expect("seed unrelated secret");
    db.close();
}

#[test]
fn fixed_socks5_presence_is_read_without_changing_the_source() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pass.redb");
    seed_secrets(&path, Some("never expose this value"));
    let before = fs::read(&path).unwrap();

    let lease = lock_and_inspect_legacy_socks5_password(&path)
        .expect("inspect legacy secrets")
        .expect("existing secrets file returns a lease");

    assert!(lease.socks5_password_configured());
    lease.finish().expect("source stayed unchanged");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn missing_file_and_missing_fixed_key_are_not_errors() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing").join("pass.redb");
    assert!(lock_and_inspect_legacy_socks5_password(&missing)
        .expect("missing file is absence")
        .is_none());
    assert!(!missing.parent().unwrap().exists());

    let path = temp.path().join("pass.redb");
    seed_secrets(&path, None);
    let before = fs::read(&path).unwrap();
    let lease = lock_and_inspect_legacy_socks5_password(&path)
        .expect("inspect database without fixed key")
        .expect("existing database returns a lease");
    assert!(!lease.socks5_password_configured());
    lease.finish().expect("source stayed unchanged");
    assert_eq!(fs::read(&path).unwrap(), before);

    let no_table = temp.path().join("no-metadata-table.redb");
    let database = redb::Database::create(&no_table).expect("create redb without metadata table");
    let write = database.begin_write().unwrap();
    {
        let _: redb::Table<'_, u32, u32> = write
            .open_table(redb::TableDefinition::new("unrelated"))
            .unwrap();
    }
    write.commit().unwrap();
    drop(database);
    let before = fs::read(&no_table).unwrap();
    let lease = lock_and_inspect_legacy_socks5_password(&no_table)
        .expect("inspect database without metadata table")
        .expect("existing database returns a lease");
    assert!(!lease.socks5_password_configured());
    lease.finish().expect("source stayed unchanged");
    assert_eq!(fs::read(&no_table).unwrap(), before);
}

#[test]
fn active_redb_owner_fails_busy_within_the_bounded_retry_window() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pass.redb");
    let _owner = SecretsDb::open(&path, &TEST_KEY).expect("hold redb source lock");
    let started = Instant::now();

    let error = match lock_and_inspect_legacy_socks5_password(&path) {
        Err(error) => error,
        Ok(_) => panic!("an active owner must not be inspected"),
    };

    assert_eq!(error.kind(), LegacyInspectionErrorKind::Busy);
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn corrupt_and_oversized_sources_fail_as_bounded_backend_errors() {
    let temp = tempfile::tempdir().unwrap();
    let corrupt = temp.path().join("corrupt.redb");
    fs::write(&corrupt, b"not a redb database").unwrap();
    let corrupt_before = fs::read(&corrupt).unwrap();
    let corrupt_error = match lock_and_inspect_legacy_socks5_password(&corrupt) {
        Err(error) => error,
        Ok(_) => panic!("corrupt source must fail"),
    };
    assert_eq!(corrupt_error.kind(), LegacyInspectionErrorKind::Backend);
    assert!(corrupt_error.to_string().len() <= 512);
    assert_eq!(fs::read(&corrupt).unwrap(), corrupt_before);

    let oversized = temp.path().join("oversized.redb");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    let oversized_error = match lock_and_inspect_legacy_socks5_password(&oversized) {
        Err(error) => error,
        Ok(_) => panic!("oversized source must fail"),
    };
    assert_eq!(oversized_error.kind(), LegacyInspectionErrorKind::Backend);
    assert!(oversized_error.to_string().len() <= 512);
}

#[cfg(unix)]
#[test]
fn symlink_source_is_rejected_without_following_it() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.redb");
    seed_secrets(&target, Some("secret"));
    let link = temp.path().join("pass.redb");
    symlink(&target, &link).unwrap();

    let error = match lock_and_inspect_legacy_socks5_password(&link) {
        Err(error) => error,
        Ok(_) => panic!("symbolic links must not be followed"),
    };

    assert_eq!(error.kind(), LegacyInspectionErrorKind::PermissionDenied);
}
