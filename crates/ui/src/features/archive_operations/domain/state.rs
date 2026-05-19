use crate::core::tabs::{OpGuard, TabState};
use arclain_core::backends::sevenz_cli::ProgressUpdate;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

pub struct ArchiveOperationsState {
    // Extraction progress state
    pub extraction_rx: Option<Receiver<ProgressUpdate>>,
    pub extraction_child: Option<std::process::Child>,
    pub extraction_minimized: bool,
    pub extraction_started: Option<Instant>,
    /// RAII counter: incremented when extraction starts, dropped when it ends.
    pub extraction_op_guard: Option<OpGuard>,
    /// The tab that originated this extraction. Checked for `tab_cancel` in
    /// `update_extraction_progress` to implement cooperative cancellation.
    pub extraction_origin_tab: Option<Arc<TabState>>,

    // Conversion progress state
    pub conversion_rx: Option<Receiver<ProgressUpdate>>,
    pub conversion_child: Option<std::process::Child>,
    pub conversion_minimized: bool,
    pub conversion_started: Option<Instant>,
    /// RAII counter: incremented when conversion starts, dropped when it ends.
    pub conversion_op_guard: Option<OpGuard>,
    /// The tab that originated this conversion. Checked for `tab_cancel` in
    /// `update_conversion_progress` to implement cooperative cancellation.
    pub conversion_origin_tab: Option<Arc<TabState>>,

    // Drag-out progress state
    pub drag_rx: Option<Receiver<ProgressUpdate>>,
    pub drag_started: Option<Instant>,
}

impl Default for ArchiveOperationsState {
    fn default() -> Self {
        Self {
            extraction_rx: None,
            extraction_child: None,
            extraction_minimized: false,
            extraction_started: None,
            extraction_op_guard: None,
            extraction_origin_tab: None,
            conversion_rx: None,
            conversion_child: None,
            conversion_minimized: false,
            conversion_started: None,
            conversion_op_guard: None,
            conversion_origin_tab: None,
            drag_rx: None,
            drag_started: None,
        }
    }
}
