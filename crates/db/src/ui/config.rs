//! Rusqlite-backed CRUD for the UI configuration tables.
//!
//! Three table groups: `ui_items` (toolbar / context-menu / tools-
//! dialog / info-panel entries), `ui_regions` (per-region overrides),
//! and `ui_display_options` (key-value pairs). The Diesel DSL mirror
//! of these operations lives in `diesel_ops`; the canonical seed
//! values are in `seed`.

use super::types::{ActionType, DisplayMode, UiItem, UiRegion, UiRegionConfig};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

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

/// Initialize UI config tables
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
// UI Items CRUD
// ============================================================================

/// List all items in a region, ordered by sort_order
pub fn list_items_by_region(conn: &Connection, region: UiRegion) -> Result<Vec<UiItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, region, group_id, label, icon, visible, sort_order,
                display_mode, action_type, action_data
         FROM ui_items
         WHERE region = ?1
         ORDER BY sort_order ASC",
    )?;

    let rows = stmt.query_map([region.as_str()], |row| {
        Ok(UiItem {
            id: row.get(0)?,
            region,
            group_id: row.get(2)?,
            label: row.get(3)?,
            icon: row.get(4)?,
            visible: row.get::<_, i32>(5)? != 0,
            sort_order: row.get(6)?,
            display_mode: DisplayMode::from_str(&row.get::<_, String>(7)?),
            action_type: ActionType::from_str(&row.get::<_, String>(8)?),
            action_data: row.get(9)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("reading ui_items rows")
}

/// Get a single item by ID
pub fn get_item(conn: &Connection, id: &str) -> Result<Option<UiItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, region, group_id, label, icon, visible, sort_order,
                display_mode, action_type, action_data
         FROM ui_items WHERE id = ?1",
    )?;

    stmt.query_row([id], |row| {
        let region_str: String = row.get(1)?;
        Ok(UiItem {
            id: row.get(0)?,
            region: UiRegion::from_str(&region_str).unwrap_or(UiRegion::Toolbar),
            group_id: row.get(2)?,
            label: row.get(3)?,
            icon: row.get(4)?,
            visible: row.get::<_, i32>(5)? != 0,
            sort_order: row.get(6)?,
            display_mode: DisplayMode::from_str(&row.get::<_, String>(7)?),
            action_type: ActionType::from_str(&row.get::<_, String>(8)?),
            action_data: row.get(9)?,
        })
    })
    .optional()
    .context("getting ui_item")
}

/// Insert or update a UI item
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

/// Update visibility only
pub fn set_item_visibility(conn: &Connection, id: &str, visible: bool) -> Result<()> {
    conn.execute(
        "UPDATE ui_items SET visible = ?2 WHERE id = ?1",
        params![id, visible as i32],
    )?;
    Ok(())
}

/// Update sort order only
pub fn set_item_order(conn: &Connection, id: &str, sort_order: i32) -> Result<()> {
    conn.execute(
        "UPDATE ui_items SET sort_order = ?2 WHERE id = ?1",
        params![id, sort_order],
    )?;
    Ok(())
}

/// Update display mode only
pub fn set_item_display_mode(conn: &Connection, id: &str, mode: DisplayMode) -> Result<()> {
    conn.execute(
        "UPDATE ui_items SET display_mode = ?2 WHERE id = ?1",
        params![id, mode.as_str()],
    )?;
    Ok(())
}

/// Delete an item
pub fn delete_item(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM ui_items WHERE id = ?1", [id])?;
    Ok(())
}

// ============================================================================
// UI Regions CRUD
// ============================================================================

/// Get region config
pub fn get_region_config(conn: &Connection, id: &str) -> Result<Option<UiRegionConfig>> {
    let mut stmt =
        conn.prepare("SELECT id, enabled, global_display_mode FROM ui_regions WHERE id = ?1")?;

    stmt.query_row([id], |row| {
        Ok(UiRegionConfig {
            id: row.get(0)?,
            enabled: row.get::<_, i32>(1)? != 0,
            global_display_mode: row
                .get::<_, Option<String>>(2)?
                .map(|s| DisplayMode::from_str(&s)),
        })
    })
    .optional()
    .context("getting ui_region")
}

/// Upsert region config
pub fn upsert_region_config(conn: &Connection, config: &UiRegionConfig) -> Result<()> {
    conn.execute(
        "INSERT INTO ui_regions (id, enabled, global_display_mode)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET
             enabled = excluded.enabled,
             global_display_mode = excluded.global_display_mode",
        params![
            config.id,
            config.enabled as i32,
            config.global_display_mode.map(|m| m.as_str()),
        ],
    )?;
    Ok(())
}

// ============================================================================
// Display Options (Key-Value)
// ============================================================================

/// Get a display option value
pub fn get_display_option(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM ui_display_options WHERE key = ?1")?;
    stmt.query_row([key], |row| row.get(0))
        .optional()
        .context("getting display option")
}

/// Set a display option value
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
    use super::super::seed::seed_defaults_if_empty;
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_ui_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn test_seed_defaults() {
        let conn = setup_test_db();
        seed_defaults_if_empty(&conn).unwrap();

        let items = list_items_by_region(&conn, UiRegion::Toolbar).unwrap();
        assert!(!items.is_empty(), "should have toolbar items");

        let context = list_items_by_region(&conn, UiRegion::ContextMenu).unwrap();
        assert!(!context.is_empty(), "should have context menu items");
    }

    #[test]
    fn test_item_crud() {
        let conn = setup_test_db();

        let item = UiItem {
            id: "test.item".to_string(),
            region: UiRegion::ToolsDialog,
            group_id: None,
            label: "Test Item".to_string(),
            icon: Some("STAR".to_string()),
            visible: true,
            sort_order: 0,
            display_mode: DisplayMode::IconAndText,
            action_type: ActionType::Custom,
            action_data: Some("echo hello".to_string()),
        };

        upsert_item(&conn, &item).unwrap();

        let loaded = get_item(&conn, "test.item").unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.label, "Test Item");
        assert_eq!(loaded.action_type, ActionType::Custom);

        set_item_visibility(&conn, "test.item", false).unwrap();
        let loaded = get_item(&conn, "test.item").unwrap().unwrap();
        assert!(!loaded.visible);

        delete_item(&conn, "test.item").unwrap();
        let loaded = get_item(&conn, "test.item").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_display_options() {
        let conn = setup_test_db();

        set_display_option(&conn, "test_key", "test_value").unwrap();
        let val = get_display_option(&conn, "test_key").unwrap();
        assert_eq!(val, Some("test_value".to_string()));

        set_display_option(&conn, "test_key", "new_value").unwrap();
        let val = get_display_option(&conn, "test_key").unwrap();
        assert_eq!(val, Some("new_value".to_string()));
    }
}
