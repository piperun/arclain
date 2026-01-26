//! Archive format profiles for organization operations
//!
//! Stores compression presets (e.g., "Max 7z", "Fast 7z", "Zip Compatible")
//! that can be selected when organizing archives.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Database model for archive format profiles
#[derive(Debug, Clone)]
pub struct DbArchiveProfile {
    pub id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub format: String,              // "7z", "zip"
    pub compression_level: i32,      // 0-9
    pub compression_method: Option<String>, // "LZMA2", "Deflate", etc.
    pub solid_archive: bool,
    pub encrypt_headers: bool,
    pub is_default: bool,
    pub is_system: bool,
}

/// Diesel-compatible query result for archive profiles
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::diesel_schema::archive_profiles)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbArchiveProfileRow {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub format: String,
    pub compression_level: i32,
    pub compression_method: Option<String>,
    pub solid_archive: bool,
    pub encrypt_headers: bool,
    pub is_default: bool,
    pub is_system: bool,
    pub created_at: String,
    pub modified_at: Option<String>,
}

// ============================================================================
// Rusqlite CRUD operations
// ============================================================================

pub fn list_profiles(conn: &Connection) -> Result<Vec<DbArchiveProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, format, compression_level, compression_method,
                solid_archive, encrypt_headers, is_default, is_system
         FROM archive_profiles
         ORDER BY is_default DESC, name ASC",
    )?;

    let profiles = stmt
        .query_map([], |row| {
            Ok(DbArchiveProfile {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                format: row.get(3)?,
                compression_level: row.get(4)?,
                compression_method: row.get(5)?,
                solid_archive: row.get(6)?,
                encrypt_headers: row.get(7)?,
                is_default: row.get(8)?,
                is_system: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(profiles)
}

pub fn get_profile(conn: &Connection, id: i64) -> Result<Option<DbArchiveProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, format, compression_level, compression_method,
                solid_archive, encrypt_headers, is_default, is_system
         FROM archive_profiles
         WHERE id = ?1",
    )?;

    let profile = stmt
        .query_row([id], |row| {
            Ok(DbArchiveProfile {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                format: row.get(3)?,
                compression_level: row.get(4)?,
                compression_method: row.get(5)?,
                solid_archive: row.get(6)?,
                encrypt_headers: row.get(7)?,
                is_default: row.get(8)?,
                is_system: row.get(9)?,
            })
        })
        .optional()?;

    Ok(profile)
}

pub fn get_default_profile(conn: &Connection) -> Result<Option<DbArchiveProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, format, compression_level, compression_method,
                solid_archive, encrypt_headers, is_default, is_system
         FROM archive_profiles
         WHERE is_default = 1
         LIMIT 1",
    )?;

    let profile = stmt
        .query_row([], |row| {
            Ok(DbArchiveProfile {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                format: row.get(3)?,
                compression_level: row.get(4)?,
                compression_method: row.get(5)?,
                solid_archive: row.get(6)?,
                encrypt_headers: row.get(7)?,
                is_default: row.get(8)?,
                is_system: row.get(9)?,
            })
        })
        .optional()?;

    Ok(profile)
}

pub fn save_profile(conn: &Connection, profile: &DbArchiveProfile) -> Result<i64> {
    // If setting as default, clear other defaults first
    if profile.is_default {
        conn.execute("UPDATE archive_profiles SET is_default = 0", [])?;
    }

    if let Some(id) = profile.id {
        // Update
        conn.execute(
            "UPDATE archive_profiles
             SET name = ?1, description = ?2, format = ?3, compression_level = ?4,
                 compression_method = ?5, solid_archive = ?6, encrypt_headers = ?7,
                 is_default = ?8, is_system = ?9, modified_at = CURRENT_TIMESTAMP
             WHERE id = ?10",
            params![
                profile.name,
                profile.description,
                profile.format,
                profile.compression_level,
                profile.compression_method,
                profile.solid_archive,
                profile.encrypt_headers,
                profile.is_default,
                profile.is_system,
                id
            ],
        )
        .context("Failed to update archive profile")?;
        Ok(id)
    } else {
        // Insert
        conn.execute(
            "INSERT INTO archive_profiles
             (name, description, format, compression_level, compression_method,
              solid_archive, encrypt_headers, is_default, is_system)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                profile.name,
                profile.description,
                profile.format,
                profile.compression_level,
                profile.compression_method,
                profile.solid_archive,
                profile.encrypt_headers,
                profile.is_default,
                profile.is_system
            ],
        )
        .context("Failed to insert archive profile")?;
        Ok(conn.last_insert_rowid())
    }
}

pub fn delete_profile(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM archive_profiles WHERE id = ?1 AND is_system = 0",
        [id],
    )?;
    Ok(())
}

