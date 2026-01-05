//! Centralized logging configuration for arclain
//!
//! Provides structured logging with four levels:
//! - ERROR: Critical errors that prevent operation
//! - WARNING: Non-critical issues that should be noted
//! - INFO: General informational messages
//! - FINE (DEBUG): Detailed diagnostic information
//!
//! Logs are written to both console and rotating log files in the `logs` directory.

use std::path::PathBuf;
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
///
/// Logs are written to:
/// - Console (stdout/stderr) - only in debug builds
/// - Rolling log files in `%APPDATA%/arclain/logs/` directory (daily rotation)
pub fn init_logging() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Use platform-specific app data directory
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("arclain")
        .join("logs");

    // Ensure log directory exists
    std::fs::create_dir_all(&log_dir)?;

    // Create filename with format: arclain-YYYY-MM-DD.log
    let today = chrono::Local::now().format("%Y-%m-%d");
    let log_filename = format!("arclain-{}.log", today);

    // Use 'never' rotation with our custom filename (we handle date in filename)
    let file_appender = tracing_appender::rolling::never(&log_dir, &log_filename);
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // File layer - always write to file
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_ansi(false); // Disable ANSI colors in file logs

    // Only add console layer in debug builds (when console is visible)
    #[cfg(debug_assertions)]
    {
        let console_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_file(true)
            .with_line_number(true)
            .with_span_events(FmtSpan::NONE);

        tracing_subscriber::registry()
            .with(filter)
            .with(console_layer)
            .with(file_layer)
            .try_init()?;
    }

    #[cfg(not(debug_assertions))]
    {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .try_init()?;
    }

    // Store the guard to keep the non-blocking writer alive
    // This is a workaround since we can't return it
    std::mem::forget(_guard);

    Ok(())
}

/// Initialize logging with custom filter
///
/// Logs are written to rolling log files in `%APPDATA%/arclain/logs/` directory.
pub fn init_logging_with_filter(filter: &str) -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::new(filter);

    // Use platform-specific app data directory
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("arclain")
        .join("logs");

    // Ensure log directory exists
    std::fs::create_dir_all(&log_dir)?;

    // Create filename with format: arclain-YYYY-MM-DD.log
    let today = chrono::Local::now().format("%Y-%m-%d");
    let log_filename = format!("arclain-{}.log", today);

    let file_appender = tracing_appender::rolling::never(&log_dir, &log_filename);
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // File layer - always write to file
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_ansi(false);

    // Only add console layer in debug builds
    #[cfg(debug_assertions)]
    {
        let console_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_file(true)
            .with_line_number(true)
            .with_span_events(FmtSpan::NONE);

        tracing_subscriber::registry()
            .with(filter)
            .with(console_layer)
            .with(file_layer)
            .try_init()?;
    }

    #[cfg(not(debug_assertions))]
    {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .try_init()?;
    }

    // Store the guard to keep the non-blocking writer alive
    std::mem::forget(_guard);

    Ok(())
}

/// Initialize logging specifically for tests
///
/// Creates test-specific log files in `./logs/tests/` directory with the test name.
/// This helps separate test logs from application logs for easier debugging.
///
/// # Arguments
/// * `test_name` - Name of the test (used in log filename)
///
/// # Example
/// ```no_run
/// use arclain_core::utilities::logging::init_test_logging;
///
/// #[test]
/// fn my_test() {
///     init_test_logging("my_test").unwrap();
///     // Test code here
/// }
/// ```
pub fn init_test_logging(test_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));

    // Create test-specific log directory
    let log_dir = PathBuf::from("./logs/tests");
    std::fs::create_dir_all(&log_dir)?;

    // Use test name in filename with timestamp
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let log_filename = format!("{}_{}.log", test_name, timestamp);

    let file_appender = tracing_appender::rolling::never(&log_dir, log_filename);
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Console layer (optional for tests, can be disabled if needed)
    let console_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(true) // Include thread IDs for parallel test debugging
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NONE)
        .with_test_writer(); // Use test output capture

    // File layer for test logs
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::CLOSE) // Include span timing for performance analysis
        .with_ansi(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .ok(); // Ignore error if already initialized by another test

    // Store the guard to keep the non-blocking writer alive
    std::mem::forget(_guard);

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
mod tests;
