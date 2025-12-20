//! Plugin dialog and page state management
//!
//! Tracks open dialogs and page stacks for plugins that use
//! ButtonAction::ShowDialog or ButtonAction::OpenPage.

/// State for managing plugin dialogs and pages
#[allow(dead_code)] // Future use for dialog/page navigation
#[derive(Debug, Default)]
pub struct PluginDialogState {
    /// Currently open dialog: (plugin_id, dialog_id)
    pub open_dialog: Option<(String, String)>,
    /// Page stack: each entry is (plugin_id, page_id)
    pub page_stack: Vec<(String, String)>,
}

impl PluginDialogState {
    /// Create a new (empty) dialog state
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a dialog for a specific plugin
    #[allow(dead_code)]
    pub fn open_dialog(&mut self, plugin_id: &str, dialog_id: &str) {
        self.open_dialog = Some((plugin_id.to_string(), dialog_id.to_string()));
    }

    /// Close the current dialog
    #[allow(dead_code)]
    pub fn close_dialog(&mut self) {
        self.open_dialog = None;
    }

    /// Check if a dialog is currently open
    #[allow(dead_code)]
    pub fn has_open_dialog(&self) -> bool {
        self.open_dialog.is_some()
    }

    /// Push a new page onto the stack
    #[allow(dead_code)]
    pub fn open_page(&mut self, plugin_id: &str, page_id: &str) {
        self.page_stack
            .push((plugin_id.to_string(), page_id.to_string()));
    }

    /// Pop the current page from the stack
    #[allow(dead_code)]
    pub fn close_page(&mut self) {
        self.page_stack.pop();
    }

    /// Get the current page (if any)
    #[allow(dead_code)]
    pub fn current_page(&self) -> Option<(&str, &str)> {
        self.page_stack
            .last()
            .map(|(p, d)| (p.as_str(), d.as_str()))
    }

    /// Check if any pages are open
    #[allow(dead_code)]
    pub fn has_open_page(&self) -> bool {
        !self.page_stack.is_empty()
    }
}
