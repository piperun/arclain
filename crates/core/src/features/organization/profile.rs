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

    /// Whether `ArchiveProfile::solid_archive` means anything for this
    /// format.
    ///
    /// The container decides: only 7z has solid blocks, so the packer
    /// emits `-ms=on`/`-ms=off` for it and nothing at all for zip (see
    /// `backends::sevenz_cli::backend`'s per-format switch
    /// construction). Exposed so a profile editor can hide a toggle that
    /// would otherwise store a flag nothing can honor, instead of each
    /// frontend re-deriving "is this 7z?" for itself.
    pub fn supports_solid_archive(&self) -> bool {
        matches!(self, ArchiveFormat::SevenZ)
    }

    /// Whether `ArchiveProfile::encrypt_headers` means anything for this
    /// format. Only 7z can encrypt its own file listing (`-mhe=on`); a
    /// zip's listing is always readable.
    pub fn supports_header_encryption(&self) -> bool {
        matches!(self, ArchiveFormat::SevenZ)
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

/// Lists every archive profile directly against the config database at
/// `config_db_path`, the same short-lived, unpooled-connection pattern
/// [`load_archive_profile`] uses (see its own doc comment for why) --
/// `arclain_app::ArclainApp::organization_profiles` is this function's
/// one caller.
pub fn list_archive_profiles(
    config_db_path: &std::path::Path,
) -> anyhow::Result<Vec<ArchiveProfile>> {
    let mut conn = diesel::SqliteConnection::establish(&config_db_path.to_string_lossy())
        .with_context(|| format!("opening config database at {}", config_db_path.display()))?;
    let db_profiles =
        arclain_db::list_profiles(&mut conn).with_context(|| "listing archive profiles")?;
    Ok(db_profiles.iter().map(ArchiveProfile::from_db).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every format's compression menu must offer its own default, or a
    /// profile editor's method picker opens on a value it cannot show.
    #[test]
    fn every_format_offers_its_own_default_method() {
        for format in ArchiveFormat::all() {
            let probe = ArchiveProfile {
                format: *format,
                ..ArchiveProfile::default()
            };
            assert!(
                probe
                    .available_compression_methods()
                    .contains(&probe.default_compression_method()),
                "{} does not offer its own default method",
                format.as_str()
            );
        }
    }

    /// The two container capabilities, pinned to what the packer
    /// actually emits: `-ms`/`-mhe` appear only in the 7z arm of
    /// `backends::sevenz_cli::backend::create_archive_with_profile`, and
    /// the zip arm emits neither.
    #[test]
    fn only_the_seven_zip_container_honours_solid_blocks_and_header_encryption() {
        assert!(ArchiveFormat::SevenZ.supports_solid_archive());
        assert!(ArchiveFormat::SevenZ.supports_header_encryption());
        assert!(!ArchiveFormat::Zip.supports_solid_archive());
        assert!(!ArchiveFormat::Zip.supports_header_encryption());
    }
}
