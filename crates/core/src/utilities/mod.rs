//! Shared utility functions
//! 
//! This module provides common utilities used across the crate:
//! - Logging configuration
//! - Title sanitization for filenames
//! - File opening strategies

pub mod file_opener;
pub mod logging;
pub mod title_filter;

pub use file_opener::{FileOpener, OpenStrategy};
pub use logging::{init_logging, init_logging_with_filter};
pub use title_filter::{sanitize_title, sanitize_title_with_config, TitleFilterConfig};