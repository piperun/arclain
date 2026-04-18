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
pub fn target_folder_names(archives: &[PathBuf], strip_prefix: bool) -> Vec<(PathBuf, String)> {
    let base_names: Vec<String> = archives
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(strip_archive_extension)
        .collect();

    let prefix = if strip_prefix {
        let refs: Vec<&str> = base_names.iter().map(|s| s.as_str()).collect();
        longest_common_prefix(&refs)
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

/// Extract all nested archives in the given directory tree to sibling folders,
/// then remove the original archive files.
///
/// Walks the whole tree — if the outer archive extracted into `SubFolder/*.rar`,
/// inner archives are still found and flattened to `SubFolder/<name>/`.
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
                report.extracted.push(final_name);
            }
            Err(e) => {
                // Clean up the empty destination on failure
                let _ = fs::remove_dir_all(&dest_folder);
                report.failed.push((folder_name, e.to_string()));
            }
        }
    }

    Ok(report)
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
mod tests {
    use super::*;

    #[test]
    fn test_is_archive_filename() {
        assert!(is_archive_filename("mod.rar"));
        assert!(is_archive_filename("mod.RAR"));
        assert!(is_archive_filename("mod.zip"));
        assert!(is_archive_filename("mod.7z"));
        assert!(is_archive_filename("Something - Patch Main.rar"));
        assert!(!is_archive_filename("mod.pak"));
        assert!(!is_archive_filename("readme.txt"));
        assert!(!is_archive_filename("no_extension"));
    }

    #[test]
    fn test_strip_archive_extension() {
        assert_eq!(strip_archive_extension("mod.rar"), "mod");
        assert_eq!(strip_archive_extension("Patch.Main.zip"), "Patch.Main");
        assert_eq!(strip_archive_extension("mod.RAR"), "mod");
        assert_eq!(strip_archive_extension("not_archive.pak"), "not_archive.pak");
    }

