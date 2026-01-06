//! UI Service for managing UI configuration items
//!
//! Wraps arclain_db UI item operations with connection pool management.
//! Centralizes access to toolbar items, info panel items, and display options.

use anyhow::Result;
use arclain_db::{
    delete_item_diesel, get_display_option_diesel, list_items_by_region_diesel,
    set_display_option_diesel, upsert_item_diesel, DieselPool, UiItem, UiRegion,
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
            .with_conn(|conn| list_items_by_region_diesel(conn, region))
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
        self.pool.with_conn(|conn| upsert_item_diesel(conn, item))
    }

    /// Delete a UI item by ID
    pub fn delete_item(&self, item_id: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| delete_item_diesel(conn, item_id))
    }

    /// Batch upsert multiple items (more efficient than individual calls)
    pub fn upsert_items(&self, items: &[UiItem]) -> Result<()> {
        self.pool.with_conn(|conn| {
            for item in items {
                upsert_item_diesel(conn, item)?;
            }
            Ok(())
        })
    }

    // =========================================================================
    // Display Options
    // =========================================================================

    /// Get a display option value by key
    pub fn get_display_option(&self, key: &str) -> Result<Option<String>> {
        self.pool
            .with_conn(|conn| get_display_option_diesel(conn, key))
    }

    /// Set a display option value
    pub fn set_display_option(&self, key: &str, value: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| set_display_option_diesel(conn, key, value))
    }
}

impl std::fmt::Debug for UiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiService")
            .field("pool", &self.pool)
            .finish()
    }
}
