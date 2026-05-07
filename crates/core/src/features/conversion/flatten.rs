//! Flatten nested archives — extract inner archives as sibling folders.
//!
//! When an archive contains other archive files as entries (common in mod packs
//! where each variant is wrapped in its own archive), this operation extracts
//! each inner archive to a sibling folder named after the entry, then removes
//! the original archive file.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::longest_common_prefix;

const ARCHIVE_EXTENSIONS: &[&str] = &["rar", "zip", "7z", "tar", "tgz"];

/// Check if a filename has an archive extension.
pub fn is_archive_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

/// Strip archive extension from a filename.
/// Returns the base name without the archive extension.
pub fn strip_archive_extension(name: &str) -> String {
    let lower = name.to_lowercase();
    for ext in ARCHIVE_EXTENSIONS {
        let suffix = format!(".{}", ext);
        if lower.ends_with(&suffix) {
            return name[..name.len() - suffix.len()].to_string();
        }
    }
    name.to_string()
}

/// Find all archive files in a directory (top-level only).
pub fn find_archive_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut archives = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("Reading {:?}", dir))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if is_archive_filename(name) {
                    archives.push(path);
                }
            }
        }
    }
    Ok(archives)
}

/// Find all archive files in a directory tree (recursive).
/// Used by the pipeline flatten operation because outer archives often
/// extract into a subfolder layout rather than files-at-root.
pub fn find_archive_files_recursive(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut archives = Vec::new();
    walk(dir, &mut archives)?;
    Ok(archives)
}

fn walk(dir: &Path, archives: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Reading {:?}", dir))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if is_archive_filename(name) {
                    archives.push(path);
                }
            }
        } else if path.is_dir() {
            walk(&path, archives)?;
        }
    }
    Ok(())
}

/// Compute the target folder names for a list of archive files.
/// If strip_common_prefix is true, strips the longest common prefix (if meaningful).
///
/// The whole batch reverts to unstripped names if the candidate prefix
/// would produce a broken folder name for any archive — empty, or
/// starting with a non-alphanumeric character (`-`, ` `, `_` etc.).
/// This catches the addon-pack pattern where the parent mod's name
/// itself is the longest common prefix:
///
///   - `ModName v1.0`              → after strip: `v1.0`
///   - `ModName - Variant A v1.0`  → after strip: `- Variant A v1.0`
///   - `ModName - Variant B v1.0`  → after strip: `- Variant B v1.0`
///
/// The parent loses its mod-name identity (folder is just `v1.0`) and
/// the addon folders get leading-dash names. Mod managers that use a
/// `addonfor=ModName` field in modinfo.ini to group addons under their
/// parent then can't resolve the link, because no folder is named
/// `ModName` anymore. Whole-batch abort preserves identity for every
/// row in the pack.
pub fn target_folder_names(archives: &[PathBuf], strip_prefix: bool) -> Vec<(PathBuf, String)> {
    let base_names: Vec<String> = archives
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(strip_archive_extension)
        .collect();

    let prefix = if strip_prefix {
        let refs: Vec<&str> = base_names.iter().map(|s| s.as_str()).collect();
        let candidate = longest_common_prefix(&refs);
        if !candidate.is_empty() && would_produce_broken_names(&base_names, &candidate) {
            // Whole-batch abort: any archive would get a malformed
            // folder name post-strip, so keep originals for everyone.
            String::new()
        } else {
            candidate
        }
    } else {
        String::new()
    };

    archives
        .iter()
        .zip(base_names.iter())
        .map(|(path, base)| {
            let folder_name = if !prefix.is_empty() && base.starts_with(&prefix) {
                let stripped = &base[prefix.len()..];
                if stripped.is_empty() {
                    base.clone()
                } else {
                    stripped.to_string()
                }
            } else {
                base.clone()
            };
            (path.clone(), folder_name)
        })
        .collect()
}

/// Returns true if applying `prefix` as a strip to any of `names` would
/// leave an empty string OR a string starting with a non-alphanumeric
/// character (the "broken folder name" signal — `- X Addon`, ` Extra`,
/// etc.). When this fires the caller aborts prefix-stripping for the
/// whole batch.
fn would_produce_broken_names(names: &[String], prefix: &str) -> bool {
    names.iter().any(|name| {
        let Some(stripped) = name.strip_prefix(prefix) else {
            return false;
        };
        match stripped.chars().next() {
            None => true, // empty after strip
            Some(c) => !c.is_alphanumeric(),
        }
    })
}

