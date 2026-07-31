//! Plugin dialog and page state management
//!
//! Tracks open dialogs and page stacks for plugins that use
//! ButtonAction::ShowDialog or ButtonAction::OpenPage.

use crate::core::tabs::TabId;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PageInitState {
    #[default]
    Idle,
    Pending {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
    },
    Initializing {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
    },
    Unavailable,
    Failed {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
        error: Arc<str>,
    },
}

/// State for managing plugin dialogs and pages.
///
/// Dialogs and pages both draw from facade sessions, but *where* one is
/// open stays renderer-owned navigation state rather than application
/// document state.
#[derive(Debug, Default, Clone)]
pub struct PluginDialogState {
    /// Currently open dialog: (plugin_id, dialog_id, origin tab). Read by
    /// the dialog renderer to key its facade session slot, so clearing it
    /// from any typed navigation or host-intent path is what closes that
    /// session, via the renderer's per-frame reconcile.
    pub open_dialog: Option<(String, String, TabId)>,
    /// Page stack: each entry is (plugin_id, page_id)
    pub page_stack: Vec<(String, String, TabId)>,
    /// Lifecycle state for the current page. Session identity and action
    /// routing reject stale completions, so this needs no request-id
    /// generation of its own.
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
    pub fn open_page(&mut self, plugin_id: &str, page_id: &str, origin_tab: TabId) {
        self.page_stack
            .push((plugin_id.to_string(), page_id.to_string(), origin_tab));
        self.page_init = PageInitState::Pending {
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
            origin_tab,
        };
    }

    /// Pop the current page from the stack
    pub fn close_page(&mut self) {
        self.page_stack.pop();
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

    pub fn page_init_pending(&self) -> bool {
        matches!(
            self.page_init,
            PageInitState::Pending { .. } | PageInitState::Initializing { .. }
        )
    }

    /// A page layout is only valid after the matching page-init
    /// generation has completed. Fetching it sooner can cache the
    /// plugin's pre-initialization layout indefinitely.
    pub fn page_layout_ready(&self) -> bool {
        matches!(self.page_init, PageInitState::Idle)
    }

    /// Claims the pending lifecycle generation for the facade session
    /// that has just become ready. Returns `false` for a stale render of
    /// any page other than the current pending one.
    pub fn begin_page_initialization(
        &mut self,
        plugin_id: &str,
        page_id: &str,
        origin_tab: TabId,
    ) -> bool {
        let PageInitState::Pending {
            plugin_id: pending_plugin,
            page_id: pending_page,
            origin_tab: pending_tab,
        } = &self.page_init
        else {
            return false;
        };
        if pending_plugin != plugin_id || pending_page != page_id || *pending_tab != origin_tab {
            return false;
        }
        self.page_init = PageInitState::Initializing {
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
            origin_tab,
        };
        true
    }

    pub fn complete_page_initialization(
        &mut self,
        plugin_id: &str,
        page_id: &str,
        origin_tab: TabId,
    ) -> bool {
        let PageInitState::Initializing {
            plugin_id: initializing_plugin,
            page_id: initializing_page,
            origin_tab: initializing_tab,
        } = &self.page_init
        else {
            return false;
        };
        if initializing_plugin != plugin_id
            || initializing_page != page_id
            || *initializing_tab != origin_tab
        {
            return false;
        }
        self.page_init = PageInitState::Idle;
        true
    }

    pub fn fail_page_initialization(
        &mut self,
        plugin_id: &str,
        page_id: &str,
        origin_tab: TabId,
        error: impl Into<Arc<str>>,
    ) -> bool {
        let PageInitState::Initializing {
            plugin_id: initializing_plugin,
            page_id: initializing_page,
            origin_tab: initializing_tab,
        } = &self.page_init
        else {
            return false;
        };
        if initializing_plugin != plugin_id
            || initializing_page != page_id
            || *initializing_tab != origin_tab
        {
            return false;
        }
        self.page_init = PageInitState::Failed {
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
            origin_tab,
            error: error.into(),
        };
        true
    }

    /// Settles lifecycle state when the facade session itself could not
    /// open (or failed before an init operation could be started). This
    /// is distinct from an init-action failure: the slot owns the visible
    /// error, while no pre-init document exists to retain.
    pub fn mark_page_unavailable(
        &mut self,
        plugin_id: &str,
        page_id: &str,
        origin_tab: TabId,
    ) -> bool {
        let matches_current = match &self.page_init {
            PageInitState::Pending {
                plugin_id: current_plugin,
                page_id: current_page,
                origin_tab: current_tab,
            }
            | PageInitState::Initializing {
                plugin_id: current_plugin,
                page_id: current_page,
                origin_tab: current_tab,
            } => {
                current_plugin == plugin_id && current_page == page_id && *current_tab == origin_tab
            }
            PageInitState::Idle | PageInitState::Unavailable | PageInitState::Failed { .. } => {
                false
            }
        };
        if matches_current {
            self.page_init = PageInitState::Unavailable;
        }
        matches_current
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
    fn page_layout_waits_for_matching_initialization() {
        let mut state = PluginDialogState::default();
        state.open_page("plugin", "page", TabId(1));

        assert!(!state.page_layout_ready());
        assert!(state.begin_page_initialization("plugin", "page", TabId(1)));
        assert!(state.complete_page_initialization("plugin", "page", TabId(1)));
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
