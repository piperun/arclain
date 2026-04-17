use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_core::{DisplayMode, UiItem, UiRegion};
use arclain_plugins::types::PluginUiElement;
use std::collections::HashMap;

/// Configuration for toolbar items loaded from database
pub struct ToolbarConfig {
    items: Vec<UiItem>,
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl ToolbarConfig {
    pub fn new(items: Vec<UiItem>) -> Self {
        // Filter to only toolbar items and sort by sort_order
        let mut items: Vec<UiItem> = items
            .into_iter()
            .filter(|i| i.region == UiRegion::Toolbar)
            .collect();
        items.sort_by_key(|i| i.sort_order);
        Self { items }
    }

    /// Check if an item is visible by its id (e.g., "toolbar.back")
    #[allow(dead_code)]
    pub fn is_visible(&self, id: &str) -> bool {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.visible)
            .unwrap_or(true) // Default to visible if not configured
    }

    /// Get display mode for an item
    #[allow(dead_code)]
    pub fn display_mode(&self, id: &str) -> DisplayMode {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.display_mode)
            .unwrap_or(DisplayMode::IconAndText)
    }

    /// Get visible items grouped by group_id, in sort order
    pub fn items_by_group(&self) -> Vec<(Option<String>, Vec<&UiItem>)> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<Option<String>, Vec<&UiItem>> = BTreeMap::new();

        for item in self.items.iter().filter(|i| i.visible) {
            groups.entry(item.group_id.clone()).or_default().push(item);
        }

        // Convert to vec, maintaining group order by first item's sort_order
        let mut result: Vec<_> = groups.into_iter().collect();
        result.sort_by_key(|(_, items)| items.first().map(|i| i.sort_order).unwrap_or(0));
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolbarState {
    pub show_tree_panel: bool,
    pub show_properties_panel: bool,
    pub grid_view: bool,
    pub columns_locked: bool,
}

impl Default for ToolbarState {
    fn default() -> Self {
        Self {
            show_tree_panel: true,
            show_properties_panel: true,
            grid_view: false,
            columns_locked: true, // Start with columns locked to ensure proper default widths
        }
    }
}

#[derive(Default)]
pub struct ToolbarActions {
    pub go_back: bool,
    pub go_forward: bool,
    pub go_up: bool,
    pub extract: bool,
    pub extract_all: bool,
    pub add: bool,
    pub open: bool,
    pub delete_selected: bool,
    pub convert_to_7z: bool,
    pub batch_convert: bool,
    pub organize_archive: bool,
    /// Collected plugin events: (plugin_id, element_id, value)
    pub plugin_events: Vec<(String, String, Option<String>)>,
}

/// Context for button rendering
pub struct ButtonContext<'a> {
    pub theme: &'a AppTheme,
    pub shared: Option<&'a SharedState>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub can_go_up: bool,
    pub archive_loaded: bool,
    pub has_selection: bool,
    /// Cached plugin UI elements by plugin_id
    pub plugin_elements: HashMap<String, Vec<PluginUiElement>>,
    /// Show text labels next to icons
    pub show_labels: bool,
}
