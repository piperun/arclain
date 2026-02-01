//! 7-Zip CLI backend for archive operations
//!
//! This module provides a command-line interface wrapper for 7-Zip,
//! supporting archive listing, extraction, and creation with progress tracking.

mod cli;
mod progress;

pub use cli::SevenZipCli;
pub use progress::{ChildWithProgress, ProgressUpdate};
