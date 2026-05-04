//! UI configuration database layer.
//!
//! Provides normalized tables for UI items (toolbar, context menu, tools dialog),
//! regions, and display options with full CRUD support.

use crate::diesel_err;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

// ============================================================================
// Types
// ============================================================================

/// Display mode for UI elements
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayMode {
    #[default]
    IconAndText,
    IconOnly,
    TextOnly,
}

impl DisplayMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DisplayMode::IconAndText => "icon_and_text",
            DisplayMode::IconOnly => "icon_only",
            DisplayMode::TextOnly => "text_only",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "icon_only" => DisplayMode::IconOnly,
            "text_only" => DisplayMode::TextOnly,
            _ => DisplayMode::IconAndText,
        }
    }
}

/// Action type for UI items
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionType {
    #[default]
    Builtin,
    Plugin,
    Custom,
}

impl ActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionType::Builtin => "builtin",
            ActionType::Plugin => "plugin",
            ActionType::Custom => "custom",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "plugin" => ActionType::Plugin,
            "custom" => ActionType::Custom,
            _ => ActionType::Builtin,
        }
    }
}

/// UI region identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiRegion {
    Toolbar,
    ContextMenu,
    ToolsDialog,
    InfoPanel,
}

impl UiRegion {
    pub fn as_str(&self) -> &'static str {
        match self {
            UiRegion::Toolbar => "toolbar",
            UiRegion::ContextMenu => "context_menu",
            UiRegion::ToolsDialog => "tools_dialog",
            UiRegion::InfoPanel => "info_panel",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "toolbar" => Some(UiRegion::Toolbar),
            "context_menu" => Some(UiRegion::ContextMenu),
            "tools_dialog" => Some(UiRegion::ToolsDialog),
            "info_panel" => Some(UiRegion::InfoPanel),
            _ => None,
        }
    }
}

/// A UI item (button, menu item, etc.)
#[derive(Clone, Debug)]
pub struct UiItem {
    pub id: String,
    pub region: UiRegion,
    pub group_id: Option<String>,
    pub label: String,
    pub icon: Option<String>,
    pub visible: bool,
    pub sort_order: i32,
    pub display_mode: DisplayMode,
    pub action_type: ActionType,
    pub action_data: Option<String>,
}

/// Region-level configuration
#[derive(Clone, Debug)]
pub struct UiRegionConfig {
    pub id: String,
    pub enabled: bool,
    pub global_display_mode: Option<DisplayMode>,
}

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

// ============================================================================
// Default Seeding
// ============================================================================