/// Summary of a flatten operation.
#[derive(Debug, Default, Clone)]
pub struct FlattenReport {
    pub extracted: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl FlattenReport {
    pub fn total(&self) -> usize {
        self.extracted.len() + self.skipped.len() + self.failed.len()
    }

    pub fn success_count(&self) -> usize {
        self.extracted.len()
    }
}

/// Hard safety caps for recursive flatten. Prevent runaway extraction if an
/// archive contains itself, or if the extracted content is itself an archive
/// that expands into more of the same pattern.
pub const FLATTEN_MAX_ITERATIONS: u32 = 10;
pub const FLATTEN_MAX_TOTAL_EXTRACTIONS: u32 = 1000;

/// Run `flatten_nested_archives` iteratively until the tree stops producing
/// new archives, or until a cap is reached.
///
/// `max_depth` semantics:
/// - `0` — unlimited (still bounded by `FLATTEN_MAX_ITERATIONS` and
///   `FLATTEN_MAX_TOTAL_EXTRACTIONS` for safety)
/// - `1` — single pass (identical to calling `flatten_nested_archives` once)
/// - `n` — up to `n` passes (or until stable)
///
/// Each iteration is a full tree walk; it exits early as soon as a pass
/// produces zero extractions (the tree has stabilized). The returned
/// `FlattenReport` is the union of all iterations' reports.
pub fn flatten_nested_archives_recursive<F>(
    dir: &Path,
    strip_prefix: bool,
    max_depth: u32,
    mut extractor: F,
) -> Result<FlattenReport>
where
    F: FnMut(&Path, &Path) -> Result<()>,
{
    let requested_cap = if max_depth == 0 {
        FLATTEN_MAX_ITERATIONS
    } else {
        max_depth.min(FLATTEN_MAX_ITERATIONS)
    };

    let mut combined = FlattenReport::default();
    let mut total_extractions: u32 = 0;

    for iteration in 0..requested_cap {
        let pass = flatten_nested_archives(dir, strip_prefix, &mut extractor)?;
        let pass_extracted = pass.extracted.len();
        let pass_failed = pass.failed.len();
        let pass_skipped = pass.skipped.len();

        combined.extracted.extend(pass.extracted);
        combined.failed.extend(pass.failed);
        combined.skipped.extend(pass.skipped);

        total_extractions = total_extractions.saturating_add(pass_extracted as u32);

        if total_extractions > FLATTEN_MAX_TOTAL_EXTRACTIONS {
            tracing::warn!(
                "[flatten] Extraction safety cap reached ({} extractions in {} iterations). \
                 Refusing to continue — this archive likely contains a self-referential cycle.",
                total_extractions,
                iteration + 1,
            );
            break;
        }

        if pass_extracted == 0 {
            // Tree is stable — even if this pass saw skipped or failed entries,
            // no new content appeared, so another iteration would be pointless.
            tracing::debug!(
                "[flatten] Tree stabilized after {} iteration(s) (skipped={}, failed={})",
                iteration + 1,
                pass_skipped,
                pass_failed,
            );
            break;
        }
    }

    Ok(combined)
}

/// Extract all nested archives in the given directory tree to sibling folders,
/// then remove the original archive files.
///
/// Walks the whole tree — if the outer archive extracted into `SubFolder/*.rar`,
/// inner archives are still found and flattened to `SubFolder/<name>/`.
///
/// This is a single-pass operation. If an extraction produces a new archive
/// (e.g. `.rar` → folder → `.zip`), that inner archive will NOT be unpacked
/// by this call — use `flatten_nested_archives_recursive` for that.
///
/// `extractor` is a callback that extracts `(archive_path, dest_dir) -> Result<()>`.
pub fn flatten_nested_archives<F>(
    dir: &Path,
    strip_prefix: bool,
    mut extractor: F,
) -> Result<FlattenReport>
where
    F: FnMut(&Path, &Path) -> Result<()>,
{
    let archives = find_archive_files_recursive(dir)?;
    let targets = target_folder_names(&archives, strip_prefix);

    let mut report = FlattenReport::default();

    for (archive_path, folder_name) in targets {
        // Place the output folder next to the archive file, not at tree root
        let dest_parent = archive_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| dir.to_path_buf());
        let dest_folder = dest_parent.join(&folder_name);

        // Skip if destination already exists (avoid overwriting user data)
        if dest_folder.exists() {
            report.skipped.push(folder_name.clone());
            continue;
        }

        fs::create_dir_all(&dest_folder)
            .with_context(|| format!("Creating folder {:?}", dest_folder))?;

        match extractor(&archive_path, &dest_folder) {
            Ok(()) => {
                if let Err(e) = fs::remove_file(&archive_path) {
                    tracing::warn!(
                        "[flatten] Extracted {} but failed to remove original: {}",
                        archive_path.display(),
                        e
                    );
                }

                // If the archive unpacked into a single root folder, promote that folder
                // so mod managers see the archive's own folder name instead of our wrapper.
                let final_name = match unwrap_single_root_folder(&dest_folder) {
                    Ok(Some(promoted)) => promoted,
                    Ok(None) => folder_name,
                    Err(e) => {
                        tracing::warn!(
                            "[flatten] Failed to unwrap single-root for {}: {}",
                            dest_folder.display(),
                            e
                        );
                        folder_name
                    }
                };

                // Prefer modinfo.ini's `name=` field as the folder name
                // when present — that's the value mod managers display
                // and use to resolve `addonfor=` links between addons
                // and their parent mod. Folder-on-disk and display-name
                // stay in sync, and addon packs whose archive-derived
                // names lost the parent-mod identity (`v1.0/`,
                // `- Variant A v1.0/`, etc.) get clean folders matching
                // their modinfo `name=` values.
                let final_path = dest_parent.join(&final_name);
                let final_name = match rename_to_modinfo_name(&final_path) {
                    Ok(Some(renamed)) => renamed,
                    Ok(None) => final_name,
                    Err(e) => {
                        tracing::warn!(
                            "[flatten] modinfo.ini rename failed for {:?}: {}",
                            final_path,
                            e
                        );
                        final_name
                    }
                };
                report.extracted.push(final_name);
            }
            Err(e) => {
                // Clean up the empty destination on failure. If removal
                // fails (typical cause: another process has a file in
                // dest_folder open), surface it in the failure report
                // so the user knows there's leftover state to clean up
                // manually (audit finding M6).
                if let Err(rm_err) = fs::remove_dir_all(&dest_folder) {
                    tracing::warn!(
                        "[flatten] cleanup of {:?} after extract failure failed: {}",
                        dest_folder,
                        rm_err
                    );
                }
                report.failed.push((folder_name, e.to_string()));
            }
        }
    }

