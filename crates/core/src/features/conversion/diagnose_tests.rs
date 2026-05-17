//! Tests for `diagnose.rs`. Loaded as `diagnose::tests` via `#[path]`
//! from `diagnose.rs`, so `super::*` here is everything in `diagnose.rs`.

use super::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Create a mod folder with given modinfo.ini contents.
fn make_mod(extract_dir: &Path, folder: &str, modinfo: &str) {
    let dir = extract_dir.join(folder);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("modinfo.ini"), modinfo).unwrap();
}

/// Create a folder with no modinfo (silent in checks 1 and 2).
fn make_folder_no_modinfo(extract_dir: &Path, folder: &str) {
    fs::create_dir_all(extract_dir.join(folder)).unwrap();
}

/// Drop bytes at a relative path under a mod folder.
fn drop_file(extract_dir: &Path, folder: &str, rel: &str, bytes: &[u8]) {
    let p = extract_dir.join(folder).join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, bytes).unwrap();
}

// ---- Check 1: missing screenshot ----

#[test]
fn missing_screenshot_emits_warning() {
    let tmp = TempDir::new().unwrap();
    make_mod(tmp.path(), "ModA", "name=ModA\nscreenshot=ModPic.png\n");
    let w = diagnose_mods(tmp.path()).unwrap();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].mod_folder, "ModA");
    assert_eq!(
        w[0].kind,
        WarningKind::MissingScreenshot {
            referenced: "ModPic.png".to_string()
        }
    );
}

#[test]
fn existing_screenshot_no_warning() {
    let tmp = TempDir::new().unwrap();
    make_mod(tmp.path(), "ModA", "name=ModA\nscreenshot=preview.png\n");
    drop_file(tmp.path(), "ModA", "preview.png", b"\x89PNG fake");
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty(), "expected no warnings, got {:?}", w);
}

#[test]
fn missing_screenshot_empty_field_silent() {
    let tmp = TempDir::new().unwrap();
    make_mod(tmp.path(), "ModA", "name=ModA\nscreenshot=\n");
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty());
}

// ---- Check 2: missing addon parent ----

#[test]
fn addon_parent_present_no_warning() {
    let tmp = TempDir::new().unwrap();
    make_mod(tmp.path(), "ParentMod", "name=ParentMod\n");
    make_mod(
        tmp.path(),
        "Addon",
        "name=Addon\naddonfor=ParentMod\n",
    );
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty());
}

#[test]
fn addon_parent_missing_emits_warning() {
    let tmp = TempDir::new().unwrap();
    make_mod(
        tmp.path(),
        "OrphanAddon",
        "name=OrphanAddon\naddonfor=MissingParent\n",
    );
    let w = diagnose_mods(tmp.path()).unwrap();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].mod_folder, "OrphanAddon");
    assert_eq!(
        w[0].kind,
        WarningKind::MissingAddonParent {
            needs: "MissingParent".to_string()
        }
    );
}

#[test]
fn addon_parent_case_insensitive_match() {
    let tmp = TempDir::new().unwrap();
    make_mod(tmp.path(), "ParentMod", "name=ParentMod\n");
    make_mod(
        tmp.path(),
        "Addon",
        "name=Addon\naddonfor=parentmod\n",
    );
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty(), "case-insensitive match should not warn: {:?}", w);
}

#[test]
fn multiple_orphan_addons_one_warning_each() {
    let tmp = TempDir::new().unwrap();
    make_mod(tmp.path(), "AddonA", "name=AddonA\naddonfor=NoParent\n");
    make_mod(tmp.path(), "AddonB", "name=AddonB\naddonfor=NoParent\n");
    make_mod(tmp.path(), "AddonC", "name=AddonC\naddonfor=NoParent\n");
    let w = diagnose_mods(tmp.path()).unwrap();
    assert_eq!(w.len(), 3);
    for warning in &w {
        assert!(matches!(warning.kind, WarningKind::MissingAddonParent { .. }));
    }
}

// ---- Check 3: duplicate preview ----

