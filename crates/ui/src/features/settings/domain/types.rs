//! Shared types for settings pages
//!
//! Contains enums, state structs, and action types used across settings pages.

use crate::features::password_management::dialogs::zip_pass_rules::PasswordRule;
use std::collections::HashMap;

/// Encrypted CRC policy for encrypted archives
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EncryptedCrcPolicy {
    OnOpen,
    PromptOnOpen,
    OnAccess,
}

impl Default for EncryptedCrcPolicy {
    fn default() -> Self {
        Self::OnOpen
    }
}

impl EncryptedCrcPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            EncryptedCrcPolicy::OnOpen => "on_open",
            EncryptedCrcPolicy::PromptOnOpen => "prompt_on_open",
            EncryptedCrcPolicy::OnAccess => "on_access",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            EncryptedCrcPolicy::OnOpen => "When opening archive",
            EncryptedCrcPolicy::PromptOnOpen => "Prompt when opening archive",
            EncryptedCrcPolicy::OnAccess => "When opening/editing file",
        }
    }
}

/// Actions that can be triggered from settings pages
#[derive(Debug, Clone)]
pub enum SettingsAction {
    /// Save security settings
    SaveSecurity {
        key_file_path: Option<String>,
        secrets_db_path: Option<String>,
        encrypted_crc_policy: Option<String>,
    },
    /// Move vault to new location
    MoveVault { dest_path: String },
    /// Rekey vault with new key
    RekeyVault { new_key_file_path: String },
    /// Save password rules
    SavePasswordRules { rules: Vec<PasswordRule> },
    /// Save archives settings
    SaveArchives { temp_dir: Option<String> },
    /// Install a plugin from a .wasm file
    InstallPlugin { wasm_path: String },
    /// Clear the cache index (database entries)
    ClearCacheIndex,
    /// Clear the cache content (files on disk)
    ClearCacheContent,
    /// Garbage collect orphaned cache entries
    GarbageCollectCache,
    /// Clean up old search cache (older than 7 days)
    CleanOldSearchCache,
    /// Fix/migrate cache entries (update cache_type and product_id)
    MigrateCacheEntries,
    /// Save general settings
    SaveGeneral { open_nested_in_new_tab: bool },
    /// Save network settings
    SaveNetwork {
        socks5_enabled: bool,
        socks5_address: Option<String>,
        socks5_username: Option<String>,
        socks5_password: Option<String>,
    },
    /// Test network settings
    TestNetwork {
        socks5_enabled: bool,
        socks5_address: Option<String>,
        socks5_username: Option<String>,
        socks5_password: Option<String>,
    },
    /// Save keyboard and mouse settings
    SaveKeyboardMouse { bindings: HashMap<String, String> },
    /// Save gameta server settings
    SaveServer {
        enabled: bool,
        url: Option<String>,
        api_key: Option<String>,
    },
    /// Test gameta server connection
    TestServer {
        url: String,
        api_key: Option<String>,
    },
    /// Navigate to another settings page
    NavigateTo(crate::core::navigation::SettingsPage),
    /// Save the currently edited organization rule
    SaveEditedRule,
}

use arclain_signals::Signal;

/// State for the network settings page
#[derive(Clone)]
pub struct NetworkSettingsState {
    pub socks5_enabled: Signal<bool>,
    pub socks5_address: Signal<String>,
    pub socks5_username: Signal<String>,
    pub socks5_password: Signal<String>,
    pub connection_test_status: Signal<ConnectionTestStatus>,
}

impl Default for NetworkSettingsState {
    fn default() -> Self {
        Self {
            socks5_enabled: Signal::new(false),
            socks5_address: Signal::new(String::new()),
            socks5_username: Signal::new(String::new()),
            socks5_password: Signal::new(String::new()),
            connection_test_status: Signal::new(ConnectionTestStatus::Idle),
        }
    }
}

/// Result of a single test step
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TestStepResult {
    pub name: String,
    pub passed: bool,
    pub message: Option<String>,
}

