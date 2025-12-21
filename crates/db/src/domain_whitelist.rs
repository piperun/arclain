//! Domain whitelist storage for plugin network access control
//!
//! Stores which domains each plugin is allowed to access.

use anyhow::Result;
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

/// Ensure the domain_whitelist table exists
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

/// List all whitelist entries
pub fn list_whitelist_entries(conn: &Connection) -> Result<Vec<DbWhitelistEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, plugin_id, domain, approved FROM domain_whitelist ORDER BY plugin_id, domain",
    )?;

    let entries = stmt
        .query_map([], |row| {
            Ok(DbWhitelistEntry {
                id: Some(row.get(0)?),
                plugin_id: row.get(1)?,
                domain: row.get(2)?,
                approved: row.get::<_, i32>(3)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

/// List whitelist entries for a specific plugin
pub fn list_plugin_domains(conn: &Connection, plugin_id: &str) -> Result<Vec<DbWhitelistEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, plugin_id, domain, approved 
         FROM domain_whitelist 
         WHERE plugin_id = ?1
         ORDER BY domain",
    )?;

    let entries = stmt
        .query_map([plugin_id], |row| {
            Ok(DbWhitelistEntry {
                id: Some(row.get(0)?),
                plugin_id: row.get(1)?,
                domain: row.get(2)?,
                approved: row.get::<_, i32>(3)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

/// Add or update a whitelist entry (upsert)
pub fn upsert_whitelist_entry(conn: &Connection, entry: &DbWhitelistEntry) -> Result<i64> {
    let approved_at = if entry.approved {
        "CURRENT_TIMESTAMP"
    } else {
        "NULL"
    };

    conn.execute(
        &format!(
            "INSERT INTO domain_whitelist (plugin_id, domain, approved, approved_at)
             VALUES (?1, ?2, ?3, {})
             ON CONFLICT(plugin_id, domain) DO UPDATE SET
                approved = excluded.approved,
                approved_at = CASE WHEN excluded.approved = 1 THEN CURRENT_TIMESTAMP ELSE approved_at END",
            approved_at
        ),
        rusqlite::params![entry.plugin_id, entry.domain, entry.approved as i32],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Approve a domain for a plugin
pub fn approve_domain(conn: &Connection, plugin_id: &str, domain: &str) -> Result<()> {
    conn.execute(
        "UPDATE domain_whitelist 
         SET approved = 1, approved_at = CURRENT_TIMESTAMP
         WHERE plugin_id = ?1 AND domain = ?2",
        [plugin_id, domain],
    )?;

    // If no rows updated, insert new approved entry
    if conn.changes() == 0 {
        conn.execute(
            "INSERT INTO domain_whitelist (plugin_id, domain, approved, approved_at)
             VALUES (?1, ?2, 1, CURRENT_TIMESTAMP)",
            [plugin_id, domain],
        )?;
    }

    Ok(())
}

/// Revoke a domain for a plugin
pub fn revoke_domain(conn: &Connection, plugin_id: &str, domain: &str) -> Result<()> {
    conn.execute(
        "UPDATE domain_whitelist SET approved = 0 WHERE plugin_id = ?1 AND domain = ?2",
        [plugin_id, domain],
    )?;
    Ok(())
}

/// Delete a whitelist entry entirely
pub fn delete_whitelist_entry(conn: &Connection, plugin_id: &str, domain: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM domain_whitelist WHERE plugin_id = ?1 AND domain = ?2",
        [plugin_id, domain],
    )?;
    Ok(())
}

/// Delete all entries for a plugin
pub fn delete_plugin_whitelist(conn: &Connection, plugin_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM domain_whitelist WHERE plugin_id = ?1",
        [plugin_id],
    )?;
    Ok(())
}

/// Check if a domain is approved for a plugin
pub fn is_domain_approved(conn: &Connection, plugin_id: &str, domain: &str) -> Result<bool> {
    let result: Option<i32> = conn
        .query_row(
            "SELECT approved FROM domain_whitelist WHERE plugin_id = ?1 AND domain = ?2",
            [plugin_id, domain],
            |row| row.get(0),
        )
        .ok();

    Ok(result == Some(1))
}

/// Check if a domain exists (approved or pending) for a plugin
pub fn domain_exists(conn: &Connection, plugin_id: &str, domain: &str) -> Result<bool> {
    let result: Option<i64> = conn
        .query_row(
            "SELECT id FROM domain_whitelist WHERE plugin_id = ?1 AND domain = ?2",
            [plugin_id, domain],
            |row| row.get(0),
        )
        .ok();

    Ok(result.is_some())
}

/// Get pending (unapproved) entries across all plugins
pub fn list_pending_approvals(conn: &Connection) -> Result<Vec<DbWhitelistEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, plugin_id, domain, approved 
         FROM domain_whitelist 
         WHERE approved = 0
         ORDER BY plugin_id, domain",
    )?;

    let entries = stmt
        .query_map([], |row| {
            Ok(DbWhitelistEntry {
                id: Some(row.get(0)?),
                plugin_id: row.get(1)?,
                domain: row.get(2)?,
                approved: row.get::<_, i32>(3)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_whitelist_table(&conn).unwrap();
        conn
    }

    #[test]
    fn test_add_and_list() {
        let conn = setup_db();

        let entry = DbWhitelistEntry::pending("test-plugin", "dlsite.com");
        upsert_whitelist_entry(&conn, &entry).unwrap();

        let entries = list_whitelist_entries(&conn).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_id, "test-plugin");
        assert_eq!(entries[0].domain, "dlsite.com");
        assert!(!entries[0].approved);
    }

    #[test]
    fn test_approve() {
        let conn = setup_db();

        let entry = DbWhitelistEntry::pending("test-plugin", "dlsite.com");
        upsert_whitelist_entry(&conn, &entry).unwrap();

        assert!(!is_domain_approved(&conn, "test-plugin", "dlsite.com").unwrap());

        approve_domain(&conn, "test-plugin", "dlsite.com").unwrap();

        assert!(is_domain_approved(&conn, "test-plugin", "dlsite.com").unwrap());
    }

    #[test]
    fn test_revoke() {
        let conn = setup_db();

        approve_domain(&conn, "test-plugin", "example.com").unwrap();
        assert!(is_domain_approved(&conn, "test-plugin", "example.com").unwrap());

        revoke_domain(&conn, "test-plugin", "example.com").unwrap();
        assert!(!is_domain_approved(&conn, "test-plugin", "example.com").unwrap());
    }

    #[test]
    fn test_pending_list() {
        let conn = setup_db();

        upsert_whitelist_entry(&conn, &DbWhitelistEntry::pending("p1", "a.com")).unwrap();
        upsert_whitelist_entry(&conn, &DbWhitelistEntry::approved("p1", "b.com")).unwrap();
        upsert_whitelist_entry(&conn, &DbWhitelistEntry::pending("p2", "c.com")).unwrap();

        let pending = list_pending_approvals(&conn).unwrap();
        assert_eq!(pending.len(), 2);
    }
}
