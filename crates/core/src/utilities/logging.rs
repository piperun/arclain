//! Centralized logging configuration for arclain
//!
//! Provides structured logging with four levels:
//! - ERROR: Critical errors that prevent operation
//! - WARNING: Non-critical issues that should be noted
//! - INFO: General informational messages
//! - FINE (DEBUG): Detailed diagnostic information
//!
//! Logs are written to both console and rotating log files in the `logs` directory.

use std::path::{Path, PathBuf};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan, time::OffsetTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Directory that stores arclain's application log files.
pub fn app_log_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("arclain")
        .join("logs")
}

/// Name of the subdirectory of the app log directory that holds
/// per-plugin log files.
///
/// Exposed as a constant so a caller that already knows its own log
/// directory can locate the plugin logs inside it without re-deriving
/// the OS-conventional root through [`app_log_dir`].
pub const PLUGIN_LOG_SUBDIR: &str = "plugins";

/// Directory that stores per-plugin log files.
pub fn plugin_log_dir() -> PathBuf {
    app_log_dir().join(PLUGIN_LOG_SUBDIR)
}

/// File name of the app log for a specific local date.
///
/// The one place arclain's daily log-file naming lives. The path
/// helpers below build on it, and [`init_logging`] takes its file
/// appender's name from [`current_app_log_path`] rather than spelling
/// the pattern a second time, so a reader can never disagree with the
/// writer about which file the current session lands in.
fn app_log_file_name_for_date(date: chrono::NaiveDate) -> String {
    format!("arclain-{}.log", date.format("%Y-%m-%d"))
}

/// File name of the app log for today's local date.
///
/// Same reason [`PLUGIN_LOG_SUBDIR`] is public: a caller holding its own
/// log directory needs the file name without a root baked into it.
pub fn current_app_log_file_name() -> String {
    app_log_file_name_for_date(chrono::Local::now().date_naive())
}

/// App log path for a specific local date.
pub fn app_log_path_for_date(date: chrono::NaiveDate) -> PathBuf {
    app_log_dir().join(app_log_file_name_for_date(date))
}

/// App log path for today's local date.
pub fn current_app_log_path() -> PathBuf {
    app_log_dir().join(current_app_log_file_name())
}

fn prepare_file_appender(
    log_dir: &Path,
    log_filename: &str,
) -> Result<RollingFileAppender, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(log_dir)?;
    Ok(RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_prefix(log_filename)
        .build(log_dir)?)
}

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
    let log_dir = app_log_dir();

    // Create filename with format: arclain-YYYY-MM-DD.log
    let log_filename = current_app_log_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("arclain.log")
        .to_string();

    // Use 'never' rotation with our custom filename (we handle date in filename)
    let file_appender = prepare_file_appender(&log_dir, &log_filename)?;
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Create local time formatter
    let local_time = OffsetTime::local_rfc_3339().expect("Failed to get local time offset");

    // File layer - always write to file
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_timer(local_time.clone())
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
            .with_timer(local_time)
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
    let log_dir = app_log_dir();

    // Create filename with format: arclain-YYYY-MM-DD.log
    let log_filename = current_app_log_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("arclain.log")
        .to_string();

    let file_appender = prepare_file_appender(&log_dir, &log_filename)?;
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
    // Use test name in filename with timestamp
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let log_filename = format!("{}_{}.log", test_name, timestamp);

    let file_appender = prepare_file_appender(&log_dir, &log_filename)?;
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
