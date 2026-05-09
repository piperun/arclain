//! Plugin dialog and page state management
//!
//! Tracks open dialogs and page stacks for plugins that use
//! ButtonAction::ShowDialog or ButtonAction::OpenPage.

use arclain_plugins::types::PluginLayout;

/// State for managing plugin dialogs and pages
#[derive(Debug, Default, Clone)]
pub struct PluginDialogState {
    /// Currently open dialog: (plugin_id, dialog_id)
    pub open_dialog: Option<(String, String)>,
    /// Page stack: each entry is (plugin_id, page_id)
    pub page_stack: Vec<(String, String)>,
    /// Cached layout for the current dialog. Kept across UI events so
    /// the user sees the previous layout instead of a blank panel
    /// while a worker thread holds the plugin lock; the `*_stale`
    /// flag below tells the renderer to refetch when the lock is free.
    pub cached_dialog_layout: Option<PluginLayout>,
    /// Cached layout for the current page (same semantics as dialog).
    pub cached_page_layout: Option<PluginLayout>,
    /// Layout cache is stale and should be refetched on the next
    /// frame where the plugin instance lock is free.
    pub cached_dialog_layout_stale: bool,
    pub cached_page_layout_stale: bool,
    /// Flag to send __page_init event on next render (for SetPageDisplayName etc)
    pub page_needs_init: bool,
}

impl PluginDialogState {
    /// Create a new (empty) dialog state
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a dialog for a specific plugin
    pub fn open_dialog(&mut self, plugin_id: &str, dialog_id: &str) {
        self.open_dialog = Some((plugin_id.to_string(), dialog_id.to_string()));
        // Different dialog → previous cache is for a different layout.
        self.cached_dialog_layout = None;
        self.cached_dialog_layout_stale = false;
    }

    /// Close the current dialog
    pub fn close_dialog(&mut self) {
        self.open_dialog = None;
        self.cached_dialog_layout = None;
        self.cached_dialog_layout_stale = false;
    }

    /// Check if a dialog is currently open
    pub fn has_open_dialog(&self) -> bool {
        self.open_dialog.is_some()
    }

    /// Push a new page onto the stack
    pub fn open_page(&mut self, plugin_id: &str, page_id: &str) {
        self.page_stack
            .push((plugin_id.to_string(), page_id.to_string()));
        // Different page → previous cache is for a different layout.
        self.cached_page_layout = None;
        self.cached_page_layout_stale = false;
        self.page_needs_init = true; // Request init event on next render
    }

    /// Pop the current page from the stack
    pub fn close_page(&mut self) {
        self.page_stack.pop();
        self.cached_page_layout = None;
        self.cached_page_layout_stale = false;
    }

    /// Get the current page (if any)
    pub fn current_page(&self) -> Option<(&str, &str)> {
        self.page_stack
            .last()
            .map(|(p, d)| (p.as_str(), d.as_str()))
    }

    /// Check if any pages are open
    pub fn has_open_page(&self) -> bool {
        !self.page_stack.is_empty()
    }

    /// Mark the cached page layout stale. The renderer keeps showing
    /// the existing cache until it can refetch — this prevents the
    /// page from blanking while a worker thread holds the plugin lock
    /// during a long-running event.
    pub fn invalidate_page_layout(&mut self) {
        self.cached_page_layout_stale = true;
    }

    /// Mark the cached dialog layout stale (same semantics as
    /// `invalidate_page_layout`).
    pub fn invalidate_dialog_layout(&mut self) {
        self.cached_dialog_layout_stale = true;
    }
}
