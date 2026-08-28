//! Tests for `modinfo.rs`. Loaded as `modinfo::tests` via `#[path]`
//! from `modinfo.rs`, so `super::*` here is everything in `modinfo.rs`.

use super::*;
use std::fs;
use tempfile::TempDir;

fn write_modinfo(dir: &std::path::Path, contents: &str) {
    fs::write(dir.join("modinfo.ini"), contents).unwrap();
}

#[test]
fn parse_happy_path() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(
        tmp.path(),
        "name=TestMod\naddonfor=ParentMod\nscreenshot=preview.png\n",
    );
    let info = parse(tmp.path()).unwrap();
    assert_eq!(info.name.as_deref(), Some("TestMod"));
    assert_eq!(info.addonfor.as_deref(), Some("ParentMod"));
    assert_eq!(info.screenshot.as_deref(), Some("preview.png"));
}

#[test]
fn parse_name_only() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(tmp.path(), "name=JustAName\n");
    let info = parse(tmp.path()).unwrap();
    assert_eq!(info.name.as_deref(), Some("JustAName"));
    assert!(info.addonfor.is_none());
    assert!(info.screenshot.is_none());
}

#[test]
fn parse_no_modinfo_file() {
    let tmp = TempDir::new().unwrap();
    assert!(parse(tmp.path()).is_none());
}

#[test]
fn parse_empty_modinfo() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(tmp.path(), "");
    let info = parse(tmp.path()).unwrap();
    assert!(info.name.is_none());
    assert!(info.addonfor.is_none());
    assert!(info.screenshot.is_none());
}

#[test]
fn parse_with_section_headers() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(tmp.path(), "[Mod]\nname=Sectioned\n[Other]\n");
    let info = parse(tmp.path()).unwrap();
    assert_eq!(info.name.as_deref(), Some("Sectioned"));
}

#[test]
fn parse_with_comments() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(
        tmp.path(),
        "# leading hash comment\n; leading semicolon comment\nname=Commented\n",
    );
    let info = parse(tmp.path()).unwrap();
    assert_eq!(info.name.as_deref(), Some("Commented"));
}

#[test]
fn parse_name_sanitization() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(tmp.path(), "name=Mod: Sub/Title\n");
    let info = parse(tmp.path()).unwrap();
    // Colon and slash are filesystem-illegal on Windows; mapped to '_'.
    assert_eq!(info.name.as_deref(), Some("Mod_ Sub_Title"));
}

#[test]
fn parse_screenshot_with_leading_dotslash() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(tmp.path(), "name=N\nscreenshot=./preview.png\n");
    let info = parse(tmp.path()).unwrap();
    assert_eq!(info.screenshot.as_deref(), Some("preview.png"));
}

#[test]
fn parse_whitespace_handling() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(tmp.path(), "name = SpacedOut\naddonfor=  Padded  \n");
    let info = parse(tmp.path()).unwrap();
    assert_eq!(info.name.as_deref(), Some("SpacedOut"));
    assert_eq!(info.addonfor.as_deref(), Some("Padded"));
}

#[test]
fn parse_addonfor_preserves_case() {
    let tmp = TempDir::new().unwrap();
    write_modinfo(
        tmp.path(),
        "name=Child\naddonfor=Parent: With/Illegal Chars\n",
    );
    let info = parse(tmp.path()).unwrap();
    // addonfor is NOT sanitized — used for raw lookup against name= values.
    assert_eq!(info.addonfor.as_deref(), Some("Parent: With/Illegal Chars"));
}

#[test]
fn a_string_parses_without_touching_a_filesystem() {
    let parsed = parse_str("name = Placeholder Mod\naddonfor=Parent Mod\n; a comment\n");
    assert_eq!(parsed.name.as_deref(), Some("Placeholder Mod"));
    assert_eq!(parsed.addonfor.as_deref(), Some("Parent Mod"));
    assert_eq!(parsed.screenshot, None);
}
