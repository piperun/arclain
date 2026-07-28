//! Per-plugin scratch storage for host-created files.
//!
//! Each normal plugin instance owns one private temporary directory. Files are
//! always created with `create_new`, are quota-accounted only after a complete
//! write, and disappear when the host state is dropped during unload.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) const MAX_PLUGIN_TEMP_FILES: usize = 128;
pub(super) const MAX_PLUGIN_TEMP_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SAFE_FILENAME_BYTES: usize = 96;
const MAX_CREATE_ATTEMPTS: usize = 16;

pub(super) struct PluginTempStorage {
    root: tempfile::TempDir,
    files_created: usize,
    bytes_written: u64,
    next_file_id: u64,
}

impl PluginTempStorage {
    pub(super) fn new() -> std::io::Result<Self> {
        Ok(Self {
            root: tempfile::Builder::new()
                .prefix("arclain-plugin-")
                .tempdir()?,
            files_created: 0,
            bytes_written: 0,
            next_file_id: 0,
        })
    }

    pub(super) fn create_file(
        &mut self,
        requested_name: &str,
        content: &[u8],
    ) -> Result<PathBuf, String> {
        if self.files_created >= MAX_PLUGIN_TEMP_FILES {
            return Err(format!(
                "plugin temporary file quota exceeded (max {MAX_PLUGIN_TEMP_FILES} files)"
            ));
        }

        let content_len = u64::try_from(content.len())
            .map_err(|_| "plugin temporary byte quota exceeded".to_string())?;
        let next_total = self
            .bytes_written
            .checked_add(content_len)
            .ok_or_else(|| "plugin temporary byte quota exceeded".to_string())?;
        if next_total > MAX_PLUGIN_TEMP_BYTES {
            return Err(format!(
                "plugin temporary byte quota exceeded (max {MAX_PLUGIN_TEMP_BYTES} bytes)"
            ));
        }

        let safe_name = safe_filename(requested_name)?;
        for _ in 0..MAX_CREATE_ATTEMPTS {
            let file_id = self.next_file_id;
            self.next_file_id = self
                .next_file_id
                .checked_add(1)
                .ok_or_else(|| "plugin temporary file id exhausted".to_string())?;
            let path = self.root.path().join(format!("{file_id:016x}-{safe_name}"));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("failed to create plugin temporary file: {error}"));
                }
            };

            if let Err(error) = file.write_all(content).and_then(|()| file.flush()) {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(format!("failed to write plugin temporary file: {error}"));
            }

            self.files_created += 1;
            self.bytes_written = next_total;
            return Ok(path);
        }

        Err("failed to allocate a collision-free plugin temporary file".to_string())
    }
}

fn safe_filename(requested_name: &str) -> Result<String, String> {
    if requested_name.is_empty()
        || Path::new(requested_name).is_absolute()
        || requested_name.contains(['/', '\\'])
    {
        return Err("plugin temporary filename must be a single relative name".to_string());
    }

    let mut safe = String::with_capacity(requested_name.len().min(MAX_SAFE_FILENAME_BYTES));
    for character in requested_name.chars() {
        if safe.len() >= MAX_SAFE_FILENAME_BYTES {
            break;
        }
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
            safe.push(character);
        } else {
            safe.push('_');
        }
    }

    if safe.is_empty() || safe == "." || safe == ".." {
        return Err("plugin temporary filename is invalid".to_string());
    }
    Ok(safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_byte_quota_accepts_exact_boundary_then_rejects_next_byte() {
        let mut storage = PluginTempStorage::new().unwrap();
        storage.bytes_written = MAX_PLUGIN_TEMP_BYTES - 2;

        storage
            .create_file("first.bin", b"x")
            .expect("first cumulative byte is within quota");
        storage
            .create_file("boundary.bin", b"y")
            .expect("exact quota boundary is accepted");
        let error = storage
            .create_file("over.bin", b"z")
            .expect_err("next cumulative byte must exceed quota");

        assert!(error.contains("byte quota exceeded"));
        assert_eq!(storage.bytes_written, MAX_PLUGIN_TEMP_BYTES);
        assert_eq!(storage.files_created, 2);
    }
}
