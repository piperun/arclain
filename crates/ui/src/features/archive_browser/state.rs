use crate::shared::components::{file_list, toolbar, tree_panel};

#[derive(Default)]
pub struct ArchiveBrowserState {
    /// File list entries for current directory
    pub entries: Vec<file_list::FileEntry>,

    /// Current path within archive
    pub current_path: String,

    /// Toolbar state (grid/list view, panels visibility)
    pub toolbar_state: toolbar::ToolbarState,

    /// Sorting state for file list
    pub sort_state: file_list::SortState,

    /// Tree panel state (expanded folders, etc.)
    pub tree_state: tree_panel::TreePanelState,
}
