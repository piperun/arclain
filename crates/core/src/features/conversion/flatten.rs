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
                report.extracted.push(folder_name);
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
