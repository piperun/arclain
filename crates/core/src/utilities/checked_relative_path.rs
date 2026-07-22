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
            let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'));
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
        let root_meta = std::fs::symlink_metadata(root)
            .with_context(|| format!("inspect root {}", root.display()))?;
        if root_meta.file_type().is_symlink() {
            bail!("filesystem root may not be a symlink: {}", root.display());
        }

        let canonical_root = root
            .canonicalize()
            .with_context(|| format!("canonicalize root {}", root.display()))?;
        let mut resolved = canonical_root.clone();
        for component in self.0.components() {
            resolved.push(component.as_os_str());
            match std::fs::symlink_metadata(&resolved) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    bail!("path traverses symlink component {}", resolved.display());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
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
