//! Application-level reactive signals
//!
//! This module provides reactive signals for async-updated state
//! that needs to trigger UI updates when changed from background threads.

use crate::core::state::UiPreferences;
use crate::core::tabs::TabsCollection;
use crate::shared::dialogs::archive_error_dialog::ArchiveErrorDialogState;
use crate::shared::dialogs::ask_each_time_drop::AskEachTimeDropState;
use crate::shared::dialogs::close_tab_confirm::CloseTabConfirmState;
use arclain_core::utilities::PassRule;
use arclain_core::UiItem;
use arclain_signals::{Signal, SignalContext};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

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
    /// The in-archive file path the user originally clicked to trigger
    /// this extraction (passed by `open_file_from_archive`). Set only
    /// on the file-opener path — `None` for batch organization
    /// extractions. `process_extraction_progress` reads this on
    /// password-error completion so the unlock handler can auto-retry
    /// the same file after the user enters a password.
    pub requested_file_path: Option<String>,
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

    /// Context menu items from DB - reactive for interface visibility
    /// toggles (the Interface settings page mutates these and they
    /// drive what appears in the right-click menus around the app).
    pub context_menu_items: Signal<Vec<UiItem>>,

    /// UI display preferences - reactive for settings changes
    pub ui_preferences: Signal<UiPreferences>,

    /// User preferences from config DB - reactive
    pub user_config: Signal<arclain_core::UserConfig>,

    /// Password rules for auto-unlock (from secrets DB)
    pub pass_rules: Signal<Vec<PassRule>>,

    /// [NEW] Status Bar State
    pub status_bar: Signal<crate::shared::components::status_bar::StatusBarInfo>,

    // `password_dialog` migrated to `TabState` in the 2026-05-20 B3
    // reframed slice. Read via `signals.tabs.get().active().password_dialog`.
    // Two encrypted archives in two tabs no longer overwrite each
    // other's prompt — the dialog lives on the tab that triggered it.
    // `file_edit_dialog` migrated to `TabState` in the 2026-05-19 audit.
    // Read via `signals.tabs.get().active().file_edit_dialog`.

    /// [NEW] Archive Operations context
    // `pending_open_file` migrated to `TabState` in the 2026-05-19 audit.
    // Read via `signals.tabs.get().active().pending_open_file`. The "file
    // queued for opening" is inherently per-tab — closing the tab cleanly
    // drops the request.

    // `progress_dialogs` migrated to `TabState` in the 2026-05-20 B3
    // reframed slice 2. Read via `signals.tabs.get().active().extraction_dialog()`
    // (or `.conversion_dialog()` / `.drag_dialog()`). The A3 slot-struct
    // + proxy pattern (commit 9975481) is preserved — only the
    // location changes. Background-thread workers reach the right
    // dialog through the `extraction_origin_tab` / `conversion_origin_tab`
    // handles captured at spawn time in ArchiveOperationsState.

    /// Live state of a Process page pipeline run
    pub process_run: Signal<ProcessRunState>,
    pub search_focus_requested: Signal<bool>,

    /// [NEW] Plugin Dialog State (Phase 3)
    pub plugin_dialog_state: Signal<crate::features::plugins::domain::state::PluginDialogState>,

    /// [NEW] Signal to reload hotkeys when settings change
    pub hotkeys_updated: Signal<bool>,

    // `merge_dialog` migrated to `TabState` in the 2026-05-20 audit B2 follow-up.
    // Read via `signals.tabs.get().active().merge_dialog`. The merge operation
    // always targets the active tab's archive — closing the tab cleanly drops
    // the in-progress dialog state.
    // `lightbox_state` migrated to `TabState` in the 2026-05-20 audit B2
    // follow-up. Read via `signals.tabs.get().active().lightbox_state`. The
    // lightbox is plugin-driven, and the plugin is tied to a tab.

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

    /// Archive-load error modal — populated by `load_archive_into_tab`
    /// when a backend fails to open the file for a non-password reason
    /// (EACCES is the headline case; the dialog also surfaces raw
    /// errors for everything else so users aren't left staring at an
    /// empty entry list).
    pub archive_error_dialog: Signal<ArchiveErrorDialogState>,

    /// Egui context, stashed at `bind_to_context` time so background
    /// workers can call `kick_repaint()` to force a frame even when
    /// the signal-subscriber repaint path misses a wake (XWayland-side
    /// flake). `OnceLock` so it's set exactly once and concurrent
    /// reads are lock-free. `Arc` so clones of `AppSignals` share the
    /// same slot.
    pub egui_ctx: Arc<OnceLock<egui::Context>>,
}