pub fn set_default_profile(conn: &Connection, id: i64) -> Result<()> {
    // Clear all defaults
    conn.execute("UPDATE archive_profiles SET is_default = 0", [])?;
    // Set the new default
    conn.execute(
        "UPDATE archive_profiles SET is_default = 1 WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

// ============================================================================
// Diesel DSL versions
// ============================================================================

use diesel::prelude::*;

/// List all profiles using Diesel DSL
pub fn list_profiles_diesel(
    conn: &mut diesel::SqliteConnection,
) -> Result<Vec<DbArchiveProfile>> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    let results = archive_profiles
        .order((is_default.desc(), name.asc()))
        .load::<DbArchiveProfileRow>(conn)
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(results.into_iter().map(row_to_profile).collect())
}

/// Get a single profile by ID using Diesel DSL
pub fn get_profile_diesel(
    conn: &mut diesel::SqliteConnection,
    profile_id: i32,
) -> Result<Option<DbArchiveProfile>> {
    use crate::diesel_schema::archive_profiles::dsl::*;
    use diesel::result::OptionalExtension;

    let result = archive_profiles
        .filter(id.eq(profile_id))
        .first::<DbArchiveProfileRow>(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(result.map(row_to_profile))
}

/// Get the default profile using Diesel DSL
pub fn get_default_profile_diesel(
    conn: &mut diesel::SqliteConnection,
) -> Result<Option<DbArchiveProfile>> {
    use crate::diesel_schema::archive_profiles::dsl::*;
    use diesel::result::OptionalExtension;

    let result = archive_profiles
        .filter(is_default.eq(true))
        .first::<DbArchiveProfileRow>(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("Diesel query failed: {}", e))?;

    Ok(result.map(row_to_profile))
}

/// Delete a profile using Diesel DSL
pub fn delete_profile_diesel(conn: &mut diesel::SqliteConnection, profile_id: i32) -> Result<()> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    diesel::delete(archive_profiles.filter(id.eq(profile_id).and(is_system.eq(false))))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel delete failed: {}", e))?;

    Ok(())
}

/// Save a profile (Insert or Update) using Diesel DSL
pub fn save_profile_diesel(
    conn: &mut diesel::SqliteConnection,
    profile: &DbArchiveProfile,
) -> Result<i64> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    // If setting as default, clear other defaults first
    if profile.is_default {
        diesel::update(archive_profiles)
            .set(is_default.eq(false))
            .execute(conn)
            .map_err(|e| anyhow::anyhow!("Diesel update failed: {}", e))?;
    }

    if let Some(profile_id) = profile.id {
        // Update
        diesel::update(archive_profiles.filter(id.eq(profile_id as i32)))
            .set((
                name.eq(&profile.name),
                description.eq(&profile.description),
                format.eq(&profile.format),
                compression_level.eq(profile.compression_level),
                compression_method.eq(&profile.compression_method),
                solid_archive.eq(profile.solid_archive),
                encrypt_headers.eq(profile.encrypt_headers),
                is_default.eq(profile.is_default),
                is_system.eq(profile.is_system),
                modified_at.eq(chrono::Utc::now().to_rfc3339()),
            ))
            .execute(conn)
            .map_err(|e| anyhow::anyhow!("Diesel update failed: {}", e))?;
        Ok(profile_id)
    } else {
        // Insert
        let new_id: i32 = diesel::insert_into(archive_profiles)
            .values((
                name.eq(&profile.name),
                description.eq(&profile.description),
                format.eq(&profile.format),
                compression_level.eq(profile.compression_level),
                compression_method.eq(&profile.compression_method),
                solid_archive.eq(profile.solid_archive),
                encrypt_headers.eq(profile.encrypt_headers),
                is_default.eq(profile.is_default),
                is_system.eq(profile.is_system),
            ))
            .returning(id)
            .get_result(conn)
            .map_err(|e| anyhow::anyhow!("Diesel insert failed: {}", e))?;
        Ok(new_id as i64)
    }
}

/// Set a profile as the default using Diesel DSL
pub fn set_default_profile_diesel(
    conn: &mut diesel::SqliteConnection,
    profile_id: i32,
) -> Result<()> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    // Clear all defaults
    diesel::update(archive_profiles)
        .set(is_default.eq(false))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel update failed: {}", e))?;

    // Set the new default
    diesel::update(archive_profiles.filter(id.eq(profile_id)))
        .set(is_default.eq(true))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("Diesel update failed: {}", e))?;

    Ok(())
}

// Helper to convert Diesel row to domain model
fn row_to_profile(r: DbArchiveProfileRow) -> DbArchiveProfile {
    DbArchiveProfile {
        id: Some(r.id as i64),
        name: r.name,
        description: r.description,
        format: r.format,
        compression_level: r.compression_level,
        compression_method: r.compression_method,
        solid_archive: r.solid_archive,
        encrypt_headers: r.encrypt_headers,
        is_default: r.is_default,
        is_system: r.is_system,
    }
}
