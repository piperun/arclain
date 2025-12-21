//! Shared utility functions
//!
//! This module provides common utilities used across the crate:
//! - Logging configuration
//! - Title sanitization for filenames
//! - File opening strategies
//! - Checksum verification
//! - Content caching
//! - DLsite code detection
//! - Password matching

pub mod checksum_service;

pub mod dlsite;
pub mod file_opener;
pub mod logging;
pub mod password_matcher;
pub mod title_filter;

pub use checksum_service::{ChecksumService, RecoveryAction, VerifyResult};
pub use dlsite::{detect_dlsite_code, has_dlsite_code};
pub use file_opener::{FileOpener, OpenStrategy};
pub use logging::{init_logging, init_logging_with_filter};
pub use password_matcher::{auto_password_for, PassRule};
pub use title_filter::{sanitize_title, TitleFilterConfig};