    Ok(report)
}

/// Read a `modinfo.ini`-style `name=...` value from a folder, if present.
///
/// Mod managers (Fluffy and friends) ship a `modinfo.ini` next to each
/// mod's content with at minimum a `name=Display Name` line. We prefer
/// that value over the archive-derived folder name because:
///
/// 1. It's the string the mod manager shows in its list, so folder-on-
///    disk and display-name stay in sync.
/// 2. Addon → parent linking via `addonfor=DisplayName` matches against
///    `name=DisplayName` in the parent's modinfo, NOT the parent's
///    folder name. When the archive-derived parent folder is something
///    like `v1.0/` (because `target_folder_names` stripped the common
///    prefix that happened to be the mod's name), `addonfor=` still
///    resolves correctly — but the user can't tell from the disk
///    layout which folder is which. Renaming to the modinfo name fixes
///    the disk layout to match.
///
/// Returns `None` if the file is missing, has no `name=` line, the
/// value is empty, or sanitisation produces an empty string.
fn read_modinfo_name(folder: &Path) -> Option<String> {
    let path = folder.join("modinfo.ini");
    let contents = fs::read_to_string(&path).ok()?;

    for line in contents.lines() {
        let line = line.trim();
        // Skip `[section]` headers and comments.
        if line.is_empty() || line.starts_with('[') || line.starts_with('#')
            || line.starts_with(';')
        {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name=").or_else(|| line.strip_prefix("name = ")) {
            let raw = rest.trim();
            if raw.is_empty() {
                return None;
            }
            let sanitized = sanitize_modinfo_name(raw);
            if sanitized.is_empty() {
                return None;
            }
            return Some(sanitized);
        }
    }
    None
}

