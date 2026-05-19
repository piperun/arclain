//! Application-level reactive signals
//!
//! This module provides reactive signals for async-updated state
//! that needs to trigger UI updates when changed from background threads.

use crate::core::state::UiPreferences;
use crate::core::tabs::TabsCollection;
use crate::shared::dialogs::ask_each_time_drop::AskEachTimeDropState;
use crate::shared::dialogs::close_tab_confirm::CloseTabConfirmState;
use arclain_core::utilities::PassRule;
use arclain_core::UiItem;
use arclain_signals::{Signal, SignalContext};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Connection status for the gameta server.
///
/// Used by the global `server_status` signal to drive the header indicator.
/// Distinct from `ServerConnectionStatus` in settings types, which also tracks
/// transient states (`Idle`, `Testing`) used during interactive connection tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ServerConnectionStatus {
    /// Server integration is disabled or was never contacted.
    #[default]
    Offline,
    /// Successfully connected; holds the server version string.
    Connected(String),
    /// Server is configured but unreachable or returned an error.
    Error(String),
}

/// Live state of a Process page pipeline run. Updated by the background
/// runner via signal.
#[derive(Clone, Debug, Default)]
pub struct ProcessRunState {
    pub is_running: bool,
    pub current_file: String,
    pub current_step: String,
    pub files_done: usize,
    pub files_total: usize,
    pub files_failed: usize,
    pub files_skipped: usize,
    pub step_percent: u8,
    pub completed: bool,
    pub summary: Option<String>,
    /// Pre-formatted read-only warnings emitted by pipeline steps
    /// (currently only Flatten). Each entry is one warning ready for
    /// display — see `arclain_core::WarningKind::human()`.
    pub warnings: Vec<String>,
}

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
}

/// Context for which toolbar should be displayed
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ToolbarContext {
    /// Show archive toolbar (Open, Extract, Delete, etc.)
    #[default]
    Archive,
    /// Show plugin-specific toolbar (plugin provides its own buttons)
    Plugin(String),
}

/// Application-wide reactive signals for async state.
///
/// These signals are used for state that updates asynchronously
/// (e.g., from plugin background threads) and needs to trigger
/// UI repaints when changed.
///
/// Per-tab archive-context signals (archive_path, entries, navigation, etc.)
/// have been moved into `TabState` and are accessed via
/// `signals.tabs.get().active().<signal>.<op>()`.
#[derive(Clone)]
pub struct AppSignals {
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

    /// UI display preferences - reactive for settings changes
    pub ui_preferences: Signal<UiPreferences>,

    /// User preferences from config DB - reactive
    pub user_config: Signal<arclain_core::UserConfig>,

    /// Password rules for auto-unlock (from secrets DB)
    pub pass_rules: Signal<Vec<PassRule>>,

    /// [NEW] Status Bar State
    pub status_bar: Signal<crate::shared::components::status_bar::StatusBarInfo>,

    /// [NEW] Dialog States
    pub password_dialog: Signal<crate::features::password_management::dialogs::PasswordDialog>,
    pub file_edit_dialog: Signal<crate::features::file_editing::FileEditDialog>,

    /// [NEW] Archive Operations context
    pub pending_open_file: Signal<Option<String>>,

    /// [NEW] Operation Dialogs (Phase 2)
    pub extraction_dialog: Signal<crate::shared::dialogs::ExtractionProgressDialog>,
    pub conversion_dialog: Signal<crate::shared::dialogs::ExtractionProgressDialog>,
    /// Live state of a Process page pipeline run
    pub process_run: Signal<ProcessRunState>,
    pub drag_dialog: Signal<crate::shared::dialogs::ExtractionProgressDialog>,
    pub search_focus_requested: Signal<bool>,

    /// [NEW] Plugin Dialog State (Phase 3)
    pub plugin_dialog_state: Signal<crate::features::plugins::domain::state::PluginDialogState>,

    /// [NEW] Signal to reload hotkeys when settings change
    pub hotkeys_updated: Signal<bool>,

    /// [NEW] Merge dialog state for merging split archives
    pub merge_dialog: Signal<crate::shared::dialogs::MergeDialogState>,

    /// [NEW] Lightbox state for full-screen image viewing
    pub lightbox_state: Signal<crate::shared::dialogs::LightboxState>,

    /// Gameta server connection status — drives the header indicator.
    pub server_status: Signal<ServerConnectionStatus>,

    /// Multi-tab archive state. Per-tab signals (archive_path, entries,
    /// navigation, etc.) live inside each TabState — read via
    /// `signals.tabs.get().active().<signal>.get()`.
    pub tabs: Signal<TabsCollection>,

    /// Close-tab confirmation modal — shown when a tab with in-flight ops
    /// is closed. The user must confirm before the tab is force-closed.
    pub close_tab_confirm: Signal<CloseTabConfirmState>,

