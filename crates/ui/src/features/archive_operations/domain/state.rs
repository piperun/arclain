use arclain_core::backends::sevenz_cli::ProgressUpdate;
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(Default)]
pub struct ArchiveOperationsState {
    // Extraction progress state
    pub extraction_rx: Option<Receiver<ProgressUpdate>>,
    pub extraction_child: Option<std::process::Child>,
    pub extraction_minimized: bool,
    pub extraction_started: Option<Instant>,

    // Conversion progress state
    pub conversion_rx: Option<Receiver<ProgressUpdate>>,
    pub conversion_child: Option<std::process::Child>,
    pub conversion_minimized: bool,
    pub conversion_started: Option<Instant>,

    // Drag-out progress state
    pub drag_rx: Option<Receiver<ProgressUpdate>>,
    pub drag_started: Option<Instant>,
}