/// Proxy that lets callers keep the old `signals.extraction_dialog
/// .get()/.set()/.set_if_changed()` shape after the 3 progress
/// dialog signals were merged into one `progress_dialogs` slot
/// signal in audit A3 (commit 9975481). The proxy reads/writes the
/// slot inside `ProgressDialogs` without exposing the container
/// struct to every callsite. Call the accessor on a `TabState`
/// (`tab.extraction_dialog()`) to get a proxy; chain `.get()` /
/// `.set(d)` / `.set_if_changed(d)` as before.
///
/// Post 2026-05-20 B3 reframed slice 2: the proxy now points at a
/// `TabState`-owned signal rather than `AppSignals`. Callers
/// determine which tab to address (active for UI-thread paths,
/// `extraction_origin_tab` / `conversion_origin_tab` for background
/// workers). The proxy struct itself is unchanged.
pub struct ProgressDialogProxy<'a> {
    parent: &'a Signal<crate::shared::dialogs::ProgressDialogs>,
    kind: ProgressKind,
}

#[derive(Copy, Clone)]
enum ProgressKind {
    Extraction,
    Conversion,
    Drag,
}

impl<'a> ProgressDialogProxy<'a> {
    /// Build a proxy targeting the `extraction` slot of the given
    /// progress-dialog signal. Constructor lives here (rather than on
    /// `TabState`) so the kind-tag enum can stay private.
    pub fn extraction(parent: &'a Signal<crate::shared::dialogs::ProgressDialogs>) -> Self {
        Self {
            parent,
            kind: ProgressKind::Extraction,
        }
    }

    /// Build a proxy targeting the `conversion` slot. See
    /// [`Self::extraction`].
    pub fn conversion(parent: &'a Signal<crate::shared::dialogs::ProgressDialogs>) -> Self {
        Self {
            parent,
            kind: ProgressKind::Conversion,
        }
    }

    /// Build a proxy targeting the `drag` slot. See
    /// [`Self::extraction`].
    pub fn drag(parent: &'a Signal<crate::shared::dialogs::ProgressDialogs>) -> Self {
        Self {
            parent,
            kind: ProgressKind::Drag,
        }
    }

    fn read(&self, dlgs: &crate::shared::dialogs::ProgressDialogs)
        -> crate::shared::dialogs::ExtractionProgressDialog
    {
        match self.kind {
            ProgressKind::Extraction => dlgs.extraction.clone(),
            ProgressKind::Conversion => dlgs.conversion.clone(),
            ProgressKind::Drag => dlgs.drag.clone(),
        }
    }

    fn write(&self, dlgs: &mut crate::shared::dialogs::ProgressDialogs,
             new: crate::shared::dialogs::ExtractionProgressDialog) {
        match self.kind {
            ProgressKind::Extraction => dlgs.extraction = new,
            ProgressKind::Conversion => dlgs.conversion = new,
            ProgressKind::Drag => dlgs.drag = new,
        }
    }

    pub fn get(&self) -> crate::shared::dialogs::ExtractionProgressDialog {
        self.read(&self.parent.get())
    }

    pub fn set(&self, new: crate::shared::dialogs::ExtractionProgressDialog) {
        let mut dlgs = self.parent.get();
        self.write(&mut dlgs, new);
        self.parent.set(dlgs);
    }

