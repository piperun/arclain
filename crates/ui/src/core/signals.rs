//! Application-level reactive signals
//!
//! This module provides reactive signals for async-updated state
//! that needs to trigger UI updates when changed from background threads.

use crate::core::operations::archive::ArchiveInfo;
use crate::core::state::UiPreferences;
use arclain_core::archive::NavigationState;
use arclain_core::features::organization::GameMetadata;
use arclain_core::utilities::PassRule;
use arclain_core::ArchiveEntry;
use arclain_core::UiItem;
use arclain_signals::{Signal, SignalContext};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// State for extraction progress dialog
#[derive(Clone, Debug, Default)]
pub struct ExtractionProgressState {
    /// Current file being extracted
    pub current_file: String,
    /// Progress percentage (0-100)
    pub percent: u8,
    /// Current file index
    pub current: usize,
    /// Total files to extract
    pub total: usize,
    /// Whether extraction is complete
    pub complete: bool,
    /// Error message if extraction failed
    pub error: Option<String>,
    /// Path to open after extraction completes (for open_file_from_archive)
    pub file_to_open: Option<PathBuf>,
    /// Whether extraction was cancelled
    #[allow(dead_code)]
    pub cancelled: bool,
}

/// Context for which toolbar should be displayed
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ToolbarContext {
    /// Show archive toolbar (Open, Extract, Delete, etc.)
    #[default]
    Archive,
    /// Show plugin-specific toolbar (plugin provides its own buttons)
    Plugin(String),
    /// No toolbar
    #[allow(dead_code)]
    None,
}

/// Application-wide reactive signals for async state.
///
/// These signals are used for state that updates asynchronously
/// (e.g., from plugin background threads) and needs to trigger
/// UI repaints when changed.
#[derive(Clone)]
pub struct AppSignals {
    /// Archive entries - set when archive is listed
    pub entries: Signal<Arc<Vec<ArchiveEntry>>>,

    /// Plugin metadata - set when plugin emits metadata
    pub metadata: Signal<Option<serde_json::Value>>,

    /// Whether archive is currently loading
    pub loading: Signal<bool>,

    /// Current archive path
    pub archive_path: Signal<Option<PathBuf>>,

    /// Whether the UI has rendered after an archive was opened.
    /// This is set to `false` when an archive opens and to `true` after
    /// the UI has rendered the file list. Used to defer plugin events
    /// until the UI is ready.
    pub ui_ready: Signal<bool>,

    /// Active toolbar context - determines which toolbar to show
    pub active_toolbar: Signal<ToolbarContext>,

    /// Status bar message from plugins (e.g., "Entry selected: RJ01234567")
    pub status_message: Signal<Option<String>>,

    /// Extraction progress for native backends
    pub extraction_progress: Signal<Option<ExtractionProgressState>>,

    /// Cancellation token for extraction - set to true to cancel
    pub extraction_cancel: Arc<AtomicBool>,

    /// Search text from header - used to filter entries in archive or settings
    pub search_text: Signal<String>,

    /// Toolbar items from DB - reactive for layout editor changes
    pub toolbar_items: Signal<Vec<UiItem>>,

    /// Info panel items from DB - reactive for layout editor changes
    pub info_panel_items: Signal<Vec<UiItem>>,

    /// Archive info (format, size, encryption status) - reactive
    pub archive_info: Signal<ArchiveInfo>,

    /// Game metadata from plugins (DLSite, etc.) - reactive
    pub game_metadata: Signal<Option<GameMetadata>>,

    /// UI display preferences - reactive for settings changes
    pub ui_preferences: Signal<UiPreferences>,

    /// User preferences from config DB - reactive
    pub user_config: Signal<arclain_core::UserConfig>,

    /// Navigation state - reactive
    pub navigation: Signal<NavigationState>,

    /// Current password for the open archive
    pub current_password: Signal<Option<String>>,

    /// Password rules for auto-unlock (from secrets DB)
    pub pass_rules: Signal<Vec<PassRule>>,

    /// Number of selected entries in file list (for toolbar button state)
    pub selection_count: Signal<usize>,

    /// Opened archive session - holds Archive handle with password for session lifetime
    pub opened_archive: Signal<Option<Arc<RwLock<arclain_core::Archive>>>>,

    /// [NEW] Status Bar State
    pub status_bar: Signal<crate::shared::components::status_bar::StatusBarInfo>,

    /// [NEW] Dialog States
    pub password_dialog: Signal<
        crate::features::password_management::views::dialogs::password_dialog::PasswordDialog,
    >,
    pub file_edit_dialog: Signal<crate::features::file_editing::file_edit_dialog::FileEditDialog>,

    /// [NEW] Archive Operations context
    pub pending_open_file: Signal<Option<String>>,

