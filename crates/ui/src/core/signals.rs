//! Application-level reactive signals
//!
//! This module provides reactive signals for async-updated state
//! that needs to trigger UI updates when changed from background threads.

use crate::core::operations::archive::ArchiveInfo;
use arclain_core::ArchiveEntry;
use arclain_db::UiItem;
use arclain_signals::{Signal, SignalContext};
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
    pub entries: Signal<Vec<ArchiveEntry>>,

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
}

impl AppSignals {
    /// Create new signals with default values.
    pub fn new() -> Self {
        Self {
            entries: Signal::new(Vec::new()),
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
        // Note: ui_ready is not bound to repaint - it's a control signal, not display
    }

    /// Reset all signals to default state.
    #[allow(dead_code)]
    pub fn reset(&self) {
        self.entries.set(Vec::new());
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
    }
}

impl Default for AppSignals {
    fn default() -> Self {
        Self::new()
    }
}
