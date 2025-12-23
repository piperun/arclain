//! Application-level reactive signals
//!
//! This module provides reactive signals for async-updated state
//! that needs to trigger UI updates when changed from background threads.

use arclain_core::ArchiveEntry;
use arclain_signals::{Signal, SignalContext};
use std::path::PathBuf;

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
        }
    }

    /// Bind all signals to egui context for automatic repaint.
    pub fn bind_to_context(&self, ctx: &egui::Context) {
        let signal_ctx = SignalContext::new(ctx.clone());
        signal_ctx.bind(&self.entries);
        signal_ctx.bind(&self.metadata);
        signal_ctx.bind(&self.loading);
        signal_ctx.bind(&self.archive_path);
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
    }
}

impl Default for AppSignals {
    fn default() -> Self {
        Self::new()
    }
}