    /// "Ask each time" drop modal — shown when `drop_behavior` is
    /// `AskEachTime` and the user drops a file without aiming at an
    /// overlay zone. Holds the pending paths until the user picks
    /// New tab / Replace / Cancel.
    pub ask_each_time_drop: Signal<AskEachTimeDropState>,
}

impl AppSignals {
    /// Create new signals with default values.
    pub fn new() -> Self {
        Self {
            extraction_progress: Signal::new(None).with_name("extraction_progress"),
            extraction_cancel: Arc::new(AtomicBool::new(false)),
            search_text: Signal::new(String::new()).with_name("search_text"),
            toolbar_items: Signal::new(Vec::new()).with_name("toolbar_items"),
            info_panel_items: Signal::new(Vec::new()).with_name("info_panel_items"),
            ui_preferences: Signal::new(UiPreferences::default()).with_name("ui_preferences"),
            user_config: Signal::new(arclain_core::UserConfig::default()).with_name("user_config"),
            pass_rules: Signal::new(Vec::new()).with_name("pass_rules"),
            status_bar: Signal::new(
                crate::shared::components::status_bar::StatusBarInfo::default(),
            )
            .with_name("status_bar"),
            password_dialog: Signal::new(
                crate::features::password_management::dialogs::PasswordDialog::default(),
            )
            .with_name("password_dialog"),
            file_edit_dialog: Signal::new(crate::features::file_editing::FileEditDialog::default())
                .with_name("file_edit_dialog"),

            pending_open_file: Signal::new(None).with_name("pending_open_file"),
            extraction_dialog: Signal::new(
                crate::shared::dialogs::ExtractionProgressDialog::default(),
            )
            .with_name("extraction_dialog"),
            conversion_dialog: Signal::new(
                crate::shared::dialogs::ExtractionProgressDialog::default(),
            )
            .with_name("conversion_dialog"),
            process_run: Signal::new(ProcessRunState::default()).with_name("process_run"),
            drag_dialog: Signal::new(crate::shared::dialogs::ExtractionProgressDialog::default())
                .with_name("drag_dialog"),
            search_focus_requested: Signal::new(false).with_name("search_focus_requested"),
            plugin_dialog_state: Signal::new(
                crate::features::plugins::domain::state::PluginDialogState::default(),
            )
            .with_name("plugin_dialog_state"),
            hotkeys_updated: Signal::new(false).with_name("hotkeys_updated"),
            merge_dialog: Signal::new(crate::shared::dialogs::MergeDialogState::default())
                .with_name("merge_dialog"),
            lightbox_state: Signal::new(crate::shared::dialogs::LightboxState::default())
                .with_name("lightbox_state"),
            server_status: Signal::new(ServerConnectionStatus::default())
                .with_name("server_status"),
            tabs: Signal::new(TabsCollection::new()).with_name("tabs"),
            close_tab_confirm: Signal::new(CloseTabConfirmState::default())
                .with_name("close_tab_confirm"),
            ask_each_time_drop: Signal::new(AskEachTimeDropState::default())
                .with_name("ask_each_time_drop"),
        }
    }

    /// Bind all signals to egui context for automatic repaint.
    pub fn bind_to_context(&self, ctx: &egui::Context) {
        let signal_ctx = SignalContext::new(ctx.clone());
        signal_ctx.bind_named(&self.extraction_progress, "extraction_progress");
        signal_ctx.bind_named(&self.search_text, "search_text");
        signal_ctx.bind_named(&self.toolbar_items, "toolbar_items");
        signal_ctx.bind_named(&self.info_panel_items, "info_panel_items");
        signal_ctx.bind_named(&self.ui_preferences, "ui_preferences");
        signal_ctx.bind_named(&self.pass_rules, "pass_rules");
        signal_ctx.bind_named(&self.status_bar, "status_bar");
        signal_ctx.bind_named(&self.password_dialog, "password_dialog");
        signal_ctx.bind_named(&self.file_edit_dialog, "file_edit_dialog");
        signal_ctx.bind_named(&self.pending_open_file, "pending_open_file");
        // Note: per-tab browser_view_state is not bound here — it lives in TabState and
        // is mutated during render, so binding it would cause repaint loops
        signal_ctx.bind_named(&self.extraction_dialog, "extraction_dialog");
        signal_ctx.bind_named(&self.conversion_dialog, "conversion_dialog");
        signal_ctx.bind_named(&self.process_run, "process_run");
        signal_ctx.bind_named(&self.drag_dialog, "drag_dialog");
        // Note: plugin_dialog_state is not bound - it's mutated during render (cache) so would cause repaint loops
        // Plugin dialogs/pages are rendered in render_overlays after the signal is updated anyway
        // Note: per-tab ui_ready is not bound to repaint — it's a control signal, not display
        signal_ctx.bind_named(&self.merge_dialog, "merge_dialog");
        signal_ctx.bind_named(&self.lightbox_state, "lightbox_state");
        signal_ctx.bind_named(&self.server_status, "server_status");
        signal_ctx.bind_named(&self.tabs, "tabs");
        signal_ctx.bind_named(&self.close_tab_confirm, "close_tab_confirm");
        signal_ctx.bind_named(&self.ask_each_time_drop, "ask_each_time_drop");
    }

