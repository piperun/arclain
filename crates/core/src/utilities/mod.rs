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

pub mod checked_relative_path;
pub mod checksum_service;

pub mod dlsite;
pub mod file_opener;
pub mod filesystem;
pub mod logging;
pub mod password_matcher;
pub mod process;
pub mod time;
pub mod title_filter;

#[allow(unused_imports)] // Internal boundary for archive and plan consumers.
pub(crate) use checked_relative_path::CheckedRelativePath;
pub use checksum_service::{ChecksumService, RecoveryAction, VerifyResult};
pub use dlsite::{detect_dlsite_code, has_dlsite_code};
pub use file_opener::{FileOpener, OpenStrategy};
pub use filesystem::rename_no_replace;
pub use logging::{
    app_log_dir, app_log_path_for_date, current_app_log_path, init_logging,
    init_logging_with_filter, plugin_log_dir,
};
pub use password_matcher::{auto_password_for, PassRule};
pub use process::hide_console;
pub use time::{unix_seconds, unix_seconds_i64};
pub use title_filter::{sanitize_title, TitleFilterConfig};

pub mod proxy;
pub use proxy::{apply_proxy_to_client, effective_plugin_proxy_map, resolve_proxy_config};