/// Complete connection test result with all steps
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ConnectionTestResult {
    pub steps: Vec<TestStepResult>,
    pub success: bool,
    /// Final result message (IP/country on success)
    pub result_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionTestStatus {
    #[default]
    Idle,
    Testing,
    Complete(ConnectionTestResult),
}

/// State for the general settings page
#[derive(Clone)]
pub struct GeneralSettingsState {
    /// Whether to open nested archives in a new tab (true) or replace current view (false)
    pub open_nested_in_new_tab: Signal<bool>,
}

impl Default for GeneralSettingsState {
    fn default() -> Self {
        Self {
            open_nested_in_new_tab: Signal::new(false),
        }
    }
}

/// Status of a gameta server connection test
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ServerConnectionStatus {
    #[default]
    Idle,
    Testing,
    Connected(String),
    Failed(String),
}

/// State for the server settings page
#[derive(Clone)]
pub struct ServerSettingsState {
    pub enabled: Signal<bool>,
    pub url: Signal<String>,
    pub api_key: Signal<String>,
    pub connection_status: Signal<ServerConnectionStatus>,
}

impl Default for ServerSettingsState {
    fn default() -> Self {
        Self {
            enabled: Signal::new(false),
            url: Signal::new(String::new()),
            api_key: Signal::new(String::new()),
            connection_status: Signal::new(ServerConnectionStatus::Idle),
        }
    }
}

/// State for the security settings page
#[derive(Clone)]
pub struct SecuritySettingsState {
    pub key_file_path: Signal<String>,
    pub secrets_db_path: Signal<String>,
    pub encrypted_crc_policy: Signal<EncryptedCrcPolicy>,
    pub info: Signal<String>,
    pub error: Signal<String>,
}

impl Default for SecuritySettingsState {
    fn default() -> Self {
        Self {
            key_file_path: Signal::new(String::new()),
            secrets_db_path: Signal::new(String::new()),
            encrypted_crc_policy: Signal::new(EncryptedCrcPolicy::default()),
            info: Signal::new(String::new()),
            error: Signal::new(String::new()),
        }
    }
}

/// State for the archives settings page
#[derive(Clone)]
pub struct ArchivesSettingsState {
    pub temp_dir: Signal<String>,
    // Checksum settings
    pub checksum_enabled: Signal<bool>,
    pub checksum_mode: Signal<ChecksumMode>,
    pub checksum_algorithm: Signal<ChecksumAlgorithm>,
    pub verify_after_extract: Signal<bool>,
    pub verify_after_organize: Signal<bool>,
}

impl Default for ArchivesSettingsState {
    fn default() -> Self {
        Self {
            temp_dir: Signal::new(String::new()),
            checksum_enabled: Signal::new(false),
            checksum_mode: Signal::new(ChecksumMode::default()),
            checksum_algorithm: Signal::new(ChecksumAlgorithm::default()),
            verify_after_extract: Signal::new(false),
            verify_after_organize: Signal::new(false),
        }
    }
}

/// Checksum verification mode (mirrors VerifyMode)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChecksumMode {
    #[default]
    Simple,
    Full,
}

impl ChecksumMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            ChecksumMode::Simple => "Simple (root hash only)",
            ChecksumMode::Full => "Full (all file hashes)",
        }
    }
}

/// Checksum algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChecksumAlgorithm {
    #[default]
    Crc32,
    XxHash,
    Sha256,
}

impl ChecksumAlgorithm {
    pub fn display_name(&self) -> &'static str {
        match self {
            ChecksumAlgorithm::Crc32 => "CRC32 (fastest)",
            ChecksumAlgorithm::XxHash => "XXHash (fast, modern)",
            ChecksumAlgorithm::Sha256 => "SHA-256 (secure, slower)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // EncryptedCrcPolicy
    // =========================================================================

    #[test]
    fn encrypted_crc_policy_default() {
        assert_eq!(EncryptedCrcPolicy::default(), EncryptedCrcPolicy::OnOpen);
    }

    #[test]
    fn encrypted_crc_policy_as_str() {
        assert_eq!(EncryptedCrcPolicy::OnOpen.as_str(), "on_open");
        assert_eq!(EncryptedCrcPolicy::PromptOnOpen.as_str(), "prompt_on_open");
        assert_eq!(EncryptedCrcPolicy::OnAccess.as_str(), "on_access");
    }

    #[test]
    fn encrypted_crc_policy_display_name() {
        assert_eq!(
            EncryptedCrcPolicy::OnOpen.display_name(),
            "When opening archive"
        );
        assert_eq!(
            EncryptedCrcPolicy::PromptOnOpen.display_name(),
            "Prompt when opening archive"
        );
        assert_eq!(
            EncryptedCrcPolicy::OnAccess.display_name(),
            "When opening/editing file"
        );
    }

    // =========================================================================
    // ChecksumMode
    // =========================================================================

    #[test]
    fn checksum_mode_default() {
        assert_eq!(ChecksumMode::default(), ChecksumMode::Simple);
    }

    #[test]
    fn checksum_mode_display_name() {
        assert_eq!(
            ChecksumMode::Simple.display_name(),
            "Simple (root hash only)"
        );
        assert_eq!(ChecksumMode::Full.display_name(), "Full (all file hashes)");
    }

    // =========================================================================
    // ChecksumAlgorithm
    // =========================================================================

    #[test]
    fn checksum_algorithm_default() {
        assert_eq!(ChecksumAlgorithm::default(), ChecksumAlgorithm::Crc32);
    }

    #[test]
    fn checksum_algorithm_display_name() {
        assert_eq!(
            ChecksumAlgorithm::Crc32.display_name(),
            "CRC32 (fastest)"
        );
        assert_eq!(
            ChecksumAlgorithm::XxHash.display_name(),
            "XXHash (fast, modern)"
        );
        assert_eq!(
            ChecksumAlgorithm::Sha256.display_name(),
            "SHA-256 (secure, slower)"
        );
    }

    // =========================================================================
    // ConnectionTestStatus / ConnectionTestResult
    // =========================================================================

    #[test]
    fn connection_test_status_default_is_idle() {
        assert_eq!(ConnectionTestStatus::default(), ConnectionTestStatus::Idle);
    }

    #[test]
    fn connection_test_result_default() {
        let result = ConnectionTestResult::default();
        assert!(result.steps.is_empty());
        assert!(!result.success);
        assert!(result.result_message.is_none());
    }
}
