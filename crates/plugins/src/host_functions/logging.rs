//! Logging and messaging operations

use super::HostFunctions;
use crate::arclain::plugin::host::LogLevel;
use tracing::{error, warn};

const MAX_NETWORK_LOG_ENTRIES: usize = 256;
const MAX_NETWORK_LOG_BYTES: usize = 256 * 1024;
const MAX_NETWORK_LOG_MESSAGE_BYTES: usize = 4 * 1024;

impl HostFunctions {
    pub(super) fn impl_log(&mut self, level: LogLevel, message: String) {
        if self.plugin_logger.is_deferred() {
            return;
        }

        let prefix = match level {
            LogLevel::Error => "ERROR ",
            LogLevel::Warn => "WARN  ",
            LogLevel::Info => "INFO  ",
            LogLevel::Debug => "DEBUG ",
            LogLevel::Trace => "TRACE ",
        };
        if !self.plugin_logger.write_parts(&[prefix, &message]) {
            return;
        }

        // Escalate only entries admitted by the per-plugin logger's rate,
        // entry-size, and daily-byte caps.
        match level {
            LogLevel::Error => error!("[Plugin {}] {}", self.plugin_id, message),
            LogLevel::Warn => warn!("[Plugin {}] {}", self.plugin_id, message),
            _ => {}
        }
    }

    pub(super) fn impl_log_network_activity(&mut self, mut message: String) {
        if message.len() > MAX_NETWORK_LOG_MESSAGE_BYTES
            || message.capacity() > MAX_NETWORK_LOG_MESSAGE_BYTES
        {
            let mut end = message.len().min(MAX_NETWORK_LOG_MESSAGE_BYTES);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message = message[..end].to_owned();
        }

        let mut log = self.network_log.lock();
        let mut retained_bytes = log.iter().map(|(_, entry)| entry.len()).sum::<usize>();
        while !log.is_empty()
            && (log.len() >= MAX_NETWORK_LOG_ENTRIES
                || retained_bytes.saturating_add(message.len()) > MAX_NETWORK_LOG_BYTES)
        {
            let (_, evicted) = log.remove(0);
            retained_bytes = retained_bytes.saturating_sub(evicted.len());
        }
        log.push((std::time::SystemTime::now(), message));
    }

    pub(super) fn impl_show_message(&mut self, title: String, message: String) {
        // The original design buffered `(title, message)` into a
        // `pending_messages` Vec that the host would drain to surface
        // a modal. That drain never landed; plugins that want a modal
        // emit `PluginAction::ShowToast` instead. We keep this entry
        // point only because plugins invoke the `show-message` WIT
        // import; reduce it to a structured log line.
        let _ = self
            .plugin_logger
            .write_parts(&["MESSAGE ", &title, " - ", &message]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PluginId;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tracing_test::traced_test;

    fn capped_host(byte_cap: u64) -> (tempfile::TempDir, HostFunctions) {
        let log_dir = tempfile::tempdir().unwrap();
        let mut host = HostFunctions::new_with_plugin_log_dir(
            "logging-security-test".to_string(),
            Default::default(),
            0,
            HashMap::new(),
            log_dir.path(),
        )
        .unwrap();
        host.plugin_logger = Arc::new(super::super::PluginLogger::with_byte_cap(
            &PluginId::parse("logging-security-test").unwrap(),
            log_dir.path(),
            byte_cap,
        ));
        (log_dir, host)
    }

    #[traced_test]
    #[test]
    fn metadata_validation_discards_escalated_plugin_logs() {
        let mut host =
            HostFunctions::new_for_metadata_validation("temp-validation".to_string()).unwrap();

        host.impl_log(
            LogLevel::Warn,
            "must not reach application logging".to_string(),
        );

        assert!(!logs_contain("must not reach application logging"));
    }

    #[traced_test]
    #[test]
    fn dropped_guest_warning_does_not_reach_application_logging() {
        let (_log_dir, mut host) = capped_host(1);

        host.impl_log(
            LogLevel::Warn,
            "rejected-warning-must-not-reach-application-log".to_string(),
        );

        assert!(!logs_contain(
            "rejected-warning-must-not-reach-application-log"
        ));
    }

    #[traced_test]
    #[test]
    fn oversized_guest_warning_is_rejected_before_application_logging() {
        let (_log_dir, mut host) = capped_host(1024 * 1024);
        let marker = "oversized-warning-must-not-reach-application-log";
        let message = format!("{marker}{}", "x".repeat(32 * 1024));

        host.impl_log(LogLevel::Warn, message);

        assert!(!logs_contain(marker));
    }

    #[traced_test]
    #[test]
    fn dropped_show_message_does_not_reach_application_logging() {
        let (_log_dir, mut host) = capped_host(1);

        host.impl_show_message(
            "rejected-dialog-title".to_string(),
            "rejected-dialog-message-must-not-reach-application-log".to_string(),
        );

        assert!(!logs_contain(
            "rejected-dialog-message-must-not-reach-application-log"
        ));
    }

    #[traced_test]
    #[test]
    fn admitted_show_message_stays_in_bounded_plugin_log_only() {
        let title_marker = "dialog-title-must-not-reach-application-log";
        let message_marker = "dialog-message-must-not-reach-application-log";
        let (log_dir, mut host) = capped_host(1024 * 1024);

        host.impl_show_message(title_marker.to_string(), message_marker.to_string());

        let plugin_log = std::fs::read_dir(log_dir.path())
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect::<String>();
        assert!(plugin_log.contains(title_marker));
        assert!(plugin_log.contains(message_marker));
        assert!(!logs_contain(title_marker));
        assert!(!logs_contain(message_marker));
    }

    #[test]
    fn network_activity_log_has_message_entry_and_aggregate_bounds() {
        const EXPECTED_MAX_ENTRIES: usize = 256;
        const EXPECTED_MAX_MESSAGE_BYTES: usize = 4 * 1024;
        const EXPECTED_MAX_TOTAL_BYTES: usize = 256 * 1024;
        let (_log_dir, mut host) = capped_host(1024);

        for index in 0..=EXPECTED_MAX_ENTRIES {
            host.impl_log_network_activity(format!(
                "network-entry-{index:03}-{}",
                "x".repeat(EXPECTED_MAX_MESSAGE_BYTES * 2)
            ));
        }

        let log = host.network_log.lock();
        assert!(log.len() <= EXPECTED_MAX_ENTRIES);
        assert!(log
            .iter()
            .all(|(_, message)| message.len() <= EXPECTED_MAX_MESSAGE_BYTES));
        assert!(
            log.iter()
                .all(|(_, message)| message.capacity() <= EXPECTED_MAX_MESSAGE_BYTES),
            "truncated messages retained attacker-sized allocations"
        );
        assert!(
            log.iter().map(|(_, message)| message.len()).sum::<usize>() <= EXPECTED_MAX_TOTAL_BYTES
        );
        assert!(
            log.iter()
                .all(|(_, message)| !message.starts_with("network-entry-000-")),
            "oldest entry was not deterministically evicted"
        );
    }
}
