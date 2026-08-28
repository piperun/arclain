//! Parse mod-manager `modinfo.ini` files.
//!
//! Mod managers (Fluffy and friends) ship a `modinfo.ini` next to each
//! mod's content with at minimum a `name=Display Name` line, optionally
//! `addonfor=ParentName` for addon→parent linking, and `screenshot=...`
//! pointing to a preview image relative to the folder.
//!
//! Shared by [`super::flatten`] (which uses `name=` for folder renaming)
//! and [`super::diagnose`] (which uses all three fields for diagnostic
//! checks).

use std::fs;
use std::path::Path;

/// Parsed view of a `modinfo.ini` file.
///
/// Per-field `Option` distinguishes "present but empty" (`None` — value
/// was whitespace or sanitization left nothing) from "absent" (`None` —
/// key not in file). Both collapse to `None`; if downstream code needs
/// to distinguish, the parser would need to return raw strings instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModInfo {
    /// Display name. Sanitized via [`sanitize_modinfo_name`] for safe
    /// filesystem use. `None` when missing/empty/sanitized-to-empty.
    pub name: Option<String>,
    /// Parent mod reference for addon→parent linking. Returned raw
    /// (trimmed only) — used for cross-mod lookups against other mods'
    /// `name=` values, so preserving the modder's spelling matters.
    pub addonfor: Option<String>,
    /// Relative path to a preview image. Leading `./` is stripped.
    /// Returned raw otherwise.
    pub screenshot: Option<String>,
}

/// Parse a `modinfo.ini` that is already in memory.
///
/// Split from [`parse`] so a caller holding the file's bytes — layout
/// resolution reads entries out of an archive, not off a disk — can use
/// the same parser rather than a second copy of these rules.
pub fn parse_str(contents: &str) -> ModInfo {
    let mut info = ModInfo::default();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('[')
            || line.starts_with('#')
            || line.starts_with(';')
        {
            continue;
        }
        if let Some(rest) = take_value(line, "name") {
            let sanitized = sanitize_modinfo_name(rest);
            if !sanitized.is_empty() {
                info.name = Some(sanitized);
            }
        } else if let Some(rest) = take_value(line, "addonfor") {
            if !rest.is_empty() {
                info.addonfor = Some(rest.to_string());
            }
        } else if let Some(rest) = take_value(line, "screenshot") {
            if !rest.is_empty() {
                let stripped = rest.strip_prefix("./").unwrap_or(rest);
                if !stripped.is_empty() {
                    info.screenshot = Some(stripped.to_string());
                }
            }
        }
    }

    info
}

/// Parse a folder's `modinfo.ini` if present.
///
/// Returns `None` when the file is missing or unreadable. Returns
/// `Some(ModInfo)` with per-field `Option`s for present-but-empty vs
/// missing keys.
pub fn parse(folder: &Path) -> Option<ModInfo> {
    let contents = fs::read_to_string(folder.join("modinfo.ini")).ok()?;
    Some(parse_str(&contents))
}

/// Try to extract `value` from a `key=value` (or `key = value`) line.
/// Returns `None` if the line doesn't start with the key. Returned
/// value is trimmed of surrounding whitespace.
fn take_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let with_eq = format!("{}=", key);
    let with_pad = format!("{} = ", key);
    if let Some(rest) = line.strip_prefix(&with_pad) {
        return Some(rest.trim());
    }
    if let Some(rest) = line.strip_prefix(&with_eq) {
        return Some(rest.trim());
    }
    None
}

/// Strip filesystem-illegal characters from a modinfo `name=` value.
///
/// Windows is the strict platform: `< > : " / \ | ? *` plus control
/// chars are reserved. Trailing `.` and whitespace are also unsafe.
/// Illegal chars become `_` rather than being dropped so a `Mod: Sub`
/// doesn't collapse two siblings into the same folder. Leading and
/// trailing dots / whitespace get trimmed.
pub(crate) fn sanitize_modinfo_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    mapped
        .trim()
        .trim_end_matches('.')
        .trim_start_matches('.')
        .to_string()
}

#[cfg(test)]
#[path = "modinfo_tests.rs"]
mod tests;
