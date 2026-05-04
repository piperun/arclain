//! Archive format profiles for organization operations
//!
//! Stores compression presets (e.g., "Max 7z", "Fast 7z", "Zip Compatible")
//! that can be selected when organizing archives.

use crate::diesel_err;
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
        .map_err(diesel_err("query"))?;

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
        .map_err(diesel_err("query"))?;

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
        .map_err(diesel_err("query"))?;

    Ok(result.map(row_to_profile))
}

/// Delete a profile using Diesel DSL
pub fn delete_profile_diesel(conn: &mut diesel::SqliteConnection, profile_id: i32) -> Result<()> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    diesel::delete(archive_profiles.filter(id.eq(profile_id).and(is_system.eq(false))))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

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
            .map_err(diesel_err("update"))?;
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
            .map_err(diesel_err("update"))?;
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
            .map_err(diesel_err("insert"))?;
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
        .map_err(diesel_err("update"))?;

    // Set the new default
    diesel::update(archive_profiles.filter(id.eq(profile_id)))
        .set(is_default.eq(true))
        .execute(conn)
        .map_err(diesel_err("update"))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS archive_profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT,
                format TEXT NOT NULL DEFAULT '7z',
                compression_level INTEGER NOT NULL DEFAULT 9,
                compression_method TEXT,
                solid_archive INTEGER NOT NULL DEFAULT 1,
                encrypt_headers INTEGER NOT NULL DEFAULT 0,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_system INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                modified_at TEXT
            )",
            [],
        )
        .unwrap();
        conn
    }

    fn make_profile(name: &str) -> DbArchiveProfile {
        DbArchiveProfile {
            id: None,
            name: name.to_string(),
            description: Some(format!("{} description", name)),
            format: "7z".to_string(),
            compression_level: 9,
            compression_method: Some("LZMA2".to_string()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: false,
            is_system: false,
        }
    }

    #[test]
    fn test_save_and_list() {
        let conn = setup_db();
        let profile = make_profile("Max 7z");
        let id = save_profile(&conn, &profile).unwrap();
        assert!(id > 0);

        let profiles = list_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Max 7z");
        assert_eq!(profiles[0].id, Some(id));
    }

    #[test]
    fn test_get_profile() {
        let conn = setup_db();
        let id = save_profile(&conn, &make_profile("Fast 7z")).unwrap();

        let loaded = get_profile(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.name, "Fast 7z");
        assert_eq!(loaded.compression_level, 9);
    }

    #[test]
    fn test_get_nonexistent() {
        let conn = setup_db();
        assert!(get_profile(&conn, 999).unwrap().is_none());
    }

    #[test]
    fn test_update_profile() {
        let conn = setup_db();
        let id = save_profile(&conn, &make_profile("Original")).unwrap();

        let mut updated = make_profile("Renamed");
        updated.id = Some(id);
        updated.compression_level = 5;
        save_profile(&conn, &updated).unwrap();

        let loaded = get_profile(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.name, "Renamed");
        assert_eq!(loaded.compression_level, 5);
    }

    #[test]
    fn test_delete_profile() {
        let conn = setup_db();
        let id = save_profile(&conn, &make_profile("Temp")).unwrap();
        assert!(get_profile(&conn, id).unwrap().is_some());

        delete_profile(&conn, id).unwrap();
        assert!(get_profile(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_delete_system_profile_is_noop() {
        let conn = setup_db();
        let mut system = make_profile("System");
        system.is_system = true;
        let id = save_profile(&conn, &system).unwrap();

        delete_profile(&conn, id).unwrap();
        // System profiles cannot be deleted
        assert!(get_profile(&conn, id).unwrap().is_some());
    }

    #[test]
    fn test_set_default_profile() {
        let conn = setup_db();
        let id1 = save_profile(&conn, &make_profile("Profile A")).unwrap();
        let id2 = save_profile(&conn, &make_profile("Profile B")).unwrap();

        set_default_profile(&conn, id1).unwrap();
        assert!(get_profile(&conn, id1).unwrap().unwrap().is_default);
        assert!(!get_profile(&conn, id2).unwrap().unwrap().is_default);

        // Change default
        set_default_profile(&conn, id2).unwrap();
        assert!(!get_profile(&conn, id1).unwrap().unwrap().is_default);
        assert!(get_profile(&conn, id2).unwrap().unwrap().is_default);
    }

    #[test]
    fn test_get_default_profile() {
        let conn = setup_db();
        assert!(get_default_profile(&conn).unwrap().is_none());

        let mut profile = make_profile("Default");
        profile.is_default = true;
        save_profile(&conn, &profile).unwrap();

        let default = get_default_profile(&conn).unwrap().unwrap();
        assert_eq!(default.name, "Default");
    }

    #[test]
    fn test_save_default_clears_other_defaults() {
        let conn = setup_db();
        let mut p1 = make_profile("First Default");
        p1.is_default = true;
        save_profile(&conn, &p1).unwrap();

        let mut p2 = make_profile("Second Default");
        p2.is_default = true;
        save_profile(&conn, &p2).unwrap();

        // Only one default should exist
        let profiles = list_profiles(&conn).unwrap();
        let defaults: Vec<_> = profiles.iter().filter(|p| p.is_default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].name, "Second Default");
    }

    #[test]
    fn test_list_ordering() {
        let conn = setup_db();
        save_profile(&conn, &make_profile("Zebra")).unwrap();
        save_profile(&conn, &make_profile("Alpha")).unwrap();
        let mut def = make_profile("Middle");
        def.is_default = true;
        save_profile(&conn, &def).unwrap();

        let profiles = list_profiles(&conn).unwrap();
        // Default first, then alphabetical
        assert_eq!(profiles[0].name, "Middle");
        assert_eq!(profiles[1].name, "Alpha");
        assert_eq!(profiles[2].name, "Zebra");
    }
}