    /// [NEW] UI View State for Archive Browser
    pub browser_view_state:
        Signal<crate::features::archive_browser::domain::types::BrowserViewState>,

    /// [NEW] Operation Dialogs (Phase 2)
    pub extraction_dialog: Signal<crate::shared::dialogs::ExtractionProgressDialog>,
    pub conversion_dialog: Signal<crate::shared::dialogs::ExtractionProgressDialog>,
    pub drag_dialog: Signal<crate::shared::dialogs::ExtractionProgressDialog>,
}

impl AppSignals {
    /// Create new signals with default values.
    pub fn new() -> Self {
        Self {
            entries: Signal::new(Arc::new(Vec::new())),
            metadata: Signal::new(None),
            loading: Signal::new(false),
            archive_path: Signal::new(None),
            ui_ready: Signal::new(true), // Start as true (no archive to render)
            active_toolbar: Signal::new(ToolbarContext::Archive),
            status_message: Signal::new(None),
            extraction_progress: Signal::new(None),
            extraction_cancel: Arc::new(AtomicBool::new(false)),
            search_text: Signal::new(String::new()),
            toolbar_items: Signal::new(Vec::new()),
            info_panel_items: Signal::new(Vec::new()),
            archive_info: Signal::new(ArchiveInfo::default()),
            game_metadata: Signal::new(None),
            ui_preferences: Signal::new(UiPreferences::default()),
            user_config: Signal::new(arclain_core::UserConfig::default()),
            navigation: Signal::new(NavigationState::new()),
            current_password: Signal::new(None),
            pass_rules: Signal::new(Vec::new()),
            selection_count: Signal::new(0),
            opened_archive: Signal::new(None),
            status_bar: Signal::new(crate::shared::components::status_bar::StatusBarInfo::default()),
            password_dialog: Signal::new(crate::features::password_management::views::dialogs::password_dialog::PasswordDialog::default()),
            file_edit_dialog: Signal::new(crate::features::file_editing::file_edit_dialog::FileEditDialog::default()),
            pending_open_file: Signal::new(None),
            browser_view_state: Signal::new(crate::features::archive_browser::domain::types::BrowserViewState::default()),
            extraction_dialog: Signal::new(crate::shared::dialogs::ExtractionProgressDialog::default()),
            conversion_dialog: Signal::new(crate::shared::dialogs::ExtractionProgressDialog::default()),
            drag_dialog: Signal::new(crate::shared::dialogs::ExtractionProgressDialog::default()),
        }
    }

    /// Bind all signals to egui context for automatic repaint.
    pub fn bind_to_context(&self, ctx: &egui::Context) {
        let signal_ctx = SignalContext::new(ctx.clone());
        signal_ctx.bind(&self.entries);
        signal_ctx.bind(&self.metadata);
        signal_ctx.bind(&self.loading);
        signal_ctx.bind(&self.archive_path);
        signal_ctx.bind(&self.active_toolbar);
        signal_ctx.bind(&self.status_message);
        signal_ctx.bind(&self.extraction_progress);
        signal_ctx.bind(&self.search_text);
        signal_ctx.bind(&self.toolbar_items);
        signal_ctx.bind(&self.info_panel_items);
        signal_ctx.bind(&self.archive_info);
        signal_ctx.bind(&self.game_metadata);
        signal_ctx.bind(&self.ui_preferences);
        signal_ctx.bind(&self.navigation);
        signal_ctx.bind(&self.current_password);
        signal_ctx.bind(&self.pass_rules);
        signal_ctx.bind(&self.selection_count);
        signal_ctx.bind(&self.status_bar);
        signal_ctx.bind(&self.password_dialog);
        signal_ctx.bind(&self.file_edit_dialog);
        signal_ctx.bind(&self.pending_open_file);
        signal_ctx.bind(&self.browser_view_state);
        signal_ctx.bind(&self.extraction_dialog);
        signal_ctx.bind(&self.conversion_dialog);
        signal_ctx.bind(&self.drag_dialog);
        // Note: ui_ready is not bound to repaint - it's a control signal, not display
    }

