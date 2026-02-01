//! UnRAR CLI backend - shells out to official unrar.exe/UnRAR for RAR extraction
//!
//! This backend is used when the native `unrar` crate fails (e.g., Unicode path issues on Windows).
//! On Windows, it checks for WinRAR's UnRAR.exe in common installation paths.
//! On Linux, it looks for `unrar` in PATH.
//!
//! ## Module Structure
//! - `runner` - UnrarCli struct and command execution helpers
//! - `parser` - Output parsing for archive listings
//! - `backend` - ArchiveBackend trait implementation

mod backend;
mod parser;
mod runner;

pub use runner::UnrarCli;
