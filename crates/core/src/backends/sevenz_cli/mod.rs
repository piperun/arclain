//! 7-Zip CLI backend for archive operations
//!
//! This module provides a command-line interface wrapper for 7-Zip,
//! supporting archive listing, extraction, and creation with progress tracking.
//!
//! ## Module Structure
//! - `runner` - Core SevenZipCli struct and command execution helpers
//! - `parser` - Output parsing for archive listings
//! - `backend` - ArchiveBackend trait implementation
//! - `progress` - Progress tracking types
//! - `progress_ops` - Progress-enabled extraction operations

mod backend;
mod parser;
mod progress;
mod progress_ops;
mod runner;

pub use progress::{ChildWithProgress, ProgressUpdate};
pub use runner::SevenZipCli;
