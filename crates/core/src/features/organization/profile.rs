//! Archive format profiles for organization operations
//!
//! Defines compression presets that can be selected when organizing archives.

use anyhow::Context;
use diesel::Connection;
use serde::{Deserialize, Serialize};

/// Output format for organized archives
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ArchiveFormat {
    #[default]
    SevenZ,
    Zip,
}

impl ArchiveFormat {
    /// Get the file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Zip => "zip",
        }
    }

    /// Get the format argument for 7-Zip CLI
    pub fn format_arg(&self) -> &'static str {
        match self {
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Zip => "zip",
        }
    }

    /// Parse from string (e.g., from database)
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "zip" => ArchiveFormat::Zip,
            _ => ArchiveFormat::SevenZ,
        }
    }

    /// Convert to string for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Zip => "zip",
        }
    }

    /// Get available formats
    pub fn all() -> &'static [ArchiveFormat] {
        &[ArchiveFormat::SevenZ, ArchiveFormat::Zip]
    }

    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Zip => "ZIP",
        }
    }
}

/// Archive format profile with compression settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveProfile {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub format: ArchiveFormat,
    pub compression_level: u8,
    pub compression_method: Option<String>,
    pub solid_archive: bool,
    pub encrypt_headers: bool,
    pub is_default: bool,
    pub is_system: bool,
}

impl Default for ArchiveProfile {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Maximum Compression (7z)".to_string(),
            description: Some(
                "Best compression ratio, slower speed. Uses LZMA2 algorithm.".to_string(),
            ),
            format: ArchiveFormat::SevenZ,
            compression_level: 9,
            compression_method: Some("LZMA2".to_string()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: true,
            is_system: true,
        }
    }
}

impl ArchiveProfile {
    /// Convert from database model
    pub fn from_db(db: &arclain_db::DbArchiveProfile) -> Self {
        Self {
            id: db.id.unwrap_or(0),
            name: db.name.clone(),
            description: db.description.clone(),
            format: ArchiveFormat::from_str(&db.format),
            compression_level: db.compression_level as u8,
            compression_method: db.compression_method.clone(),
            solid_archive: db.solid_archive,
            encrypt_headers: db.encrypt_headers,
            is_default: db.is_default,
            is_system: db.is_system,
        }
    }

    /// Convert to database model
    pub fn to_db(&self) -> arclain_db::DbArchiveProfile {
        arclain_db::DbArchiveProfile {
            id: if self.id > 0 { Some(self.id) } else { None },
            name: self.name.clone(),
            description: self.description.clone(),
            format: self.format.as_str().to_string(),
            compression_level: self.compression_level as i32,
            compression_method: self.compression_method.clone(),
            solid_archive: self.solid_archive,
            encrypt_headers: self.encrypt_headers,
            is_default: self.is_default,
            is_system: self.is_system,
        }
    }

    /// Get available compression methods for the current format
    pub fn available_compression_methods(&self) -> &'static [&'static str] {
        match self.format {
            ArchiveFormat::SevenZ => &["LZMA2", "LZMA", "PPMd", "BZip2"],
            ArchiveFormat::Zip => &["Deflate", "Deflate64", "BZip2", "LZMA"],
        }
    }

    /// Get default compression method for the current format
    pub fn default_compression_method(&self) -> &'static str {
        match self.format {
            ArchiveFormat::SevenZ => "LZMA2",
            ArchiveFormat::Zip => "Deflate",
        }
    }
}

/// Looks up one archive profile by id directly against the config
/// database at `config_db_path`, opening a short-lived, unpooled
/// connection for just this one query.
///
/// A deliberately narrow entry point, not a general-purpose pool
/// constructor: unlike organization rules (`OrganizationService` holds a
/// long-lived `DieselPool` for the app's whole lifetime), nothing in this
/// crate keeps a standing handle onto the `archive_profiles` table today
/// -- `crates/ui`'s own callers (`browser_controller.rs`, `profiles_page/
/// mod.rs`) each open their own connection per lookup too. The
/// application facade is one more such caller, not a reason to introduce
/// pooling here; if profile lookups become a hot path, revisit this
/// alongside those existing call sites, not in isolation.
pub fn load_archive_profile(
    config_db_path: &std::path::Path,
    profile_id: i64,
) -> anyhow::Result<Option<ArchiveProfile>> {
    let mut conn = diesel::SqliteConnection::establish(&config_db_path.to_string_lossy())
        .with_context(|| format!("opening config database at {}", config_db_path.display()))?;
    let db_profile = arclain_db::get_profile(&mut conn, profile_id as i32)
        .with_context(|| format!("looking up archive profile {profile_id}"))?;
    Ok(db_profile.map(|profile| ArchiveProfile::from_db(&profile)))
}