/// Strip filesystem-illegal characters from a modinfo `name=` value.
///
/// Windows is the strict platform: `< > : " / \ | ? *` plus control
/// chars are reserved. Trailing `.` and whitespace are also unsafe.
/// We replace illegal chars with `_` rather than dropping them so a
/// `Mod: Subtitle` doesn't collapse two siblings into the same folder.
/// Leading/trailing dots and whitespace get trimmed.
fn sanitize_modinfo_name(name: &str) -> String {
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

/// If `folder` contains a modinfo.ini whose `name=` value differs from
/// the folder's current name, rename the folder to match. No-op when
/// modinfo.ini is missing, the values already agree, or the rename
/// target already exists (a sibling mod with the same modinfo name —
/// keep the archive-derived folder name to avoid clobber).
///
/// Returns `Some(new_name)` on rename, `None` for any no-op path, and
/// an `Err` only on filesystem failure during the rename itself.
fn rename_to_modinfo_name(folder: &Path) -> Result<Option<String>> {
    let mod_name = match read_modinfo_name(folder) {
        Some(n) => n,
        None => return Ok(None),
    };

    let current = folder
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    if current.as_deref() == Some(mod_name.as_str()) {
        return Ok(None);
    }

    let parent = match folder.parent() {
        Some(p) => p,
        None => return Ok(None),
    };
    let target = parent.join(&mod_name);
    if target.exists() {
        // Don't clobber an existing sibling. The archive-derived name
        // stays — modinfo display still works because mod managers
        // read modinfo.ini contents, the folder name is just disk
        // layout.
        tracing::debug!(
            "[flatten] modinfo rename target {:?} already exists; keeping archive-derived name",
            target
        );
        return Ok(None);
    }

    fs::rename(folder, &target)
        .with_context(|| format!("Renaming {:?} to {:?}", folder, target))?;
    Ok(Some(mod_name))
}

/// If `dest_folder` contains exactly one entry and that entry is a directory,
/// promote the inner folder to replace `dest_folder` (using the inner folder's name).
///
/// Returns:
/// - `Ok(Some(name))` when the unwrap happened (name is the inner folder's name)
/// - `Ok(None)` when the layout didn't qualify (multiple entries, only a file, etc.)
/// - `Err(_)` on I/O failure
///
/// Motivation: archives often contain a single root folder like `ModName/...` that
/// already identifies the mod. Our wrapper folder (derived from the archive filename)
/// adds an extra nesting level that breaks mod managers like fluffy which expect
/// the mod folder to be at the top level.
fn unwrap_single_root_folder(dest_folder: &Path) -> Result<Option<String>> {
    let entries: Vec<_> = fs::read_dir(dest_folder)
        .with_context(|| format!("Reading {:?}", dest_folder))?
        .collect::<std::io::Result<Vec<_>>>()?;

    if entries.len() != 1 {
        return Ok(None);
    }
    let only = &entries[0];
    if !only.path().is_dir() {
        return Ok(None);
    }

    let inner_name = match only.file_name().to_str() {
        Some(s) => s.to_string(),
        None => return Ok(None), // skip non-UTF8 names
    };
    let inner_path = only.path();

    let parent = match dest_folder.parent() {
        Some(p) => p,
        None => return Ok(None),
    };
    let final_dest = parent.join(&inner_name);

    // Inner name matches wrapper: must go via a temp name to avoid self-overwrite
    if final_dest == dest_folder {
        let tmp = parent.join(format!(
            ".arclain_flatten_tmp_{}_{}",
            std::process::id(),
            inner_name
        ));
        fs::rename(&inner_path, &tmp)
            .with_context(|| format!("Renaming {:?} to temp", inner_path))?;
        fs::remove_dir(dest_folder)
            .with_context(|| format!("Removing empty wrapper {:?}", dest_folder))?;
        fs::rename(&tmp, &final_dest)
            .with_context(|| format!("Renaming temp to {:?}", final_dest))?;
        return Ok(Some(inner_name));
    }

    // Inner name collides with an unrelated existing path — keep the wrapper to be safe
    if final_dest.exists() {
        return Ok(None);
    }

    fs::rename(&inner_path, &final_dest)
        .with_context(|| format!("Promoting {:?} to {:?}", inner_path, final_dest))?;
    fs::remove_dir(dest_folder)
        .with_context(|| format!("Removing empty wrapper {:?}", dest_folder))?;
    Ok(Some(inner_name))
}

#[cfg(test)]
#[path = "flatten_tests.rs"]
mod tests;
