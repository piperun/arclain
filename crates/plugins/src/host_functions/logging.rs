//! Logging and messaging operations

use super::HostFunctions;
use crate::arclain::plugin::host::LogLevel;
use tracing::{error, info, warn};

impl HostFunctions {
    pub(super) fn impl_log(&mut self, level: LogLevel, message: String) {
        // Always escalate ERROR and WARN to arclain.log so operators
        // see them without grepping per-plugin files. INFO/DEBUG/TRACE
        // go to the per-plugin file only.
        match level {
            LogLevel::Error => error!("[Plugin {}] {}", self.plugin_id, message),
            LogLevel::Warn => warn!("[Plugin {}] {}", self.plugin_id, message),
            _ => {}
        }

        let prefixed = match level {
            LogLevel::Error => format!("ERROR {}", message),
            LogLevel::Warn => format!("WARN  {}", message),
            LogLevel::Info => format!("INFO  {}", message),
            LogLevel::Debug => format!("DEBUG {}", message),
            LogLevel::Trace => format!("TRACE {}", message),
        };
        self.plugin_logger.write(&prefixed);
    }

    pub(super) fn impl_log_network_activity(&mut self, message: String) {
        // Store in network activity log for UI display
        self.network_log
            .lock()
            .push((std::time::SystemTime::now(), message));
    }

    pub(super) fn impl_show_message(&mut self, title: String, message: String) {
        // The original design buffered `(title, message)` into a
        // `pending_messages` Vec that the host would drain to surface
        // a modal. That drain never landed; plugins that want a modal
        // emit `PluginAction::ShowToast` instead. We keep this entry
        // point only because plugins invoke the `show-message` WIT
        // import; reduce it to a structured log line.
        info!(
            "[Plugin] Requesting message dialog: {} - {}",
            title, message
        );
    }
}