/// Seed default toolbar items if table is empty
pub fn seed_defaults_if_empty(conn: &Connection) -> Result<()> {
    let count: i32 = conn.query_row("SELECT COUNT(*) FROM ui_items", [], |row| row.get(0))?;
    if count > 0 {
        return Ok(());
    }

    // Toolbar - Navigation group
    let navigation = [
        ("toolbar.back", "Back", "ARROW_LEFT"),
        ("toolbar.forward", "Forward", "ARROW_RIGHT"),
        ("toolbar.up", "Up", "ARROW_UP"),
    ];
    for (i, (id, label, icon)) in navigation.iter().enumerate() {
        upsert_item(
            conn,
            &UiItem {
                id: id.to_string(),
                region: UiRegion::Toolbar,
                group_id: Some("navigation".to_string()),
                label: label.to_string(),
                icon: Some(icon.to_string()),
                visible: true,
                sort_order: i as i32,
                display_mode: DisplayMode::IconOnly,
                action_type: ActionType::Builtin,
                action_data: None,
            },
        )?;
    }

    // Toolbar - File actions group
    let file_actions = [
        ("toolbar.open", "Open", "FOLDER_OPEN"),
        ("toolbar.extract", "Extract", "EXPORT"),
        ("toolbar.extract_all", "Extract All", "EXPORT"),
        ("toolbar.add", "Add", "PLUS"),
        ("toolbar.delete", "Delete", "TRASH"),
        ("toolbar.convert", "Convert...", "PACKAGE"),
        ("toolbar.batch_convert", "Batch Convert...", "FOLDER_PLUS"),
        ("toolbar.organize", "Organize", "FOLDERS"),
    ];
    for (i, (id, label, icon)) in file_actions.iter().enumerate() {
        upsert_item(
            conn,
            &UiItem {
                id: id.to_string(),
                region: UiRegion::Toolbar,
                group_id: Some("file_actions".to_string()),
                label: label.to_string(),
                icon: Some(icon.to_string()),
                visible: true,
                sort_order: (100 + i) as i32,
                display_mode: DisplayMode::IconAndText,
                action_type: ActionType::Builtin,
                action_data: None,
            },
        )?;
    }

    // Toolbar - View group
    let view = [
        ("toolbar.list_view", "List View", "LIST"),
        ("toolbar.grid_view", "Grid View", "GRID_FOUR"),
        ("toolbar.column_lock", "Lock Columns", "LOCK"),
    ];
    for (i, (id, label, icon)) in view.iter().enumerate() {
        upsert_item(
            conn,
            &UiItem {
                id: id.to_string(),
                region: UiRegion::Toolbar,
                group_id: Some("view".to_string()),
                label: label.to_string(),
                icon: Some(icon.to_string()),
                visible: true,
                sort_order: (200 + i) as i32,
                display_mode: DisplayMode::IconOnly,
                action_type: ActionType::Builtin,
                action_data: None,
            },
        )?;
    }

    // Toolbar - Panel toggles
    let panels = [
        ("toolbar.tree_panel", "Tree Panel", "TREE_STRUCTURE"),
        ("toolbar.properties_panel", "Properties", "INFO"),
    ];
    for (i, (id, label, icon)) in panels.iter().enumerate() {
        upsert_item(
            conn,
            &UiItem {
                id: id.to_string(),
                region: UiRegion::Toolbar,
                group_id: Some("panels".to_string()),
                label: label.to_string(),
                icon: Some(icon.to_string()),
                visible: true,
                sort_order: (300 + i) as i32,
                display_mode: DisplayMode::IconOnly,
                action_type: ActionType::Builtin,
                action_data: None,
            },
        )?;
    }

    // Context menu items
    let context_items = [
        ("context.open", "Open", "FOLDER_OPEN"),
        ("context.extract", "Extract", "EXPORT"),
        ("context.extract_to", "Extract To...", "EXPORT"),
        ("context.copy_path", "Copy Path", "COPY"),
        ("context.delete", "Delete", "TRASH"),
        ("context.properties", "Properties", "INFO"),
    ];
    for (i, (id, label, icon)) in context_items.iter().enumerate() {
        upsert_item(
            conn,
            &UiItem {
                id: id.to_string(),
                region: UiRegion::ContextMenu,
                group_id: None,
                label: label.to_string(),
                icon: Some(icon.to_string()),
                visible: true,
                sort_order: i as i32,
                display_mode: DisplayMode::IconAndText,
                action_type: ActionType::Builtin,
                action_data: None,
            },
        )?;
    }

    // Info panel sections
    let info_sections = [
        ("info.archive", "Archive Info"),
        ("info.file", "File Info"),
        ("info.attributes", "Attributes"),
    ];
    for (i, (id, label)) in info_sections.iter().enumerate() {
        upsert_item(
            conn,
            &UiItem {
                id: id.to_string(),
                region: UiRegion::InfoPanel,
                group_id: None,
                label: label.to_string(),
                icon: None,
                visible: true,
                sort_order: i as i32,
                display_mode: DisplayMode::TextOnly,
                action_type: ActionType::Builtin,
                action_data: None,
            },
        )?;
    }

    // Default display options
    set_display_option(conn, "default_view_mode", "list")?;
    set_display_option(conn, "tree_panel_visible", "true")?;
    set_display_option(conn, "tree_panel_width", "200")?;
    set_display_option(conn, "properties_panel_visible", "true")?;
    set_display_option(conn, "properties_panel_width", "280")?;

    Ok(())
}

// ============================================================================
// Diesel DSL versions
// ============================================================================

use diesel::prelude::*;

/// Diesel-compatible UI item row
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::diesel_schema::ui_items)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbUiItemRow {
    pub id: String,
    pub region: String,
    pub group_id: Option<String>,
    pub label: String,
    pub icon: Option<String>,
    pub visible: bool,
    pub sort_order: i32,
    pub action_type: String,
    pub action_data: Option<String>,
    pub display_mode: String,
}