    #[test]
    fn test_target_folder_names_no_prefix_strip() {
        let paths = vec![
            PathBuf::from("/tmp/AG - Silver - Main.rar"),
            PathBuf::from("/tmp/AG - Silver - Patch A.rar"),
        ];
        let result = target_folder_names(&paths, false);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].1, "AG - Silver - Main");
        assert_eq!(result[1].1, "AG - Silver - Patch A");
    }

    #[test]
    fn test_target_folder_names_with_prefix_strip() {
        let paths = vec![
            PathBuf::from("/tmp/AG - LK - Silver Linning Lingerie - Main.rar"),
            PathBuf::from("/tmp/AG - LK - Silver Linning Lingerie - Patch Makeup.rar"),
            PathBuf::from("/tmp/AG - LK - Silver Linning Lingerie - Patch No Clothes.rar"),
        ];
        let result = target_folder_names(&paths, true);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].1, "Main");
        assert_eq!(result[1].1, "Patch Makeup");
        assert_eq!(result[2].1, "Patch No Clothes");
    }

    #[test]
    fn test_target_folder_names_prefix_would_empty_name() {
        let paths = vec![
            PathBuf::from("/tmp/MyModName.rar"),
            PathBuf::from("/tmp/MyModName Extra.rar"),
        ];
        let result = target_folder_names(&paths, true);
        assert_eq!(result[0].1, "MyModName"); // kept original (would be empty)
        assert_eq!(result[1].1, " Extra");
    }

    #[test]
    fn test_find_archive_files_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_archive_files(tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_archive_files_mixed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mod.rar"), b"").unwrap();
        std::fs::write(tmp.path().join("data.zip"), b"").unwrap();
        std::fs::write(tmp.path().join("readme.txt"), b"").unwrap();
        std::fs::write(tmp.path().join("game.pak"), b"").unwrap();

        let result = find_archive_files(tmp.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_flatten_extracts_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let inner_rar = tmp.path().join("inner.rar");
        std::fs::write(&inner_rar, b"fake archive").unwrap();

        let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
            std::fs::write(dest.join("extracted.txt"), b"ok")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(report.extracted.len(), 1);
        assert_eq!(report.extracted[0], "inner");
        assert!(tmp.path().join("inner").join("extracted.txt").exists());
        assert!(!inner_rar.exists());
    }

    #[test]
    fn test_flatten_handles_extraction_failure() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), false, |_, _| {
            Err(anyhow::anyhow!("extraction failed"))
        })
        .unwrap();

        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "bad");
        assert!(!tmp.path().join("bad").exists());
        assert!(tmp.path().join("bad.rar").exists());
    }

    #[test]
    fn test_flatten_with_prefix_strip() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("MyPack - Main.rar"), b"").unwrap();
        std::fs::write(tmp.path().join("MyPack - Variant A.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), true, |_archive, dest| {
            std::fs::write(dest.join("marker"), b"")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(report.extracted.len(), 2);
        assert!(tmp.path().join("Main").exists());
        assert!(tmp.path().join("Variant A").exists());
    }

    #[test]
    fn test_flatten_unwraps_single_root_folder() {
        // Archive contains its own root folder matching the real mod name —
        // the wrapper from strip_common_prefix should be promoted away so
        // mod managers see the mod folder at the top.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Pack - Main.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), true, |_archive, dest| {
            // Simulate an archive that expands to `dest/ModName/...`
            let inner = dest.join("ModName");
            std::fs::create_dir(&inner)?;
            std::fs::write(inner.join("mod.dll"), b"")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(report.extracted, vec!["ModName".to_string()]);
        // "Main" wrapper gone, "ModName" promoted next to the (now-removed) archive
        assert!(tmp.path().join("ModName/mod.dll").exists());
        assert!(!tmp.path().join("Main").exists());
    }

    #[test]
    fn test_flatten_keeps_wrapper_when_multiple_roots() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pack.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
            std::fs::create_dir(dest.join("folder_a"))?;
            std::fs::write(dest.join("loose.txt"), b"")?;
            Ok(())
        })
        .unwrap();

        // Multiple entries — wrapper stays
        assert_eq!(report.extracted, vec!["pack".to_string()]);
        assert!(tmp.path().join("pack/folder_a").exists());
        assert!(tmp.path().join("pack/loose.txt").exists());
    }

    #[test]
    fn test_flatten_keeps_wrapper_when_single_file_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pack.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
            std::fs::write(dest.join("only_file.txt"), b"")?;
            Ok(())
        })
        .unwrap();

        // Single entry but it's a file, not a folder — wrapper stays
        assert_eq!(report.extracted, vec!["pack".to_string()]);
        assert!(tmp.path().join("pack/only_file.txt").exists());
    }

    #[test]
    fn test_flatten_unwrap_when_inner_name_matches_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Main.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
            let inner = dest.join("Main");
            std::fs::create_dir(&inner)?;
            std::fs::write(inner.join("a.txt"), b"")?;
            Ok(())
        })
        .unwrap();

        // Wrapper and inner happen to share the name "Main" — unwrap still succeeds
        assert_eq!(report.extracted, vec!["Main".to_string()]);
        assert!(tmp.path().join("Main/a.txt").exists());
        // No leftover temp files or double-nesting
        assert!(!tmp.path().join("Main/Main").exists());
    }

    #[test]
    fn test_flatten_finds_archives_in_subfolders() {
        // Regression: outer archive extracts to a subfolder layout,
        // inner archives must still be found and flattened next to them.
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("PackRoot");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("Main.rar"), b"").unwrap();
        std::fs::write(sub.join("Patch.rar"), b"").unwrap();

        let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
            std::fs::write(dest.join("marker"), b"")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(report.extracted.len(), 2);
        // Output folders should sit next to their source archive, not at root
        assert!(sub.join("Main/marker").exists());
        assert!(sub.join("Patch/marker").exists());
        // Originals removed
        assert!(!sub.join("Main.rar").exists());
        assert!(!sub.join("Patch.rar").exists());
    }
}
