use anyhow::Result;
use std::path::Path;

/// Common archive file extensions
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "tar.gz", "tar.bz2", "tar.xz",
];

/// Check if a file path has an archive extension
pub fn is_archive_extension(path: &Path) -> bool {
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Check compound extensions first
    for ext in ARCHIVE_EXTENSIONS {
        if filename.ends_with(&format!(".{}", ext)) {
            return true;
        }
    }
    false
}

/// Recursively copy a directory
pub fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            std::fs::copy(&src_path, &dest_path)?;
        }
    }

    Ok(())
}
