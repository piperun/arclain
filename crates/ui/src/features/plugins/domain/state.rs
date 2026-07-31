//! Plugin dialog and page state management
//!
//! Tracks open dialogs and page stacks for plugins that use
//! ButtonAction::ShowDialog or ButtonAction::OpenPage.

use super::types::RequestId;
use crate::core::tabs::TabId;
use arclain_plugins::types::PluginLayout;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PageInitState {
    #[default]
    Idle,
    Pending {
        request_id: RequestId,
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
    },
    Failed {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
        error: Arc<str>,
    },
}

/// State for managing plugin dialogs and pages.
///
/// Deliberately shared across the two plugin render stacks while only one
/// of them has moved: the dialog half is drawn from a facade session and
/// the page half from the legacy `PluginUiJobs` cache, but a button in
/// either can navigate to the other, so *where* a dialog or page is open
/// stays one piece of renderer-owned state rather than two. See
/// `crate::features::plugins::application::facade_sessions`'s module doc
/// comment ("What is deliberately not modeled here").
#[derive(Debug, Default, Clone)]
pub struct PluginDialogState {
    /// Currently open dialog: (plugin_id, dialog_id, origin tab). Read by
    /// the dialog renderer to key its facade session slot, so clearing it
    /// from anywhere -- including the legacy queue's
    /// `PluginAction::CloseDialog` -- is what closes that session, via the
    /// renderer's per-frame reconcile.
    pub open_dialog: Option<(String, String, TabId)>,
    /// Page stack: each entry is (plugin_id, page_id)
    pub page_stack: Vec<(String, String, TabId)>,
    /// Cached layout for the current page. Kept across UI events so the
    /// user sees the previous layout instead of a blank panel while a
    /// worker thread holds the plugin lock; the `*_stale` flag below
    /// tells the renderer to refetch when the lock is free.
    pub cached_page_layout: Option<Arc<PluginLayout>>,
    /// Layout cache is stale and should be refetched on the next
    /// frame where the plugin instance lock is free.
    pub cached_page_layout_stale: bool,
    /// Generation-keyed page initialization request.
    pub page_init: PageInitState,
}

impl PluginDialogState {
    /// Create a new (empty) dialog state
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a dialog for a specific plugin.
    ///
    /// No layout bookkeeping to reset: a different dialog is a different
    /// facade session slot, so the renderer asks for a different document
    /// by construction.
    pub fn open_dialog(&mut self, plugin_id: &str, dialog_id: &str, origin_tab: TabId) {
        self.open_dialog = Some((plugin_id.to_string(), dialog_id.to_string(), origin_tab));
    }

    /// Close the current dialog
    pub fn close_dialog(&mut self) {
        self.open_dialog = None;
    }

    /// Check if a dialog is currently open
    pub fn has_open_dialog(&self) -> bool {
        self.open_dialog.is_some()
    }

    /// Push a new page onto the stack
    pub fn open_page(&mut self, plugin_id: &str, page_id: &str, origin_tab: TabId) -> RequestId {
        let request_id = RequestId::next();
        self.page_stack
            .push((plugin_id.to_string(), page_id.to_string(), origin_tab));
        // Different page → previous cache is for a different layout.
        self.cached_page_layout = None;
        self.cached_page_layout_stale = false;
        self.page_init = PageInitState::Pending {
            request_id,
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
            origin_tab,
        };
        request_id
    }

    /// Pop the current page from the stack
    pub fn close_page(&mut self) {
        self.page_stack.pop();
        self.cached_page_layout = None;
        self.cached_page_layout_stale = false;
        self.page_init = PageInitState::Idle;
    }

    /// Get the current page (if any)
    pub fn current_page(&self) -> Option<(&str, &str, TabId)> {
        self.page_stack
            .last()
            .map(|(plugin, page, origin_tab)| (plugin.as_str(), page.as_str(), *origin_tab))
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

    pub fn page_init_pending(&self) -> bool {
        matches!(self.page_init, PageInitState::Pending { .. })
    }

    /// A page layout is only valid after the matching page-init
    /// generation has completed. Fetching it sooner can cache the
    /// plugin's pre-initialization layout indefinitely.
    pub fn page_layout_ready(&self) -> bool {
        matches!(self.page_init, PageInitState::Idle)
    }

    pub fn pending_page_init(&self) -> Option<(RequestId, &str, &str, TabId)> {
        match &self.page_init {
            PageInitState::Idle | PageInitState::Failed { .. } => None,
            PageInitState::Pending {
                request_id,
                plugin_id,
                page_id,
                origin_tab,
            } => Some((*request_id, plugin_id, page_id, *origin_tab)),
        }
    }

    pub fn apply_page_initialized(&mut self, request_id: RequestId) -> bool {
        let PageInitState::Pending {
            request_id: pending,
            ..
        } = self.page_init
        else {
            return false;
        };
        if pending != request_id {
            return false;
        }
        self.page_init = PageInitState::Idle;
        true
    }

    pub fn apply_page_init_failure(&mut self, request_id: RequestId, error: String) -> bool {
        let PageInitState::Pending {
            request_id: pending,
            plugin_id,
            page_id,
            origin_tab,
        } = &self.page_init
        else {
            return false;
        };
        if *pending != request_id {
            return false;
        }
        self.page_init = PageInitState::Failed {
            plugin_id: plugin_id.clone(),
            page_id: page_id.clone(),
            origin_tab: *origin_tab,
            error: Arc::from(error),
        };
        true
    }

    pub fn page_init_error(&self) -> Option<Arc<str>> {
        match &self.page_init {
            PageInitState::Failed { error, .. } => Some(error.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_page_init_result_cannot_initialize_newer_page() {
        let mut state = PluginDialogState::default();
        let old = state.open_page("plugin", "old", TabId(1));
        let new = state.open_page("plugin", "new", TabId(2));

        assert!(!state.apply_page_initialized(old));
        assert!(state.page_init_pending());
        assert!(state.apply_page_initialized(new));
        assert!(!state.page_init_pending());
    }

    #[test]
    fn page_layout_waits_for_matching_initialization() {
        let mut state = PluginDialogState::default();
        let request = state.open_page("plugin", "page", TabId(1));

        assert!(!state.page_layout_ready());
        assert!(state.apply_page_initialized(request));
        assert!(state.page_layout_ready());
    }

    #[test]
    fn page_stack_preserves_each_pages_origin_tab() {
        let mut state = PluginDialogState::default();
        state.open_page("plugin", "parent", TabId(7));
        state.open_page("plugin", "child", TabId(9));

        assert_eq!(state.current_page(), Some(("plugin", "child", TabId(9))));
        state.close_page();
        assert_eq!(state.current_page(), Some(("plugin", "parent", TabId(7))));
    }
}
