//! Domain whitelist storage for plugin network access control
//!
//! Stores which domains each plugin is allowed to access.

use crate::diesel_err;
use anyhow::Result;
use diesel::prelude::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// A whitelist entry for a plugin's domain access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbWhitelistEntry {
    /// Row ID (for updates/deletes)
    pub id: Option<i64>,
    /// The plugin that requested this domain
    pub plugin_id: String,
    /// The domain (e.g., "dlsite.com")
    pub domain: String,
    /// Whether the user has approved this domain
    pub approved: bool,
}

/// Diesel-compatible query row for whitelist entries
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::diesel_schema::domain_whitelist)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbWhitelistRow {
    pub id: i32,
    pub plugin_id: String,
    pub domain: String,
    pub approved: bool,
    pub approved_at: Option<String>,
}

impl DbWhitelistEntry {
    /// Create a new pending (unapproved) entry
    pub fn pending(plugin_id: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            id: None,
            plugin_id: plugin_id.into(),
            domain: domain.into(),
            approved: false,
        }
    }

    /// Create a new approved entry
    pub fn approved(plugin_id: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            id: None,
            plugin_id: plugin_id.into(),
            domain: domain.into(),
            approved: true,
        }
    }
}

fn row_to_entry(r: DbWhitelistRow) -> DbWhitelistEntry {
    DbWhitelistEntry {
        id: Some(r.id as i64),
        plugin_id: r.plugin_id,
        domain: r.domain,
        approved: r.approved,
    }
}

/// Ensure the domain_whitelist table exists.
///
/// Rusqlite-flavoured because it's called from
/// [`ConfigDb::create_tables`] during startup, before any Diesel pool
/// exists. All CRUD on this table uses Diesel.
pub fn ensure_whitelist_table(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS domain_whitelist (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plugin_id TEXT NOT NULL,
            domain TEXT NOT NULL,
            approved INTEGER NOT NULL DEFAULT 0,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP,
            approved_at TEXT,
            UNIQUE(plugin_id, domain)
        )",
        [],
    )?;

    // Index for fast plugin lookups
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_whitelist_plugin ON domain_whitelist(plugin_id)",
        [],
    )?;

    Ok(())
}

// ============================================================================
// Diesel DSL CRUD
// ============================================================================

/// List all whitelist entries
pub fn list_whitelist_entries(
    conn: &mut diesel::SqliteConnection,
) -> Result<Vec<DbWhitelistEntry>> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    let rows = domain_whitelist
        .order((plugin_id.asc(), domain.asc()))
        .load::<DbWhitelistRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(rows.into_iter().map(row_to_entry).collect())
}

/// List whitelist entries for a specific plugin
pub fn list_plugin_domains(
    conn: &mut diesel::SqliteConnection,
    pid: &str,
) -> Result<Vec<DbWhitelistEntry>> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    let rows = domain_whitelist
        .filter(plugin_id.eq(pid))
        .order(domain.asc())
        .load::<DbWhitelistRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(rows.into_iter().map(row_to_entry).collect())
}

/// Add or update a whitelist entry (upsert)
pub fn upsert_whitelist_entry(
    conn: &mut diesel::SqliteConnection,
    entry: &DbWhitelistEntry,
) -> Result<()> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    diesel::insert_into(domain_whitelist)
        .values((
            plugin_id.eq(&entry.plugin_id),
            domain.eq(&entry.domain),
            approved.eq(entry.approved),
            approved_at.eq(if entry.approved {
                Some(chrono::Utc::now().to_rfc3339())
            } else {
                None
            }),
        ))
        .on_conflict((plugin_id, domain))
        .do_update()
        .set((
            approved.eq(entry.approved),
            approved_at.eq(if entry.approved {
                Some(chrono::Utc::now().to_rfc3339())
            } else {
                None
            }),
        ))
        .execute(conn)
        .map_err(diesel_err("upsert"))?;

    Ok(())
}

/// Approve a domain for a plugin (insert-or-update)
pub fn approve_domain(conn: &mut diesel::SqliteConnection, pid: &str, dom: &str) -> Result<()> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    diesel::insert_into(domain_whitelist)
        .values((
            plugin_id.eq(pid),
            domain.eq(dom),
            approved.eq(true),
            approved_at.eq(Some(chrono::Utc::now().to_rfc3339())),
        ))
        .on_conflict((plugin_id, domain))
        .do_update()
        .set((
            approved.eq(true),
            approved_at.eq(Some(chrono::Utc::now().to_rfc3339())),
        ))
        .execute(conn)
        .map_err(diesel_err("approve"))?;

    Ok(())
}

/// Revoke a domain for a plugin
pub fn revoke_domain(conn: &mut diesel::SqliteConnection, pid: &str, dom: &str) -> Result<()> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    diesel::update(domain_whitelist.filter(plugin_id.eq(pid).and(domain.eq(dom))))
        .set(approved.eq(false))
        .execute(conn)
        .map_err(diesel_err("revoke"))?;

    Ok(())
}

/// Delete a whitelist entry entirely
pub fn delete_whitelist_entry(
    conn: &mut diesel::SqliteConnection,
    pid: &str,
    dom: &str,
) -> Result<()> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    diesel::delete(domain_whitelist.filter(plugin_id.eq(pid).and(domain.eq(dom))))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Delete all entries for a plugin
pub fn delete_plugin_whitelist(conn: &mut diesel::SqliteConnection, pid: &str) -> Result<()> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    diesel::delete(domain_whitelist.filter(plugin_id.eq(pid)))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Check if a domain is approved for a plugin
pub fn is_domain_approved(
    conn: &mut diesel::SqliteConnection,
    pid: &str,
    dom: &str,
) -> Result<bool> {
    use crate::diesel_schema::domain_whitelist::dsl::*;
    use diesel::result::OptionalExtension;

    let result = domain_whitelist
        .filter(plugin_id.eq(pid).and(domain.eq(dom)))
        .select(approved)
        .first::<bool>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(result == Some(true))
}

/// Check if a domain exists (approved or pending) for a plugin
pub fn domain_exists(conn: &mut diesel::SqliteConnection, pid: &str, dom: &str) -> Result<bool> {
    use crate::diesel_schema::domain_whitelist::dsl::*;
    use diesel::result::OptionalExtension;

    let result = domain_whitelist
        .filter(plugin_id.eq(pid).and(domain.eq(dom)))
        .select(id)
        .first::<i32>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(result.is_some())
}

/// Get pending (unapproved) entries across all plugins
pub fn list_pending_approvals(
    conn: &mut diesel::SqliteConnection,
) -> Result<Vec<DbWhitelistEntry>> {
    use crate::diesel_schema::domain_whitelist::dsl::*;

    let rows = domain_whitelist
        .filter(approved.eq(false))
        .order((plugin_id.asc(), domain.asc()))
        .load::<DbWhitelistRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(rows.into_iter().map(row_to_entry).collect())
}
