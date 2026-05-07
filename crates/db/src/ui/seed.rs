//! Default UI configuration seeding
//!
//! Populates the freshly-created UI tables with the canonical set of
//! built-in toolbar entries, context-menu items, info-panel sections,
//! and display options. Idempotent — bails out if `ui_items` already
//! has rows.

use super::config::{set_display_option, upsert_item};
use super::types::{ActionType, DisplayMode, UiItem, UiRegion};
use anyhow::Result;
use rusqlite::Connection;

/// Seed default toolbar items if table is empty.
pub fn seed_defaults_if_empty(conn: &Connection) -> Result<()> {
    let count: i32 = conn.query_row("SELECT COUNT(*) FROM ui_items", [], |row| row.get(0))?;
    if count > 0 {
        return Ok(());
    }

    seed_toolbar(conn)?;
    seed_context_menu(conn)?;
    seed_info_panel(conn)?;
    seed_default_display_options(conn)?;

    Ok(())
}

fn seed_toolbar(conn: &Connection) -> Result<()> {
    // Navigation group — icon-only chips.
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

    // File-action group — icon + text.
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

    // View group — icon-only.
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

    // Panel-toggle group.
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

    Ok(())
}

fn seed_context_menu(conn: &Connection) -> Result<()> {
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
    Ok(())
}

fn seed_info_panel(conn: &Connection) -> Result<()> {
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
    Ok(())
}

fn seed_default_display_options(conn: &Connection) -> Result<()> {
    set_display_option(conn, "default_view_mode", "list")?;
    set_display_option(conn, "tree_panel_visible", "true")?;
    set_display_option(conn, "tree_panel_width", "200")?;
    set_display_option(conn, "properties_panel_visible", "true")?;
    set_display_option(conn, "properties_panel_width", "280")?;
    Ok(())
}