    /// Reset all signals to default state.
    #[allow(dead_code)]
    pub fn reset(&self) {
        self.extraction_progress.set(None);
        self.extraction_cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.search_text.set(String::new());
        self.toolbar_items.set(Vec::new());
        self.info_panel_items.set(Vec::new());
        self.ui_preferences.set(UiPreferences::default());
        self.pass_rules.set(Vec::new());
        self.status_bar
            .set(crate::shared::components::status_bar::StatusBarInfo::default());
        self.password_dialog
            .set(crate::features::password_management::dialogs::PasswordDialog::default());
        self.file_edit_dialog.set(crate::features::file_editing::FileEditDialog::default());
        self.pending_open_file.set(None);
        self.extraction_dialog
            .set(crate::shared::dialogs::ExtractionProgressDialog::default());
        self.conversion_dialog
            .set(crate::shared::dialogs::ExtractionProgressDialog::default());
        self.process_run.set(ProcessRunState::default());
        self.drag_dialog
            .set(crate::shared::dialogs::ExtractionProgressDialog::default());
        self.plugin_dialog_state
            .set(crate::features::plugins::domain::state::PluginDialogState::default());
        self.merge_dialog
            .set(crate::shared::dialogs::MergeDialogState::default());
        self.lightbox_state
            .set(crate::shared::dialogs::LightboxState::default());
        self.server_status.set(ServerConnectionStatus::default());
        self.tabs.set(TabsCollection::new());
        self.close_tab_confirm.set(CloseTabConfirmState::default());
        self.ask_each_time_drop.set(AskEachTimeDropState::default());
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
    use std::path::PathBuf;

    #[test]
    fn test_signals_initialize_with_default_values() {
        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();

        // Per-tab signals start at defaults
        assert!(tab.entries.get().is_empty());
        assert!(tab.metadata.get().is_none());
        assert!(!tab.loading.get());
        assert!(tab.archive_path.get().is_none());
        assert!(tab.ui_ready.get()); // Starts true
        assert_eq!(tab.active_toolbar.get(), ToolbarContext::Archive);
        assert!(tab.status_message.get().is_none());
        assert!(tab.game_metadata.get().is_none());
        assert!(tab.current_password.get().is_none());

        // Global signals
        assert!(signals.extraction_progress.get().is_none());
        assert!(signals.search_text.get().is_empty());
        assert!(signals.toolbar_items.get().is_empty());
        assert!(signals.info_panel_items.get().is_empty());
    }

    #[test]
    fn test_signals_reset_clears_all_values() {
        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();

        // Set some per-tab values
        tab.loading.set(true);
        tab.archive_path.set(Some(PathBuf::from("/test")));
        tab.status_message.set(Some("message".to_string()));
        tab.current_password.set(Some("secret".to_string()));
        // Set a global value
        signals.search_text.set("test".to_string());

        // Verify values are set
        assert!(tab.loading.get());
        assert!(tab.archive_path.get().is_some());
        assert_eq!(signals.search_text.get(), "test");

        // Reset replaces tabs collection (new TabState = defaults)
        signals.reset();
        let tab2 = signals.tabs.get().active().clone();

        // Per-tab signals reset via new TabState
        assert!(!tab2.loading.get());
        assert!(tab2.archive_path.get().is_none());
        assert!(tab2.status_message.get().is_none());
        assert!(tab2.current_password.get().is_none());
        // Global signal reset
        assert!(signals.search_text.get().is_empty());
    }

    #[test]
    fn test_signal_set_get_roundtrip() {
        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();

        // Per-tab signals
        tab.loading.set(true);
        assert!(tab.loading.get());

        signals.search_text.set("query".to_string());
        assert_eq!(signals.search_text.get(), "query");

        let path = PathBuf::from("/archive.zip");
        tab.archive_path.set(Some(path.clone()));
        assert_eq!(tab.archive_path.get(), Some(path));

        tab.current_password.set(Some("password123".to_string()));
        assert_eq!(
            tab.current_password.get(),
            Some("password123".to_string())
        );
    }

    #[test]
    fn test_signals_clone_shares_state() {
        let signals1 = AppSignals::new();
        let signals2 = signals1.clone();

        // Global signals are Arc-shared
        signals2.search_text.set("shared".to_string());
        assert_eq!(signals1.search_text.get(), "shared");

        // Per-tab signals: both signals views share the same TabsCollection signal,
        // so active() returns the same Arc<TabState>
        let tab1 = signals1.tabs.get().active().clone();
        let tab2 = signals2.tabs.get().active().clone();
        tab2.loading.set(true);
        assert!(tab1.loading.get());
    }

    #[test]
    fn test_signals_entries_update() {
        use arclain_core::ArchiveEntry;

        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();

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

        tab.entries.set(Arc::new(entries));

        let result = tab.entries.get();
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