impl DbUiItemRow {
    pub fn to_ui_item(&self) -> UiItem {
        UiItem {
            id: self.id.clone(),
            region: UiRegion::from_str(&self.region).unwrap_or(UiRegion::Toolbar),
            group_id: self.group_id.clone(),
            label: self.label.clone(),
            icon: self.icon.clone(),
            visible: self.visible,
            sort_order: self.sort_order,
            display_mode: DisplayMode::from_str(&self.display_mode),
            action_type: ActionType::from_str(&self.action_type),
            action_data: self.action_data.clone(),
        }
    }
}

/// List items by region using Diesel DSL
pub fn list_items_by_region_diesel(
    conn: &mut diesel::SqliteConnection,
    reg: UiRegion,
) -> Result<Vec<UiItem>> {
    use crate::diesel_schema::ui_items::dsl::*;

    let rows = ui_items
        .filter(region.eq(reg.as_str()))
        .order(sort_order.asc())
        .load::<DbUiItemRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(rows.iter().map(|r| r.to_ui_item()).collect())
}

/// Get item by ID using Diesel DSL
pub fn get_item_diesel(
    conn: &mut diesel::SqliteConnection,
    item_id: &str,
) -> Result<Option<UiItem>> {
    use crate::diesel_schema::ui_items::dsl::*;
    use diesel::result::OptionalExtension;

    let row = ui_items
        .filter(id.eq(item_id))
        .first::<DbUiItemRow>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(row.map(|r| r.to_ui_item()))
}

/// Upsert item using Diesel DSL
pub fn upsert_item_diesel(conn: &mut diesel::SqliteConnection, item: &UiItem) -> Result<()> {
    use crate::diesel_schema::ui_items::dsl::*;

    diesel::insert_into(ui_items)
        .values((
            id.eq(&item.id),
            region.eq(item.region.as_str()),
            group_id.eq(&item.group_id),
            label.eq(&item.label),
            icon.eq(&item.icon),
            visible.eq(item.visible),
            sort_order.eq(item.sort_order),
            display_mode.eq(item.display_mode.as_str()),
            action_type.eq(item.action_type.as_str()),
            action_data.eq(&item.action_data),
        ))
        .on_conflict(id)
        .do_update()
        .set((
            region.eq(item.region.as_str()),
            group_id.eq(&item.group_id),
            label.eq(&item.label),
            icon.eq(&item.icon),
            visible.eq(item.visible),
            sort_order.eq(item.sort_order),
            display_mode.eq(item.display_mode.as_str()),
            action_type.eq(item.action_type.as_str()),
            action_data.eq(&item.action_data),
        ))
        .execute(conn)
        .map_err(diesel_err("upsert"))?;

    Ok(())
}

/// Delete item using Diesel DSL
pub fn delete_item_diesel(conn: &mut diesel::SqliteConnection, item_id: &str) -> Result<()> {
    use crate::diesel_schema::ui_items::dsl::*;

    diesel::delete(ui_items.filter(id.eq(item_id)))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Set item visibility using Diesel DSL
pub fn set_item_visibility_diesel(
    conn: &mut diesel::SqliteConnection,
    item_id: &str,
    is_visible: bool,
) -> Result<()> {
    use crate::diesel_schema::ui_items::dsl::*;

    diesel::update(ui_items.filter(id.eq(item_id)))
        .set(visible.eq(is_visible))
        .execute(conn)
        .map_err(diesel_err("update"))?;

    Ok(())
}

/// Get display option using Diesel DSL
pub fn get_display_option_diesel(
    conn: &mut diesel::SqliteConnection,
    opt_key: &str,
) -> Result<Option<String>> {
    use crate::diesel_schema::ui_display_options::dsl::*;
    use diesel::result::OptionalExtension;

    let result = ui_display_options
        .filter(key.eq(opt_key))
        .select(value)
        .first::<String>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(result)
}

/// Set display option using Diesel DSL
pub fn set_display_option_diesel(
    conn: &mut diesel::SqliteConnection,
    opt_key: &str,
    opt_value: &str,
) -> Result<()> {
    use crate::diesel_schema::ui_display_options::dsl::*;

    diesel::insert_into(ui_display_options)
        .values((key.eq(opt_key), value.eq(opt_value)))
        .on_conflict(key)
        .do_update()
        .set(value.eq(opt_value))
        .execute(conn)
        .map_err(diesel_err("insert"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
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
