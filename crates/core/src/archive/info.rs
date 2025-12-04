//! Archive metadata types

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArchiveKind {
    Zip,
    SevenZ,
    Rar,
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub size: u64,
    pub packed_size: u64,
    pub modified: Option<String>,
    pub is_dir: bool,
    pub encrypted: bool,
    pub crc32: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub archive_path: PathBuf,
    pub archive_kind: ArchiveKind,
    pub entries: Vec<ArchiveEntry>,
    pub encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
}