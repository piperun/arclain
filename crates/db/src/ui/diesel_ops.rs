//! Diesel-DSL accessors for the UI configuration tables.
//!
//! Used by callers that hold a `DieselPool` (i.e.
//! `core::services::ui_service`). The startup-path API
//! (`ensure_ui_tables`, `upsert_item`, `set_display_option`) lives in
//! `config.rs` — see that module for why those two CRUD helpers stay
//! rusqlite-flavoured.

use super::types::{ActionType, DisplayMode, UiItem, UiRegion};
use crate::diesel_err;
use anyhow::Result;
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

/// List items by region
pub fn list_items_by_region(
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

/// Upsert item (Diesel mirror of [`super::config::upsert_item`])
pub fn upsert_item(conn: &mut diesel::SqliteConnection, item: &UiItem) -> Result<()> {
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

/// Delete item
pub fn delete_item(conn: &mut diesel::SqliteConnection, item_id: &str) -> Result<()> {
    use crate::diesel_schema::ui_items::dsl::*;

    diesel::delete(ui_items.filter(id.eq(item_id)))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Get display option
pub fn get_display_option(
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

/// Set display option (Diesel mirror of
/// [`super::config::set_display_option`])
pub fn set_display_option(
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
    use super::super::types::ActionType;
    use super::*;
    use diesel::Connection;
    use diesel::RunQueryDsl;

    fn setup_db() -> diesel::SqliteConnection {
        let mut conn = diesel::SqliteConnection::establish(":memory:").expect("in-memory SQLite");
        diesel::sql_query(
            "CREATE TABLE ui_items (
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
            )",
        )
        .execute(&mut conn)
        .unwrap();
        diesel::sql_query(
            "CREATE TABLE ui_display_options (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&mut conn)
        .unwrap();
        conn
    }

    #[test]
    fn test_item_crud() {
        let mut conn = setup_db();

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

        upsert_item(&mut conn, &item).unwrap();

        // list_items_by_region returns it
        let items = list_items_by_region(&mut conn, UiRegion::ToolsDialog).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Test Item");
        assert_eq!(items[0].action_type, ActionType::Custom);

        delete_item(&mut conn, "test.item").unwrap();
        let items = list_items_by_region(&mut conn, UiRegion::ToolsDialog).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_display_options() {
        let mut conn = setup_db();

        set_display_option(&mut conn, "test_key", "test_value").unwrap();
        let val = get_display_option(&mut conn, "test_key").unwrap();
        assert_eq!(val, Some("test_value".to_string()));

        set_display_option(&mut conn, "test_key", "new_value").unwrap();
        let val = get_display_option(&mut conn, "test_key").unwrap();
        assert_eq!(val, Some("new_value".to_string()));
    }
}
