//! Batch archive conversion — scan a folder for archives.
//!
//! For 1.1, this module provides scanning only. Execution (running convert
//! across many files) is deferred to 1.2 — it requires extracting the current
//! UI-bound convert pipeline into a blocking core operation first.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Find all archive files in a directory (non-recursive).
pub fn find_archives_in_folder(dir: &Path) -> Result<Vec<PathBuf>> {
    arclain_core::features::conversion::flatten::find_archive_files(dir)
}

/// Batch convert result.
#[derive(Debug, Default)]
pub struct BatchReport {
    pub converted: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub total: usize,
}

impl BatchReport {
    pub fn success_count(&self) -> usize {
        self.converted.len()
    }
}
