use crate::shared::theme::AppTheme;
use arclain_app::layout::{UiItemDto, UiRegionDto};
use eframe::egui;

/// Callback that draws whatever a plugin toolbar item resolves to, and
/// applies whatever the user did to it.
///
/// This module deliberately knows nothing about plugins: no plugin type,
/// no document, no session. It only knows how to read its *own* stored
/// item — a `UiItemDto` whose `action_data` names either a plugin
/// (`"{plugin_id}"`) or one of that plugin's buttons
/// (`"{plugin_id}:{button_id}"`) — and hands the parsed pair to a host
/// that does. The closure is built in
/// `core/arclain_app/toolbar_handler.rs`, where reaching into
/// `features::plugins` is allowed.
///
/// Nothing comes back. The host owns the whole round trip (resolve the
/// plugin's UI session, draw, dispatch the interaction), so there is no
/// event vocabulary for this module to carry, and in particular no
/// reserved event-id string for it to intercept — the pre-cutover
/// version of this seam post-processed exactly two of those prefixes and
/// silently forwarded the rest to the plugin as literal event ids.
///
/// Inputs: ui, plugin_id, the specific button id the item names (`None`
/// when it names the plugin as a whole).
pub type PluginToolbarRenderer<'a> = &'a mut dyn FnMut(&mut egui::Ui, &str, Option<&str>);

/// Configuration for toolbar items as the application reports them
pub struct ToolbarConfig {
    items: Vec<UiItemDto>,
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl ToolbarConfig {
    pub fn new(items: Vec<UiItemDto>) -> Self {
        // Filter to only toolbar items and sort by sort_order
        let mut items: Vec<UiItemDto> = items
            .into_iter()
            .filter(|i| i.region == UiRegionDto::Toolbar)
            .collect();
        items.sort_by_key(|i| i.sort_order);
        Self { items }
    }

    /// Get visible items grouped by group_id, in sort order
    pub fn items_by_group(&self) -> Vec<(Option<String>, Vec<&UiItemDto>)> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<Option<String>, Vec<&UiItemDto>> = BTreeMap::new();

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
}

/// Context for button rendering
pub struct ButtonContext<'a> {
    pub theme: &'a AppTheme,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub can_go_up: bool,
    pub archive_loaded: bool,
    pub has_selection: bool,
    /// Show text labels next to icons
    pub show_labels: bool,
}
