//! Per-tab browser-view state.
//!
//! `BrowserViewState` lives in `core/tabs/` because it's a property of
//! the **tab** — every tab in the multi-archive UI owns one, and
//! `TabState` is the source of truth. It was previously misfiled under
//! `features/archive_browser/domain/types.rs`, which forced
//! `core/tabs/tab_state.rs` to import from `features/` (violating the
//! `core/ ⊥ features/` invariant — see
//! docs/audits/2026-05-19-dependencies.md §2 + §5 medium #9).
//!
//! All fields are `shared/` types (file entries, sort state, toolbar
//! state, tree panel state) — none are archive_browser-specific — so
//! the relocation is a clean move with no field-level split.
//! `features/archive_browser` continues to consume the type, but now
//! through its core-owned home.

use crate::shared::components::toolbar::ToolbarState;
use crate::shared::components::tree_panel::TreePanelState;
use crate::shared::models::file_entry::{FileEntry, SortState};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserViewState {
    pub view_entries: Vec<FileEntry>,
    // NOTE: current_path moved to NavigationState signal pre-relocation
    //       for single source of truth; that history is preserved here.
    pub toolbar_state: ToolbarState,
    pub sort_state: SortState,
    pub tree_state: TreePanelState,
}
