//! Diesel-DSL accessors for the UI configuration tables.
//!
//! Mirror of the rusqlite functions in `config.rs` but operating
//! through the diesel connection pool. Used by callers that already
//! hold a `DieselPool` and don't want to roundtrip through a separate
//! rusqlite connection.

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
