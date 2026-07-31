//! Frontend-neutral startup logging.
//!
//! Frontends initialize tracing before constructing their window, but the
//! log location is application state. This module keeps both the subscriber
//! setup and the paths consumed by a diagnostics view behind `arclain_app`.

use std::path::PathBuf;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::AppPaths;

/// Files and directories a frontend diagnostics view consumes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LoggingPathsDto {
    pub app_log_path: PathBuf,
    pub plugin_log_dir: PathBuf,
}

/// Initialize Arclain's tracing subscriber in `paths.log_dir` and return the
/// exact paths a frontend log viewer should read.
pub fn initialize(paths: &AppPaths) -> Result<LoggingPathsDto, ApplicationError> {
    arclain_core::utilities::init_logging_in(&paths.log_dir).map_err(|error| {
        ApplicationError::new(
            ApplicationErrorKind::Persistence,
            "failed to initialize application logging",
        )
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::Fatal)
    })?;

    Ok(LoggingPathsDto {
        app_log_path: paths.current_app_log_file(),
        plugin_log_dir: paths.plugin_log_dir(),
    })
}
