//! Core types for the layout editor abstraction.
//!
//! Defines the `Region` trait — implemented by `ToolbarRegion` and
//! `InfoPanelRegion` in `mod.rs` — plus the shared
//! `LayoutEditorState`, `LayoutEditorAction` enum, and the dispatcher
//! that runs the side effects.
//!
//! Region-specific behavior (which UI region the items belong to, how
//! to sync items from plugins, which axis to lay out previews on, how
//! to render the picker) lives behind the trait so the editor itself
//! is generic over both toolbar and info panel.

use crate::shared::SharedState;
use arclain_core::{UiItem, UiRegion, UiService};
use arclain_plugins::manager::PluginManager;
use arclain_signals::Signal;
use std::marker::PhantomData;

/// Orientation of the preview + selection-area arrow buttons.
#[derive(Copy, Clone, Debug)]
pub enum Axis {
    /// Toolbar: items flow left-to-right; selection arrows are ◀ ▶.
    Horizontal,
    /// Info panel: items stack top-to-bottom; selection arrows are ▲ ▼.
    Vertical,
}

/// Behavior contract for a layout-editable UI region.
///
/// Implementors describe one editable region of the app (`Toolbar`,
/// `InfoPanel`, etc.). The editor framework calls the trait methods
/// to: look up the persistent region tag, lay out the preview on the
/// right axis, sync plugin-contributed items, decide which items
/// belong in the user-facing picker, and resolve item icons.
pub trait Region: Sized + 'static {
    /// The `UiRegion` enum value used by `UiService` queries.
    const REGION: UiRegion;
    /// Whether the editor renders horizontally (toolbar) or vertically
    /// (info panel). Drives preview layout and selection-area arrows.
    const AXIS: Axis;

    /// Walk enabled plugins, inject any items they contribute for this
    /// region into `state.items`. Returns `true` if any items were
    /// added (so the editor can mark itself dirty).
    fn sync_plugin_items(
        state: &mut LayoutEditorState<Self>,
        manager: &PluginManager,
    ) -> bool;

    /// Filter rule for items shown in the user-facing picker. Used to
    /// hide internal items (e.g. `info.plugin_metadata` in the info
    /// panel). Default: every loaded item is user-visible.
    fn user_visible(_item: &UiItem) -> bool {
        true
    }

    /// Picker groups: `&[(group_id, display_name)]`. Empty slice means
    /// a flat picker (info panel); non-empty means the picker is
    /// segmented by group (toolbar).
    fn picker_groups() -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// Resolve a stored icon name to a phosphor character for preview
    /// rendering. `None` means the region doesn't render icons in its
    /// preview (info panel). Default: no icon.
    fn icon_for_name(_name: &str) -> Option<&'static str> {
        None
    }
}

/// Mutable state owned by a single layout editor instance.
///
/// Same shape for every region; the `PhantomData<R>` tag lets us keep
/// toolbar state and info-panel state in distinct types even though
/// the fields are identical. Stored on the app struct as
/// `ToolbarLayoutState` / `InfoPanelLayoutState` aliases.
pub struct LayoutEditorState<R: Region> {
    /// All items loaded for this region, including hidden ones (the
    /// picker filters with `R::user_visible`).
    pub items: Vec<UiItem>,
    /// First-load flag. `false` until the dispatcher loads from the
    /// `UiService` for the first time.
    pub loaded: bool,
    /// `true` when local edits diverge from the persisted state. The
    /// settings header's Save button reads this to decide whether the
    /// page is dirty.
    pub dirty: bool,
    /// Currently-selected item id, used for the selection area's
    /// move / remove buttons. `None` = nothing selected.
    pub selected_item_id: Option<String>,
    _region: PhantomData<R>,
}

impl<R: Region> Default for LayoutEditorState<R> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            loaded: false,
            dirty: false,
            selected_item_id: None,
            _region: PhantomData,
        }
    }
}

impl<R: Region> LayoutEditorState<R> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persist items to the `UiService`. Called from outside render
    /// (the settings header's Save button), so this stays a direct
    /// service call — it's already in a dispatcher context.
    pub fn save_to_service(&mut self, service: &UiService) {
        let _ = service.upsert_items(&self.items);
        self.dirty = false;
    }
}

/// Intents emitted by `render_layout_editor`. Currently one variant —
/// `SyncItems` covers both first-time load and per-frame plugin
/// reconciliation — but the enum shape leaves room for future
/// region-driven side effects without changing the public interface.
#[derive(Debug, Clone)]
pub enum LayoutEditorAction {
    /// Sync items from the canonical `AppSignals` (toolbar_items,
    /// info_panel_items, etc.) into `state.items`, then walk enabled
    /// plugins and merge in any new contributions.
    ///
    /// Signal-sync only runs when `state.dirty == false` so user edits
    /// in the editor aren't clobbered mid-session. If the user has
    /// pending unsaved changes, the signal-side updates queue until
    /// Save commits or Reset discards them.
    ///
    /// Auto-fired every frame from render; the load is cheap (signal
    /// clone) and the plugin sync is the same per-frame cost the
    /// pre-MVU code already paid.
    SyncItems,
}

/// Dispatch a `LayoutEditorAction` against the canonical signals +
/// plugin manager. Called by the parent view after
/// `render_layout_editor` returns an action.
///
/// All side effects (signal reads, plugin runtime calls) live here,
/// so the render path itself stays a pure intent-emitter.
pub fn handle_layout_editor_action<R: Region>(
    state: &mut LayoutEditorState<R>,
    action: LayoutEditorAction,
    shared: &SharedState,
    plugin_manager: Option<&PluginManager>,
) {
    match action {
        LayoutEditorAction::SyncItems => {
            // Refresh from signal when not dirty (or on first load).
            // Dirty-with-pending-edits paths skip this so the user's
            // local changes survive until Save / Reset.
            if !state.loaded || !state.dirty {
                if let Some(signal_items) = signal_for_region::<R>(shared) {
                    let mut filtered: Vec<UiItem> = signal_items
                        .into_iter()
                        .filter(|i| R::user_visible(i))
                        .collect();
                    filtered.sort_by_key(|i| i.sort_order);
                    state.items = filtered;
                    // Clear selection if the previously-selected item
                    // is no longer present (e.g. a sibling editor
                    // removed it via Save). Cheap O(n) scan; the item
                    // list is small.
                    if let Some(sel) = &state.selected_item_id {
                        if !state.items.iter().any(|i| &i.id == sel) {
                            state.selected_item_id = None;
                        }
                    }
                }
                state.loaded = true;
            }

            if let Some(manager) = plugin_manager {
                if R::sync_plugin_items(state, manager) {
                    state.dirty = true;
                }
            }
        }
    }
}

/// Read the canonical signal value for `R::REGION`. Returns `None` if
/// the region doesn't have a dedicated signal — caller falls back to
/// whatever items the state already holds.
fn signal_for_region<R: Region>(shared: &SharedState) -> Option<Vec<UiItem>> {
    let signal: &Signal<Vec<UiItem>> = match R::REGION {
        UiRegion::Toolbar => &shared.signals().toolbar_items,
        UiRegion::InfoPanel => &shared.signals().info_panel_items,
        UiRegion::ContextMenu => &shared.signals().context_menu_items,
        _ => return None,
    };
    Some(signal.get())
}
