//! Logging and messaging operations

use super::HostFunctions;
use crate::arclain::plugin::host::LogLevel;
use tracing::{debug, error, info, trace, warn};

impl HostFunctions {
    pub(super) fn impl_log(&mut self, level: LogLevel, message: String) {
        match level {
            LogLevel::Error => error!("[Plugin] {}", message),
            LogLevel::Warn => warn!("[Plugin] {}", message),
            LogLevel::Info => info!("[Plugin] {}", message),
            LogLevel::Debug => debug!("[Plugin] {}", message),
            LogLevel::Trace => trace!("[Plugin] {}", message),
        }
    }

    pub(super) fn impl_log_network_activity(&mut self, message: String) {
        // Store in network activity log for UI display
        self.network_log
            .lock()
            .push((std::time::SystemTime::now(), message));
    }

    pub(super) fn impl_show_message(&mut self, title: String, message: String) {
        info!(
            "[Plugin] Requesting message dialog: {} - {}",
            title, message
        );
        self.pending_messages.lock().push((title, message));
    }

    pub(super) fn impl_copy_to_clipboard(&mut self, text: String) -> bool {
        info!("[Plugin] Copying to clipboard: {}", text);
        *self.pending_clipboard.lock() = Some(text);
        true
    }
}
