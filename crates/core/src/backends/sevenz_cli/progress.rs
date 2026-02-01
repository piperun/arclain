//! Progress tracking types for 7-Zip operations

use std::sync::mpsc;

/// Progress event from 7-Zip streaming output.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    /// Progress percentage (0..=100)
    pub percent: u8,
    /// Optional status message
    pub message: Option<String>,
}

/// Handle for a running 7-Zip process with progress updates.
pub struct ChildWithProgress {
    /// The underlying child process
    pub child: std::process::Child,
    /// Receiver for progress updates
    pub rx: mpsc::Receiver<ProgressUpdate>,
}
