//! Integration tests for archive conversion utilities.

use arclain_core::features::conversion::flatten::{
    find_archive_files, flatten_nested_archives, is_archive_filename,
};
use std::fs;

#[test]
fn flatten_integration_two_archives_with_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("MyMod - Main.rar"), b"").unwrap();
    fs::write(tmp.path().join("MyMod - Patch A.rar"), b"").unwrap();
    fs::write(tmp.path().join("readme.txt"), b"").unwrap();

    let archives = find_archive_files(tmp.path()).unwrap();
    assert_eq!(archives.len(), 2);

    let report = flatten_nested_archives(tmp.path(), true, |_src, dst| {
        fs::write(dst.join("natives"), b"")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(report.extracted.len(), 2);
    // With prefix stripping: "MyMod - " → "" leaves "Main" and "Patch A"
    assert!(tmp.path().join("Main").join("natives").exists());
    assert!(tmp.path().join("Patch A").join("natives").exists());
    // Originals should be gone
    assert!(!tmp.path().join("MyMod - Main.rar").exists());
    assert!(!tmp.path().join("MyMod - Patch A.rar").exists());
    // Non-archive files are left alone
    assert!(tmp.path().join("readme.txt").exists());
}

#[test]
fn flatten_without_prefix_strip_keeps_full_names() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("A - Main.rar"), b"").unwrap();
    fs::write(tmp.path().join("A - Extra.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_src, dst| {
        fs::write(dst.join("marker"), b"")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(report.extracted.len(), 2);
    assert!(tmp.path().join("A - Main").exists());
    assert!(tmp.path().join("A - Extra").exists());
}

#[test]
fn flatten_skips_when_destination_exists() {
    let tmp = tempfile::tempdir().unwrap();
    // Pre-create a folder that matches what flatten would want to create
    fs::create_dir(tmp.path().join("existing")).unwrap();
    fs::write(tmp.path().join("existing.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_, _| Ok(())).unwrap();

    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0], "existing");
    // Original archive NOT removed when skipped
    assert!(tmp.path().join("existing.rar").exists());
}

#[test]
fn is_archive_recognizes_common_extensions() {
    assert!(is_archive_filename("mod.rar"));
    assert!(is_archive_filename("mod.RAR"));
    assert!(is_archive_filename("mod.Zip"));
    assert!(is_archive_filename("mod.7z"));
    assert!(!is_archive_filename("mod.pak"));
    assert!(!is_archive_filename("readme.txt"));
}
