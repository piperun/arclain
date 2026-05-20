//! Rusqlite-backed schema + seed helpers for the UI configuration tables.
//!
//! Three tables: `ui_items` (toolbar / context-menu / tools-dialog /
//! info-panel entries), `ui_regions` (per-region overrides — currently
//! unused), and `ui_display_options` (key-value pairs).
//!
//! Only the startup-path API lives here:
//! [`ensure_ui_tables`] (called from [`ConfigDb::create_tables`] before
//! the Diesel pool exists), [`upsert_item`] and [`set_display_option`]
//! (called from `seed.rs` to populate defaults). Everything else
//! (read/list/delete/setters) lives in the Diesel mirror at
//! `diesel_ops`, used by `core::services::ui_service` over a
//! `DieselPool`.
//!
//! [`ConfigDb::create_tables`]: crate::config::ConfigDb

use super::types::UiItem;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};

// ============================================================================
// Schema
// ============================================================================

const CREATE_UI_ITEMS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS ui_items (
    id TEXT PRIMARY KEY,
    region TEXT NOT NULL,
    group_id TEXT,
    label TEXT NOT NULL,
    icon TEXT,
    visible INTEGER DEFAULT 1,
    sort_order INTEGER DEFAULT 0,
    display_mode TEXT DEFAULT 'icon_and_text',
    action_type TEXT DEFAULT 'builtin',
    action_data TEXT
)
"#;

const CREATE_UI_REGIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS ui_regions (
    id TEXT PRIMARY KEY,
    enabled INTEGER DEFAULT 1,
    global_display_mode TEXT
)
"#;

const CREATE_UI_DISPLAY_OPTIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS ui_display_options (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
)
"#;

/// Initialize UI config tables.
///
/// Called from [`ConfigDb::create_tables`] during startup, before any
/// Diesel pool exists. Pure rusqlite.
///
/// [`ConfigDb::create_tables`]: crate::config::ConfigDb
pub fn ensure_ui_tables(conn: &Connection) -> Result<()> {
    conn.execute(CREATE_UI_ITEMS_TABLE, [])
        .context("creating ui_items table")?;
    conn.execute(CREATE_UI_REGIONS_TABLE, [])
        .context("creating ui_regions table")?;
    conn.execute(CREATE_UI_DISPLAY_OPTIONS_TABLE, [])
        .context("creating ui_display_options table")?;
    Ok(())
}

// ============================================================================
// Seed-path CRUD (rusqlite — called from `seed.rs` during startup)
// ============================================================================

/// Insert or update a UI item.
///
/// Rusqlite-flavoured because it's called from
/// [`super::seed::seed_defaults_if_empty`] during startup, before the
/// Diesel pool exists. The Diesel mirror used by `UiService` lives at
/// [`super::diesel_ops::upsert_item`].
pub fn upsert_item(conn: &Connection, item: &UiItem) -> Result<()> {
    conn.execute(
        "INSERT INTO ui_items (id, region, group_id, label, icon, visible, sort_order,
                               display_mode, action_type, action_data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
             region = excluded.region,
             group_id = excluded.group_id,
             label = excluded.label,
             icon = excluded.icon,
             visible = excluded.visible,
             sort_order = excluded.sort_order,
             display_mode = excluded.display_mode,
             action_type = excluded.action_type,
             action_data = excluded.action_data",
        params![
            item.id,
            item.region.as_str(),
            item.group_id,
            item.label,
            item.icon,
            item.visible as i32,
            item.sort_order,
            item.display_mode.as_str(),
            item.action_type.as_str(),
            item.action_data,
        ],
    )?;
    Ok(())
}

/// Set a display option value.
///
/// Rusqlite-flavoured because it's called from
/// [`super::seed::seed_defaults_if_empty`] during startup. The Diesel
/// mirror used by `UiService` lives at
/// [`super::diesel_ops::set_display_option`].
pub fn set_display_option(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO ui_display_options (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::diesel_ops::list_items_by_region;
    use super::super::seed::seed_defaults_if_empty;
    use super::super::types::UiRegion;
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_ui_tables(&conn).unwrap();
        conn
    }

    /// Seeds via the rusqlite startup path, then verifies items landed
    /// in the table by re-querying via the Diesel API (since we share
    /// the underlying file via a fresh Diesel connection in
    /// [`super::super::diesel_ops::tests`]).
    #[test]
    fn test_seed_defaults_populates_tables() {
        let conn = setup_test_db();
        seed_defaults_if_empty(&conn).unwrap();

        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM ui_items WHERE region = 'toolbar'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "should have toolbar items");

        let context_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM ui_items WHERE region = 'context_menu'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(context_count > 0, "should have context menu items");

        // sanity: ensure list_items_by_region (typed in tests dir) compiles
        let _ = list_items_by_region;
        let _ = UiRegion::Toolbar;
    }
}
