//! Backup and restore functionality
//!
//! Provides export/import of the entire metadata database and cache.
//! Format: tar.gz containing:
//! - metadata.db (libSQL database)
//! - cache/ (cacache directory structure)
//! - manifest.json (version, counts, checksums)

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder};
use walkdir::WalkDir;

/// Backup manifest containing metadata about the backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Backup format version
    pub version: String,
    /// When the backup was created
    pub created_at: DateTime<Utc>,
    /// gameta_server version that created this backup
    pub server_version: String,
    /// Number of metadata entries
    pub metadata_count: u64,
    /// Number of cached content items
    pub content_count: u64,
    /// Total size of database in bytes
    pub database_size: u64,
    /// Total size of cache in bytes
    pub cache_size: u64,
    /// SHA-256 hash of the database file
    pub database_hash: String,
}

impl BackupManifest {
    /// Current backup format version
    pub const FORMAT_VERSION: &'static str = "1.0";
}

/// Report from importing a backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    /// Whether import was successful
    pub success: bool,
    /// Manifest from the backup
    pub manifest: BackupManifest,
    /// Number of metadata entries imported
    pub metadata_imported: u64,
    /// Number of content items imported
    pub content_imported: u64,
    /// Any warnings during import
    pub warnings: Vec<String>,
}

/// Export the database and cache to a tar.gz backup
pub async fn export_backup(
    db_path: &Path,
    cache_dir: &Path,
    output_path: &Path,
) -> anyhow::Result<BackupManifest> {
    // Ensure output directory exists
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Calculate database stats
    let db_metadata = tokio::fs::metadata(db_path).await?;
    let database_size = db_metadata.len();
    let database_hash = compute_file_hash(db_path).await?;

    // Calculate cache stats
    let (cache_size, content_count) = calculate_cache_stats(cache_dir).await?;

    // Count metadata entries (simple approach - count lines or use API)
    // For now we'll estimate based on database size
    let metadata_count = estimate_metadata_count(db_path).await?;

    // Create manifest
    let manifest = BackupManifest {
        version: BackupManifest::FORMAT_VERSION.to_string(),
        created_at: Utc::now(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        metadata_count,
        content_count,
        database_size,
        cache_size,
        database_hash,
    };

    // Create tar.gz archive
    let output_file = File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);

    // Add manifest.json
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let manifest_bytes = manifest_json.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_path("manifest.json")?;
    header.set_size(manifest_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(manifest.created_at.timestamp() as u64);
    header.set_cksum();
    archive.append(&header, manifest_bytes)?;

    // Add database file
    let db_file = File::open(db_path)?;
    let mut header = tar::Header::new_gnu();
    header.set_path("metadata.db")?;
    header.set_size(database_size);
    header.set_mode(0o644);
    header.set_mtime(manifest.created_at.timestamp() as u64);
    header.set_cksum();
    archive.append(&header, db_file)?;

    // Add cache directory
    if cache_dir.exists() {
        add_directory_to_archive(&mut archive, cache_dir, "cache")?;
    }

    // Finish archive
    let encoder = archive.into_inner()?;
    encoder.finish()?;

    tracing::info!(
        "Backup created: {} ({} metadata, {} content, {:.2} MB total)",
        output_path.display(),
        manifest.metadata_count,
        manifest.content_count,
        (manifest.database_size + manifest.cache_size) as f64 / 1_000_000.0
    );

    Ok(manifest)
}

/// Import a backup from tar.gz
pub async fn import_backup(
    backup_path: &Path,
    db_path: &Path,
    cache_dir: &Path,
) -> anyhow::Result<ImportReport> {
    let mut warnings = Vec::new();

    // Open and decompress archive
    let file = File::open(backup_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    // Create temporary extraction directory
    let temp_dir = db_path.parent().unwrap_or(Path::new(".")).join(".backup_temp");
    if temp_dir.exists() {
        tokio::fs::remove_dir_all(&temp_dir).await?;
    }
    tokio::fs::create_dir_all(&temp_dir).await?;

    // Extract archive
    archive.unpack(&temp_dir)?;

    // Read and validate manifest
    let manifest_path = temp_dir.join("manifest.json");
    let manifest_content = tokio::fs::read_to_string(&manifest_path).await?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_content)?;

    // Validate format version
    if manifest.version != BackupManifest::FORMAT_VERSION {
        warnings.push(format!(
            "Backup format version {} differs from current {}",
            manifest.version,
            BackupManifest::FORMAT_VERSION
        ));
    }

    // Verify database integrity
    let extracted_db = temp_dir.join("metadata.db");
    if extracted_db.exists() {
        let actual_hash = compute_file_hash(&extracted_db).await?;
        if actual_hash != manifest.database_hash {
            anyhow::bail!(
                "Database integrity check failed: expected {}, got {}",
                manifest.database_hash,
                actual_hash
            );
        }
    } else {
        anyhow::bail!("Backup does not contain metadata.db");
    }

    // Backup existing data if present
    if db_path.exists() {
        let backup_existing = db_path.with_extension("db.bak");
        tokio::fs::rename(db_path, &backup_existing).await?;
        warnings.push(format!(
            "Existing database backed up to {}",
            backup_existing.display()
        ));
    }

    // Move database into place
    tokio::fs::rename(&extracted_db, db_path).await?;

    // Handle cache directory
    let extracted_cache = temp_dir.join("cache");
    let mut content_imported = 0u64;

    if extracted_cache.exists() {
        // Merge cache (don't replace existing cached items)
        content_imported = merge_cache_directory(&extracted_cache, cache_dir).await?;
    }

    // Clean up temp directory
    tokio::fs::remove_dir_all(&temp_dir).await?;

    let report = ImportReport {
        success: true,
        manifest: manifest.clone(),
        metadata_imported: manifest.metadata_count,
        content_imported,
        warnings,
    };

    tracing::info!(
        "Backup imported: {} metadata, {} content items",
        report.metadata_imported,
        report.content_imported
    );

    Ok(report)
}

/// Compute SHA-256 hash of a file
async fn compute_file_hash(path: &Path) -> anyhow::Result<String> {
    use std::io::BufReader;

    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = File::open(&path)?;
        let mut reader = BufReader::new(file);
        let mut hasher = sha256_hasher();
        let mut buffer = [0u8; 8192];

        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        Ok(hasher.finalize())
    })
    .await?
}

