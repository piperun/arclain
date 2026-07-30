//! UI Service for managing UI configuration items
//!
//! Wraps arclain_db UI item operations with connection pool management.
//! Centralizes access to toolbar items, info panel items, and display options.

use anyhow::Result;
use arclain_db::{
    delete_item, get_display_option, list_items_by_region, set_display_option, set_display_options,
    sync_host_item, upsert_item, DieselPool, UiItem, UiRegion,
};

/// Service for managing UI configuration items
#[derive(Clone)]
pub struct UiService {
    pool: DieselPool,
}

impl UiService {
    /// Create a new UI service with the given connection pool
    pub fn new(pool: DieselPool) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying pool (for advanced use cases)
    pub fn pool(&self) -> &DieselPool {
        &self.pool
    }

    // =========================================================================
    // UI Item Operations
    // =========================================================================

    /// List all UI items for a specific region (Toolbar, InfoPanel, etc.)
    pub fn list_items(&self, region: UiRegion) -> Result<Vec<UiItem>> {
        self.pool
            .with_conn(|conn| list_items_by_region(conn, region))
    }

    /// List toolbar items
    pub fn list_toolbar_items(&self) -> Result<Vec<UiItem>> {
        self.list_items(UiRegion::Toolbar)
    }

    /// List info panel items
    pub fn list_info_panel_items(&self) -> Result<Vec<UiItem>> {
        self.list_items(UiRegion::InfoPanel)
    }

    /// Upsert (insert or update) a UI item
    pub fn upsert_item(&self, item: &UiItem) -> Result<()> {
        self.pool.with_conn(|conn| upsert_item(conn, item))
    }

    /// Delete a UI item by ID
    pub fn delete_item(&self, item_id: &str) -> Result<()> {
        self.pool.with_conn(|conn| delete_item(conn, item_id))
    }

    /// Batch upsert multiple items (more efficient than individual calls)
    pub fn upsert_items(&self, items: &[UiItem]) -> Result<()> {
        self.pool.with_conn(|conn| {
            for item in items {
                upsert_item(conn, item)?;
            }
            Ok(())
        })
    }

    /// Batch host-refresh of items: creates missing rows whole, and on
    /// existing rows writes only the host-owned columns, never the
    /// user's arrangement (visibility, position, display mode). For a
    /// launch-time sync of plugin-declared items; a user-driven save
    /// wants [`Self::upsert_items`]. See `arclain_db::sync_host_item`
    /// for the exact column split.
    pub fn sync_host_items(&self, items: &[UiItem]) -> Result<()> {
        self.pool.with_conn(|conn| {
            for item in items {
                sync_host_item(conn, item)?;
            }
            Ok(())
        })
    }

    // =========================================================================
    // Display Options
    // =========================================================================

    /// Get a display option value by key
    pub fn get_display_option(&self, key: &str) -> Result<Option<String>> {
        self.pool.with_conn(|conn| get_display_option(conn, key))
    }

    /// Set a display option value
    pub fn set_display_option(&self, key: &str, value: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| set_display_option(conn, key, value))
    }

    /// Set several display options on one connection, in one
    /// transaction: every entry lands or none does. For entries that
    /// form one logical value -- a settings page saving all of its
    /// keys as one edit must not leave a mix of old and new behind a
    /// failure partway through.
    pub fn set_display_options(&self, entries: &[(&str, &str)]) -> Result<()> {
        self.pool
            .with_conn(|conn| set_display_options(conn, entries))
    }
}

impl std::fmt::Debug for UiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiService")
            .field("pool", &self.pool)
            .finish()
    }
}
