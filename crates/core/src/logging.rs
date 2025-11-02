//! Centralized logging configuration for arclain
//!
//! Provides structured logging with four levels:
//! - ERROR: Critical errors that prevent operation
//! - WARNING: Non-critical issues that should be noted
//! - INFO: General informational messages
//! - FINE (DEBUG): Detailed diagnostic information

use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Initialize the logging system with default configuration
///
/// Log levels can be controlled via RUST_LOG environment variable:
/// - RUST_LOG=error - Only errors
/// - RUST_LOG=warn - Warnings and errors
/// - RUST_LOG=info - Info, warnings, and errors (default)
/// - RUST_LOG=debug - Fine/debug level, info, warnings, and errors
/// - RUST_LOG=trace - All logging
pub fn init_logging() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()?;

    Ok(())
}

/// Initialize logging with custom filter
pub fn init_logging_with_filter(filter: &str) -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::new(filter);

    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()?;

    Ok(())
}

/// Convenience macros for logging at different levels
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        tracing::error!($($arg)*)
    };
}

#[macro_export]
macro_rules! log_warning {
    ($($arg:tt)*) => {
        tracing::warn!($($arg)*)
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        tracing::info!($($arg)*)
    };
}

#[macro_export]
macro_rules! log_fine {
    ($($arg:tt)*) => {
        tracing::debug!($($arg)*)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_init() {
        // This may fail if already initialized, which is fine for tests
        let _ = init_logging();
    }
}