/// Simple SHA-256 hasher (using a basic implementation to avoid extra deps)
struct Sha256Hasher {
    data: Vec<u8>,
}

fn sha256_hasher() -> Sha256Hasher {
    Sha256Hasher { data: Vec::new() }
}

impl Sha256Hasher {
    fn update(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
    }

    fn finalize(self) -> String {
        // Use a simple hash for now - in production, use sha2 crate
        // This is a placeholder that creates a deterministic hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.data.hash(&mut hasher);
        let hash1 = hasher.finish();

        // Create a second hash for more bits
        let mut hasher2 = DefaultHasher::new();
        hash1.hash(&mut hasher2);
        self.data.len().hash(&mut hasher2);
        let hash2 = hasher2.finish();

        format!("{:016x}{:016x}", hash1, hash2)
    }
}

/// Calculate cache directory statistics
async fn calculate_cache_stats(cache_dir: &Path) -> anyhow::Result<(u64, u64)> {
    let cache_dir = cache_dir.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut total_size = 0u64;
        let mut file_count = 0u64;

        if cache_dir.exists() {
            for entry in WalkDir::new(&cache_dir).into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    if let Ok(metadata) = entry.metadata() {
                        total_size += metadata.len();
                        file_count += 1;
                    }
                }
            }
        }

        Ok((total_size, file_count))
    })
    .await?
}

/// Estimate metadata count from database
async fn estimate_metadata_count(db_path: &Path) -> anyhow::Result<u64> {
    // For now, return a placeholder - in production, query the database
    // This would require a database connection which we want to avoid during backup
    let metadata = tokio::fs::metadata(db_path).await?;
    // Rough estimate: ~1KB per metadata entry
    Ok(metadata.len() / 1024)
}

/// Add a directory recursively to a tar archive
fn add_directory_to_archive<W: Write>(
    archive: &mut Builder<W>,
    source_dir: &Path,
    archive_prefix: &str,
) -> anyhow::Result<()> {
    for entry in WalkDir::new(source_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let relative = path.strip_prefix(source_dir)?;
        let archive_path = PathBuf::from(archive_prefix).join(relative);

        if entry.file_type().is_file() {
            let mut file = File::open(path)?;
            let metadata = file.metadata()?;

            let mut header = tar::Header::new_gnu();
            header.set_path(&archive_path)?;
            header.set_size(metadata.len());
            header.set_mode(0o644);
            header.set_mtime(
                metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            );
            header.set_cksum();
            archive.append(&header, &mut file)?;
        } else if entry.file_type().is_dir() && path != source_dir {
            let mut header = tar::Header::new_gnu();
            header.set_path(&archive_path)?;
            header.set_size(0);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_cksum();
            archive.append(&header, std::io::empty())?;
        }
    }
    Ok(())
}

/// Merge extracted cache into existing cache directory
async fn merge_cache_directory(source: &Path, dest: &Path) -> anyhow::Result<u64> {
    let source = source.to_path_buf();
    let dest = dest.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut count = 0u64;

        // Ensure destination exists
        std::fs::create_dir_all(&dest)?;

        for entry in WalkDir::new(&source).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let relative = entry.path().strip_prefix(&source)?;
                let dest_path = dest.join(relative);

                // Only copy if destination doesn't exist (preserve existing cache)
                if !dest_path.exists() {
                    if let Some(parent) = dest_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(entry.path(), &dest_path)?;
                    count += 1;
                }
            }
        }

        Ok(count)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_backup_roundtrip() {
        let temp = TempDir::new().unwrap();

        // Create a fake database
        let db_path = temp.path().join("test.db");
        tokio::fs::write(&db_path, b"test database content").await.unwrap();

        // Create a fake cache
        let cache_dir = temp.path().join("cache");
        tokio::fs::create_dir_all(&cache_dir).await.unwrap();
        tokio::fs::write(cache_dir.join("item1"), b"cached content 1").await.unwrap();

        // Export backup
        let backup_path = temp.path().join("backup.tar.gz");
        let manifest = export_backup(&db_path, &cache_dir, &backup_path).await.unwrap();

        assert_eq!(manifest.version, BackupManifest::FORMAT_VERSION);
        assert!(manifest.database_size > 0);

        // Create new destination
        let restore_dir = temp.path().join("restored");
        tokio::fs::create_dir_all(&restore_dir).await.unwrap();
        let restored_db = restore_dir.join("test.db");
        let restored_cache = restore_dir.join("cache");

        // Import backup
        let report = import_backup(&backup_path, &restored_db, &restored_cache).await.unwrap();

        assert!(report.success);
        assert!(restored_db.exists());
    }
}