    /// Reset all signals to default state.
    #[allow(dead_code)]
    pub fn reset(&self) {
        self.entries.set(Arc::new(Vec::new()));
        self.metadata.set(None);
        self.loading.set(false);
        self.archive_path.set(None);
        self.ui_ready.set(true);
        self.active_toolbar.set(ToolbarContext::Archive);
        self.status_message.set(None);
        self.extraction_progress.set(None);
        self.extraction_cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.search_text.set(String::new());
        self.toolbar_items.set(Vec::new());
        self.info_panel_items.set(Vec::new());
        self.archive_info.set(ArchiveInfo::default());
        self.game_metadata.set(None);
        self.ui_preferences.set(UiPreferences::default());
        self.navigation.set(NavigationState::new());
        self.current_password.set(None);
        self.pass_rules.set(Vec::new());
        self.selection_count.set(0);
        self.opened_archive.set(None);
        self.status_bar
            .set(crate::shared::components::status_bar::StatusBarInfo::default());
        self.password_dialog.set(crate::features::password_management::views::dialogs::password_dialog::PasswordDialog::default());
        self.file_edit_dialog
            .set(crate::features::file_editing::file_edit_dialog::FileEditDialog::default());
        self.pending_open_file.set(None);
        self.browser_view_state
            .set(crate::features::archive_browser::domain::types::BrowserViewState::default());
        self.extraction_dialog
            .set(crate::shared::dialogs::ExtractionProgressDialog::default());
        self.conversion_dialog
            .set(crate::shared::dialogs::ExtractionProgressDialog::default());
        self.drag_dialog
            .set(crate::shared::dialogs::ExtractionProgressDialog::default());
    }
}

impl Default for AppSignals {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signals_initialize_with_default_values() {
        let signals = AppSignals::new();

        // Verify default values
        assert!(signals.entries.get().is_empty());
        assert!(signals.metadata.get().is_none());
        assert!(!signals.loading.get());
        assert!(signals.archive_path.get().is_none());
        assert!(signals.ui_ready.get()); // Starts true
        assert_eq!(signals.active_toolbar.get(), ToolbarContext::Archive);
        assert!(signals.status_message.get().is_none());
        assert!(signals.extraction_progress.get().is_none());
        assert!(signals.search_text.get().is_empty());
        assert!(signals.toolbar_items.get().is_empty());
        assert!(signals.info_panel_items.get().is_empty());
        assert!(signals.game_metadata.get().is_none());
        assert!(signals.current_password.get().is_none());
    }

    #[test]
    fn test_signals_reset_clears_all_values() {
        let signals = AppSignals::new();

        // Set some values
        signals.loading.set(true);
        signals.search_text.set("test".to_string());
        signals.archive_path.set(Some(PathBuf::from("/test")));
        signals.status_message.set(Some("message".to_string()));
        signals.current_password.set(Some("secret".to_string()));

        // Verify values are set
        assert!(signals.loading.get());
        assert_eq!(signals.search_text.get(), "test");
        assert!(signals.archive_path.get().is_some());

        // Reset
        signals.reset();

        // Verify all values are back to defaults
        assert!(!signals.loading.get());
        assert!(signals.search_text.get().is_empty());
        assert!(signals.archive_path.get().is_none());
        assert!(signals.status_message.get().is_none());
        assert!(signals.current_password.get().is_none());
    }

    #[test]
    fn test_signal_set_get_roundtrip() {
        let signals = AppSignals::new();

        // Test set/get for various signal types
        signals.loading.set(true);
        assert!(signals.loading.get());

        signals.search_text.set("query".to_string());
        assert_eq!(signals.search_text.get(), "query");

        let path = PathBuf::from("/archive.zip");
        signals.archive_path.set(Some(path.clone()));
        assert_eq!(signals.archive_path.get(), Some(path));

        signals
            .current_password
            .set(Some("password123".to_string()));
        assert_eq!(
            signals.current_password.get(),
            Some("password123".to_string())
        );
    }

    #[test]
    fn test_signals_clone_shares_state() {
        let signals1 = AppSignals::new();
        let signals2 = signals1.clone();

        // Modify through clone
        signals2.loading.set(true);
        signals2.search_text.set("shared".to_string());

        // Original should see the changes (signals are Arc-wrapped)
        assert!(signals1.loading.get());
        assert_eq!(signals1.search_text.get(), "shared");
    }

    #[test]
    fn test_signals_entries_update() {
        use arclain_core::ArchiveEntry;

        let signals = AppSignals::new();

        // Create test entries
        let entries = vec![
            ArchiveEntry {
                path: "file1.txt".to_string(),
                size: 100,
                packed_size: 50,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            },
            ArchiveEntry {
                path: "file2.txt".to_string(),
                size: 200,
                packed_size: 100,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            },
        ];

        signals.entries.set(Arc::new(entries));

        let result = signals.entries.get();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "file1.txt");
        assert_eq!(result[1].path, "file2.txt");
    }

    #[test]
    fn test_extraction_cancel_atomic() {
        use std::sync::atomic::Ordering;

        let signals = AppSignals::new();

        // Default is false
        assert!(!signals.extraction_cancel.load(Ordering::SeqCst));

        // Set to true
        signals.extraction_cancel.store(true, Ordering::SeqCst);
        assert!(signals.extraction_cancel.load(Ordering::SeqCst));

        // Reset clears it
        signals.reset();
        assert!(!signals.extraction_cancel.load(Ordering::SeqCst));
    }
}