    pub fn set_if_changed(&self, new: crate::shared::dialogs::ExtractionProgressDialog) {
        let mut dlgs = self.parent.get();
        // Only propagate the change if THIS slot actually differs —
        // mirrors the per-signal `set_if_changed` semantics so we
        // don't trigger a full repaint for a no-op assignment on
        // one slot just because another slot's state happens to
        // also live in the same struct.
        let current_slot = self.read(&dlgs);
        if current_slot != new {
            self.write(&mut dlgs, new);
            self.parent.set(dlgs);
        }
    }
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
            context_menu_items: Signal::new(Vec::new()).with_name("context_menu_items"),
            ui_preferences: Signal::new(UiPreferences::default()).with_name("ui_preferences"),
            user_config: Signal::new(arclain_core::UserConfig::default()).with_name("user_config"),
            pass_rules: Signal::new(Vec::new()).with_name("pass_rules"),
            status_bar: Signal::new(
                crate::shared::components::status_bar::StatusBarInfo::default(),
            )
            .with_name("status_bar"),

            process_run: Signal::new(ProcessRunState::default()).with_name("process_run"),
            search_focus_requested: Signal::new(false).with_name("search_focus_requested"),
            plugin_dialog_state: Signal::new(
                crate::features::plugins::domain::state::PluginDialogState::default(),
            )
            .with_name("plugin_dialog_state"),
            hotkeys_updated: Signal::new(false).with_name("hotkeys_updated"),
            server_status: Signal::new(ServerConnectionStatus::default())
                .with_name("server_status"),
            tabs: Signal::new(TabsCollection::new()).with_name("tabs"),
            close_tab_confirm: Signal::new(CloseTabConfirmState::default())
                .with_name("close_tab_confirm"),
            ask_each_time_drop: Signal::new(AskEachTimeDropState::default())
                .with_name("ask_each_time_drop"),
            archive_error_dialog: Signal::new(ArchiveErrorDialogState::default())
                .with_name("archive_error_dialog"),
            egui_ctx: Arc::new(OnceLock::new()),
        }
    }

    /// Wake the event loop from a worker thread. No-op if the egui
    /// context hasn't been bound yet (e.g. called from a tab-restore
    /// thread that fires before `bind_to_context` lands on the first
    /// frame).
    ///
    /// Backstop for the signal-subscriber repaint mechanism. The bound
    /// subscribers each call `ctx.request_repaint()` on `.set()` already,
    /// but on XWayland those wakes sometimes get dropped and the UI
    /// renders the pre-write state until the next mouse event. Calling
    /// `kick_repaint()` explicitly closes that race.
    pub fn kick_repaint(&self) {
        if let Some(ctx) = self.egui_ctx.get() {
            ctx.request_repaint();
        }
    }

    /// Bind all signals to egui context for automatic repaint.
    pub fn bind_to_context(&self, ctx: &egui::Context) {
        // Stash the ctx for `kick_repaint()` (the worker-thread
        // backstop). OnceLock returns Err if it's already populated —
        // that only happens if bind is called twice, which the
        // `bind_signals_once` guard upstream prevents.
        let _ = self.egui_ctx.set(ctx.clone());
        let signal_ctx = SignalContext::new(ctx.clone());
        signal_ctx.bind_named(&self.extraction_progress, "extraction_progress");
        signal_ctx.bind_named(&self.search_text, "search_text");
        signal_ctx.bind_named(&self.toolbar_items, "toolbar_items");
        signal_ctx.bind_named(&self.info_panel_items, "info_panel_items");
        signal_ctx.bind_named(&self.context_menu_items, "context_menu_items");
        signal_ctx.bind_named(&self.ui_preferences, "ui_preferences");
        signal_ctx.bind_named(&self.pass_rules, "pass_rules");
        signal_ctx.bind_named(&self.status_bar, "status_bar");
        // Note: per-tab password_dialog is not bound here — it lives in TabState
        // (post 2026-05-20 B3 reframed slice)
        // Note: per-tab browser_view_state is not bound here — it lives in TabState and
        // is mutated during render, so binding it would cause repaint loops
        // Note: per-tab progress_dialogs is not bound here — it lives in TabState
        // (post 2026-05-20 B3 reframed slice 2)
        signal_ctx.bind_named(&self.process_run, "process_run");
        // Note: plugin_dialog_state is not bound - it's mutated during render (cache) so would cause repaint loops
        // Plugin dialogs/pages are rendered in render_overlays after the signal is updated anyway
        // Note: per-tab ui_ready is not bound to repaint — it's a control signal, not display
        // Note: per-tab merge_dialog and lightbox_state are not bound here — they
        // live in TabState (post 2026-05-20 audit B2 follow-up)
        signal_ctx.bind_named(&self.server_status, "server_status");
        signal_ctx.bind_named(&self.tabs, "tabs");
        signal_ctx.bind_named(&self.close_tab_confirm, "close_tab_confirm");
        signal_ctx.bind_named(&self.ask_each_time_drop, "ask_each_time_drop");
        signal_ctx.bind_named(&self.archive_error_dialog, "archive_error_dialog");

        // Per-tab signal auto-binding.
        //
        // Per-tab signals (entries, archive_path, browser_view_state,
        // password_dialog, progress_dialogs, …) live on each TabState
        // and are NOT bound by the explicit bind_named calls above.
        // Without this hook, background-thread writes to those signals
        // (drop-zip archive list, password-error dialog show,
        // extraction progress writes, …) would land silently and the
        // UI would render the pre-write state until the next input
        // event woke it up. Symptom: "drop a zip and nothing shows
        // until I click somewhere."
        //
        // Bind any tabs that already exist (the default initial tab
        // from TabsCollection::new and any tabs restored by
        // app_lifecycle::restore_tabs_on_launch before bind_to_context
        // fires on the first frame).
        for tab in self.tabs.get().tabs() {
            tab.bind_to_context_once(ctx);
        }

        // Subscribe to future tabs.set() so every newly-added tab
        // (drop overlay, Ctrl+T, reopen-closed) gets bound the moment
        // it joins the collection. Idempotent via the per-tab
        // AtomicBool flag — re-firing on every mutation is cheap.
        let ctx_for_future = ctx.clone();
        let tabs_signal = self.tabs.clone();
        self.tabs.subscribe(move || {
            for tab in tabs_signal.get().tabs() {
                tab.bind_to_context_once(&ctx_for_future);
            }
        });
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
        self.context_menu_items.set(Vec::new());
        self.ui_preferences.set(UiPreferences::default());
        self.pass_rules.set(Vec::new());
        self.status_bar
            .set(crate::shared::components::status_bar::StatusBarInfo::default());
        self.process_run.set(ProcessRunState::default());
        self.plugin_dialog_state
            .set(crate::features::plugins::domain::state::PluginDialogState::default());
        self.server_status.set(ServerConnectionStatus::default());
        self.tabs.set(TabsCollection::new());
        self.close_tab_confirm.set(CloseTabConfirmState::default());
        self.ask_each_time_drop.set(AskEachTimeDropState::default());
        self.archive_error_dialog
            .set(ArchiveErrorDialogState::default());
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
        assert!(tab.archive_path.get().is_none());
        assert!(tab.ui_ready.get()); // Starts true
        assert_eq!(tab.active_toolbar.get(), ToolbarContext::Archive);
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
        tab.archive_path.set(Some(PathBuf::from("/test")));
        tab.current_password.set(Some("secret".to_string()));
        // Set a global value
        signals.search_text.set("test".to_string());

        // Verify values are set
        assert!(tab.archive_path.get().is_some());
        assert_eq!(signals.search_text.get(), "test");

        // Reset replaces tabs collection (new TabState = defaults)
        signals.reset();
        let tab2 = signals.tabs.get().active().clone();

        // Per-tab signals reset via new TabState
        assert!(tab2.archive_path.get().is_none());
        assert!(tab2.current_password.get().is_none());
        // Global signal reset
        assert!(signals.search_text.get().is_empty());
    }

    #[test]
    fn test_signal_set_get_roundtrip() {
        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();

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
        tab2.archive_path.set(Some(PathBuf::from("/shared.zip")));
        assert_eq!(tab1.archive_path.get(), Some(PathBuf::from("/shared.zip")));
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
