//! Read-only diagnostics for the conversion flatten output.
//!
//! After [`super::flatten::flatten_nested_archives`] extracts a meta-
//! archive into per-mod sibling folders, this module walks the output
//! and emits warnings for source-archive quality issues the modder
//! introduced — missing screenshot files, addon mods whose parent
//! isn't present, and sibling mods sharing byte-identical preview
//! images.
//!
//! Strict read-only: no file mutation, no recursive descent, no
//! network access. Top-level folders only.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::modinfo::{self, ModInfo};

/// Filenames considered "preview images" for duplicate detection.
/// Matched case-insensitively against each folder's direct entries.
const PREVIEW_FILENAMES: &[&str] = &["preview.jpg", "preview.png", "cover.jpg", "cover.png"];

/// Minimum file size to hash for duplicate detection. Anything smaller
/// is almost certainly an error placeholder; hashing it yields noise.
const PREVIEW_MIN_BYTES: u64 = 1024;

/// Maximum file size to hash. Preview images shouldn't be this large;
/// if they are, that's its own modder problem and not worth our IO.
const PREVIEW_MAX_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModWarning {
    pub mod_folder: String,
    pub kind: WarningKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    /// modinfo.ini references a `screenshot=` file that isn't on disk.
    MissingScreenshot { referenced: String },
    /// modinfo.ini has `addonfor=Parent` but no sibling mod has
    /// `name=Parent` (case-insensitive comparison).
    MissingAddonParent { needs: String },
    /// Two sibling mods ship byte-identical preview/cover files.
    /// `peer_folder` is the lex-first folder in the dup set (anchor);
    /// the warning is emitted on every other member.
    DuplicatePreview { peer_folder: String, file: String },
}

impl WarningKind {
    pub fn human(&self) -> String {
        match self {
            Self::MissingScreenshot { referenced } => format!(
                "modinfo references screenshot {:?} but file is missing",
                referenced
            ),
            Self::MissingAddonParent { needs } => format!(
                "modinfo addonfor={:?} but no sibling mod has that name",
                needs
            ),
            Self::DuplicatePreview { peer_folder, file } => {
                format!("{} byte-identical to sibling {:?}", file, peer_folder)
            }
        }
    }
}

/// Walk the post-flatten extract directory and emit diagnostics.
///
/// Returns an empty `Vec` for an empty or non-existent extract dir.
/// Returns `Err` only on filesystem failures during directory walks.
pub fn diagnose_mods(extract_dir: &Path) -> Result<Vec<ModWarning>> {
    let folders = top_level_dirs(extract_dir)?;
    let infos: BTreeMap<String, ModInfo> = folders
        .iter()
        .filter_map(|f| modinfo::parse(&extract_dir.join(f)).map(|mi| (f.clone(), mi)))
        .collect();

    let mut warnings = Vec::new();
    check_missing_screenshots(extract_dir, &infos, &mut warnings);
    check_missing_addon_parents(&infos, &mut warnings);
    check_duplicate_previews(extract_dir, &folders, &mut warnings)?;
    Ok(warnings)
}

/// Return sorted top-level directory names under `extract_dir`.
/// Non-UTF8 names are skipped. Returns `Ok(vec![])` if `extract_dir`
/// doesn't exist (matches the "empty" diagnose contract).
fn top_level_dirs(extract_dir: &Path) -> Result<Vec<String>> {
    if !extract_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(extract_dir).with_context(|| format!("Reading {:?}", extract_dir))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            out.push(name.to_string());
        }
    }
    out.sort();
    Ok(out)
}

fn check_missing_screenshots(
    extract_dir: &Path,
    infos: &BTreeMap<String, ModInfo>,
    out: &mut Vec<ModWarning>,
) {
    for (folder, info) in infos {
        let Some(rel) = info.screenshot.as_deref() else {
            continue;
        };
        let resolved = extract_dir.join(folder).join(rel);
        if !resolved.exists() {
            out.push(ModWarning {
                mod_folder: folder.clone(),
                kind: WarningKind::MissingScreenshot {
                    referenced: rel.to_string(),
                },
            });
        }
    }
}

fn check_missing_addon_parents(infos: &BTreeMap<String, ModInfo>, out: &mut Vec<ModWarning>) {
    let name_set: HashSet<String> = infos
        .values()
        .filter_map(|mi| mi.name.as_deref().map(str::to_lowercase))
        .collect();

    for (folder, info) in infos {
        let Some(parent) = info.addonfor.as_deref() else {
            continue;
        };
        let needle = parent.trim().to_lowercase();
        if needle.is_empty() {
            continue;
        }
        if !name_set.contains(&needle) {
            out.push(ModWarning {
                mod_folder: folder.clone(),
                kind: WarningKind::MissingAddonParent {
                    needs: parent.to_string(),
                },
            });
        }
    }
}

fn check_duplicate_previews(
    extract_dir: &Path,
    folders: &[String],
    out: &mut Vec<ModWarning>,
) -> Result<()> {
    // Key: (lowercased filename, sha256). Value: folders that share it.
    let mut by_hash: HashMap<(String, [u8; 32]), Vec<String>> = HashMap::new();

    for folder in folders {
        let folder_path = extract_dir.join(folder);
        for filename in find_preview_files(&folder_path)? {
            let path = folder_path.join(&filename);
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let len = meta.len();
            if len < PREVIEW_MIN_BYTES || len > PREVIEW_MAX_BYTES {
                continue;
            }
            let hash = match sha256_file(&path) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let key = (filename.to_lowercase(), hash);
            by_hash.entry(key).or_default().push(folder.clone());
        }
    }

    for ((filename_lower, _hash), mut folders_in_set) in by_hash {
        if folders_in_set.len() < 2 {
            continue;
        }
        folders_in_set.sort();
        let anchor = folders_in_set[0].clone();
        for folder in folders_in_set.into_iter().skip(1) {
            out.push(ModWarning {
                mod_folder: folder,
                kind: WarningKind::DuplicatePreview {
                    peer_folder: anchor.clone(),
                    file: filename_lower.clone(),
                },
            });
        }
    }

    Ok(())
}

/// Find direct-entry filenames in `folder` whose lowercased name
/// matches any of `PREVIEW_FILENAMES`. Returned filenames preserve
/// the on-disk casing (used to construct paths back to the file).
fn find_preview_files(folder: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let read = match fs::read_dir(folder) {
        Ok(r) => r,
        Err(_) => return Ok(out),
    };
    for entry in read {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let lower = name.to_lowercase();
        if PREVIEW_FILENAMES.iter().any(|f| *f == lower) {
            out.push(name);
        }
    }
    Ok(out)
}

fn sha256_file(path: &Path) -> Result<[u8; 32]> {
    use std::io::Read;
    let mut file = fs::File::open(path).with_context(|| format!("Opening {:?}", path))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
#[path = "diagnose_tests.rs"]
mod tests;
