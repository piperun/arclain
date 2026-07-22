use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// A portable, non-empty relative path that cannot lexically escape a root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckedRelativePath(PathBuf);

impl CheckedRelativePath {
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let raw = value.as_ref();
        if raw.is_empty() {
            bail!("relative path is empty");
        }

        let normalized = raw.replace('\\', "/");
        if normalized.starts_with('/') || normalized.contains("//") {
            bail!("path must be a non-empty relative path: {raw:?}");
        }

        for component in normalized.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                bail!("unsafe path component {component:?} in {raw:?}");
            }
            if component.ends_with('.')
                || component.ends_with(' ')
                || component
                    .chars()
                    .any(|character| character.is_control() || r#"<>:\"|?*"#.contains(character))
            {
                bail!("non-portable path component {component:?}");
            }

            let stem = component
                .split('.')
                .next()
                .unwrap_or(component)
                .to_ascii_uppercase();
            let numbered_device_suffix = stem
                .strip_prefix("COM")
                .or_else(|| stem.strip_prefix("LPT"));
            let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || matches!(
                    numbered_device_suffix,
                    Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
                );
            if reserved {
                bail!("reserved path component {component:?}");
            }
        }

        Ok(Self(PathBuf::from(normalized)))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Rejects static symlink traversal and returns the checked host path.
    ///
    /// This is not a handle-relative open: callers that write files must use
    /// same-parent random staging plus `persist_noclobber` and retain the
    /// documented local-process TOCTOU limitation from the plan constraints.
    pub fn resolve_under(&self, root: &Path) -> Result<PathBuf> {
        let normalized_root: PathBuf = root.components().collect();
        let root_meta = std::fs::symlink_metadata(&normalized_root)
            .with_context(|| format!("inspect root {}", root.display()))?;
        if root_meta.file_type().is_symlink() {
            bail!("filesystem root may not be a symlink: {}", root.display());
        }
        if !root_meta.is_dir() {
            bail!("filesystem root must be a directory: {}", root.display());
        }

        let canonical_root = normalized_root
            .canonicalize()
            .with_context(|| format!("canonicalize root {}", root.display()))?;
        let mut resolved = canonical_root.clone();
        let mut components = self.0.components().peekable();
        while let Some(component) = components.next() {
            resolved.push(component.as_os_str());
            match std::fs::symlink_metadata(&resolved) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    bail!("path traverses symlink component {}", resolved.display());
                }
                Ok(meta) if components.peek().is_some() && !meta.is_dir() => {
                    bail!("path parent is not a directory: {}", resolved.display());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    for remaining in components {
                        resolved.push(remaining.as_os_str());
                    }
                    break;
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("inspect {}", resolved.display()));
                }
            }
        }

        if !resolved.starts_with(&canonical_root) {
            bail!("resolved path escaped {}", canonical_root.display());
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_escape_absolute_empty_and_reserved_paths() {
        for unsafe_path in [
            "",
            "/absolute",
            r"C:\absolute",
            "../escape",
            "safe/../../escape",
            "safe//empty",
            "safe/./dot",
            "CON",
            "dir/NUL.txt",
            "trailing. ",
            "colon:name",
            "question?mark",
        ] {
            assert!(
                CheckedRelativePath::new(unsafe_path).is_err(),
                "accepted unsafe path {unsafe_path:?}",
            );
        }
    }

    #[test]
    fn normalizes_backslashes_to_portable_relative_path() {
        let path = CheckedRelativePath::new(r"Game\data\file.bin").unwrap();
        assert_eq!(path.as_path(), Path::new("Game/data/file.bin"));
    }

    #[test]
    fn rejects_superscript_digit_windows_device_names() {
        for unsafe_path in [
            "COM¹",
            "com².txt",
            "CoM³.bin",
            "LPT¹",
            "lpt².log",
            "dir/LpT³.data",
        ] {
            assert!(
                CheckedRelativePath::new(unsafe_path).is_err(),
                "accepted reserved device path {unsafe_path:?}",
            );
        }
    }

    #[test]
    fn resolve_under_allows_existing_parent_with_missing_leaf() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(root.join("existing")).unwrap();

        let path = CheckedRelativePath::new("existing/missing.txt").unwrap();
        let resolved = path.resolve_under(&root).unwrap();

        assert_eq!(
            resolved,
            root.canonicalize().unwrap().join("existing/missing.txt")
        );
    }

    #[test]
    fn resolve_under_allows_entirely_missing_subtree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();

        let path = CheckedRelativePath::new("missing/subtree/file.bin").unwrap();
        let resolved = path.resolve_under(&root).unwrap();

        assert_eq!(
            resolved,
            root.canonicalize()
                .unwrap()
                .join("missing/subtree/file.bin")
        );
    }

    #[test]
    fn resolve_under_rejects_regular_file_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root-file");
        std::fs::write(&root, b"not a directory").unwrap();

        let path = CheckedRelativePath::new("child.txt").unwrap();
        let error = path.resolve_under(&root).unwrap_err().to_string();

        assert!(error.contains("directory"), "unexpected error: {error}");
    }

    #[test]
    fn resolve_under_rejects_regular_file_parent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file-parent"), b"not a directory").unwrap();

        let path = CheckedRelativePath::new("file-parent/child.txt").unwrap();
        let error = path.resolve_under(&root).unwrap_err().to_string();

        assert!(error.contains("directory"), "unexpected error: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_under_rejects_symlink_root() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let root = temp.path().join("root-link");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &root).unwrap();

        let path = CheckedRelativePath::new("escaped.txt").unwrap();
        let error = path.resolve_under(&root).unwrap_err().to_string();

        assert!(error.contains("symlink"), "unexpected error: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_under_rejects_symlink_root_with_trailing_separator() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let root = temp.path().join("root-link");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &root).unwrap();
        let root_with_separator = PathBuf::from(format!("{}/", root.display()));

        let path = CheckedRelativePath::new("escaped.txt").unwrap();
        let error = path
            .resolve_under(&root_with_separator)
            .unwrap_err()
            .to_string();

        assert!(error.contains("symlink"), "unexpected error: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_under_rejects_symlink_root_with_dot_component() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let root = temp.path().join("root-link");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &root).unwrap();

        let path = CheckedRelativePath::new("escaped.txt").unwrap();
        let error = path.resolve_under(&root.join(".")).unwrap_err().to_string();

        assert!(error.contains("symlink"), "unexpected error: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_under_rejects_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        let path = CheckedRelativePath::new("linked/escaped.txt").unwrap();
        let error = path.resolve_under(&root).unwrap_err().to_string();
        assert!(error.contains("symlink"), "unexpected error: {error}");
    }
}
