//! Archive format profiles for organization operations
//!
//! Stores compression presets (e.g., "Max 7z", "Fast 7z", "Zip Compatible")
//! that can be selected when organizing archives.

use crate::diesel_err;
use anyhow::Result;
use diesel::prelude::*;

/// Database model for archive format profiles
#[derive(Debug, Clone)]
pub struct DbArchiveProfile {
    pub id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub format: String,                     // "7z", "zip"
    pub compression_level: i32,             // 0-9
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
// Diesel DSL CRUD
// ============================================================================

/// List all profiles
pub fn list_profiles(conn: &mut diesel::SqliteConnection) -> Result<Vec<DbArchiveProfile>> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    let results = archive_profiles
        .order((is_default.desc(), name.asc()))
        .load::<DbArchiveProfileRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(results.into_iter().map(row_to_profile).collect())
}

/// Get a single profile by ID
pub fn get_profile(
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

/// Get the default profile
pub fn get_default_profile(
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

/// Delete a profile (system profiles are immune to delete)
pub fn delete_profile(conn: &mut diesel::SqliteConnection, profile_id: i32) -> Result<()> {
    use crate::diesel_schema::archive_profiles::dsl::*;

    diesel::delete(archive_profiles.filter(id.eq(profile_id).and(is_system.eq(false))))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Save a profile (Insert or Update)
pub fn save_profile(
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

/// Set a profile as the default
pub fn set_default_profile(conn: &mut diesel::SqliteConnection, profile_id: i32) -> Result<()> {
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
    use diesel::Connection;
    use diesel::RunQueryDsl;

    fn setup_db() -> diesel::SqliteConnection {
        let mut conn = diesel::SqliteConnection::establish(":memory:").expect("in-memory SQLite");
        diesel::sql_query(
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
        )
        .execute(&mut conn)
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
        let mut conn = setup_db();
        let profile = make_profile("Max 7z");
        let id = save_profile(&mut conn, &profile).unwrap();
        assert!(id > 0);

        let profiles = list_profiles(&mut conn).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Max 7z");
        assert_eq!(profiles[0].id, Some(id));
    }

    #[test]
    fn test_get_profile() {
        let mut conn = setup_db();
        let id = save_profile(&mut conn, &make_profile("Fast 7z")).unwrap();

        let loaded = get_profile(&mut conn, id as i32).unwrap().unwrap();
        assert_eq!(loaded.name, "Fast 7z");
        assert_eq!(loaded.compression_level, 9);
    }

    #[test]
    fn test_get_nonexistent() {
        let mut conn = setup_db();
        assert!(get_profile(&mut conn, 999).unwrap().is_none());
    }

    #[test]
    fn test_update_profile() {
        let mut conn = setup_db();
        let id = save_profile(&mut conn, &make_profile("Original")).unwrap();

        let mut updated = make_profile("Renamed");
        updated.id = Some(id);
        updated.compression_level = 5;
        save_profile(&mut conn, &updated).unwrap();

        let loaded = get_profile(&mut conn, id as i32).unwrap().unwrap();
        assert_eq!(loaded.name, "Renamed");
        assert_eq!(loaded.compression_level, 5);
    }

    #[test]
    fn test_delete_profile() {
        let mut conn = setup_db();
        let id = save_profile(&mut conn, &make_profile("Temp")).unwrap();
        assert!(get_profile(&mut conn, id as i32).unwrap().is_some());

        delete_profile(&mut conn, id as i32).unwrap();
        assert!(get_profile(&mut conn, id as i32).unwrap().is_none());
    }

    #[test]
    fn test_delete_system_profile_is_noop() {
        let mut conn = setup_db();
        let mut system = make_profile("System");
        system.is_system = true;
        let id = save_profile(&mut conn, &system).unwrap();

        delete_profile(&mut conn, id as i32).unwrap();
        // System profiles cannot be deleted
        assert!(get_profile(&mut conn, id as i32).unwrap().is_some());
    }

    #[test]
    fn test_set_default_profile() {
        let mut conn = setup_db();
        let id1 = save_profile(&mut conn, &make_profile("Profile A")).unwrap();
        let id2 = save_profile(&mut conn, &make_profile("Profile B")).unwrap();

        set_default_profile(&mut conn, id1 as i32).unwrap();
        assert!(
            get_profile(&mut conn, id1 as i32)
                .unwrap()
                .unwrap()
                .is_default
        );
        assert!(
            !get_profile(&mut conn, id2 as i32)
                .unwrap()
                .unwrap()
                .is_default
        );

        // Change default
        set_default_profile(&mut conn, id2 as i32).unwrap();
        assert!(
            !get_profile(&mut conn, id1 as i32)
                .unwrap()
                .unwrap()
                .is_default
        );
        assert!(
            get_profile(&mut conn, id2 as i32)
                .unwrap()
                .unwrap()
                .is_default
        );
    }

    #[test]
    fn test_get_default_profile() {
        let mut conn = setup_db();
        assert!(get_default_profile(&mut conn).unwrap().is_none());

        let mut profile = make_profile("Default");
        profile.is_default = true;
        save_profile(&mut conn, &profile).unwrap();

        let default = get_default_profile(&mut conn).unwrap().unwrap();
        assert_eq!(default.name, "Default");
    }

    #[test]
    fn test_save_default_clears_other_defaults() {
        let mut conn = setup_db();
        let mut p1 = make_profile("First Default");
        p1.is_default = true;
        save_profile(&mut conn, &p1).unwrap();

        let mut p2 = make_profile("Second Default");
        p2.is_default = true;
        save_profile(&mut conn, &p2).unwrap();

        // Only one default should exist
        let profiles = list_profiles(&mut conn).unwrap();
        let defaults: Vec<_> = profiles.iter().filter(|p| p.is_default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].name, "Second Default");
    }

    #[test]
    fn test_list_ordering() {
        let mut conn = setup_db();
        save_profile(&mut conn, &make_profile("Zebra")).unwrap();
        save_profile(&mut conn, &make_profile("Alpha")).unwrap();
        let mut def = make_profile("Middle");
        def.is_default = true;
        save_profile(&mut conn, &def).unwrap();

        let profiles = list_profiles(&mut conn).unwrap();
        // Default first, then alphabetical
        assert_eq!(profiles[0].name, "Middle");
        assert_eq!(profiles[1].name, "Alpha");
        assert_eq!(profiles[2].name, "Zebra");
    }
}