#[test]
fn duplicate_preview_two_siblings_one_warning() {
    let tmp = TempDir::new().unwrap();
    let bytes: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    make_folder_no_modinfo(tmp.path(), "ModA");
    make_folder_no_modinfo(tmp.path(), "ModB");
    drop_file(tmp.path(), "ModA", "preview.jpg", &bytes);
    drop_file(tmp.path(), "ModB", "preview.jpg", &bytes);
    let w = diagnose_mods(tmp.path()).unwrap();
    assert_eq!(w.len(), 1, "expected 1 dup warning, got {:?}", w);
    // Lex-first folder is anchor; second emits warning pointing back at anchor.
    assert_eq!(w[0].mod_folder, "ModB");
    assert_eq!(
        w[0].kind,
        WarningKind::DuplicatePreview {
            peer_folder: "ModA".to_string(),
            file: "preview.jpg".to_string()
        }
    );
}

#[test]
fn duplicate_preview_three_siblings_two_warnings() {
    let tmp = TempDir::new().unwrap();
    let bytes: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    for folder in ["ModA", "ModB", "ModC"] {
        make_folder_no_modinfo(tmp.path(), folder);
        drop_file(tmp.path(), folder, "preview.jpg", &bytes);
    }
    let w = diagnose_mods(tmp.path()).unwrap();
    assert_eq!(w.len(), 2);
    let folders: Vec<&str> = w.iter().map(|x| x.mod_folder.as_str()).collect();
    assert!(folders.contains(&"ModB"));
    assert!(folders.contains(&"ModC"));
    assert!(!folders.contains(&"ModA"), "anchor should not emit warning");
}

#[test]
fn unique_previews_no_warning() {
    let tmp = TempDir::new().unwrap();
    let bytes_a: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    let bytes_b: Vec<u8> = (0..2048).map(|i| ((i + 1) % 256) as u8).collect();
    make_folder_no_modinfo(tmp.path(), "ModA");
    make_folder_no_modinfo(tmp.path(), "ModB");
    drop_file(tmp.path(), "ModA", "preview.jpg", &bytes_a);
    drop_file(tmp.path(), "ModB", "preview.jpg", &bytes_b);
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty());
}

#[test]
fn dup_preview_skips_tiny_files() {
    let tmp = TempDir::new().unwrap();
    let tiny: Vec<u8> = vec![0u8; 512]; // < 1 KB
    make_folder_no_modinfo(tmp.path(), "ModA");
    make_folder_no_modinfo(tmp.path(), "ModB");
    drop_file(tmp.path(), "ModA", "preview.jpg", &tiny);
    drop_file(tmp.path(), "ModB", "preview.jpg", &tiny);
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty(), "tiny files should be skipped: {:?}", w);
}

#[test]
fn dup_preview_skips_huge_files() {
    let tmp = TempDir::new().unwrap();
    // 51 MB — just over the 50 MB cap.
    let huge: Vec<u8> = vec![0u8; 51 * 1024 * 1024];
    make_folder_no_modinfo(tmp.path(), "ModA");
    make_folder_no_modinfo(tmp.path(), "ModB");
    drop_file(tmp.path(), "ModA", "preview.jpg", &huge);
    drop_file(tmp.path(), "ModB", "preview.jpg", &huge);
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty(), "huge files should be skipped: {:?}", w);
}

// ---- Combinations and corners ----

#[test]
fn mixed_warnings_screenshot_and_addon() {
    let tmp = TempDir::new().unwrap();
    make_mod(
        tmp.path(),
        "BadMod",
        "name=BadMod\naddonfor=MissingParent\nscreenshot=missing.png\n",
    );
    let w = diagnose_mods(tmp.path()).unwrap();
    assert_eq!(w.len(), 2);
    assert!(w.iter().any(|x| matches!(x.kind, WarningKind::MissingScreenshot { .. })));
    assert!(w.iter().any(|x| matches!(x.kind, WarningKind::MissingAddonParent { .. })));
}

#[test]
fn empty_extract_dir_no_warnings() {
    let tmp = TempDir::new().unwrap();
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty());
}

#[test]
fn folder_without_modinfo_silent() {
    let tmp = TempDir::new().unwrap();
    make_folder_no_modinfo(tmp.path(), "PlainFolder");
    let w = diagnose_mods(tmp.path()).unwrap();
    assert!(w.is_empty());
}
