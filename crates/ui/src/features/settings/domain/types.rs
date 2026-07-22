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
        // On-open eagerly fired one 7z subprocess per encrypted entry, which
        // turned a 5933-entry encrypted RAR into a ~6-minute hang. On-access
        // defers CRC to the moment the user inspects an entry.
        Self::OnAccess
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
#[derive(Clone)]
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
    SaveGeneral {
        open_nested_in_new_tab: bool,
        drop_behavior: arclain_core::DropBehavior,
        restore_tabs_on_launch: bool,
    },
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

fn redacted_optional(value: &Option<String>) -> Option<&'static str> {
    value.as_ref().map(|_| "[REDACTED]")
}

impl std::fmt::Debug for SettingsAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaveSecurity {
                key_file_path,
                secrets_db_path,
                encrypted_crc_policy,
            } => f
                .debug_struct("SaveSecurity")
                .field("key_file_path", key_file_path)
                .field("secrets_db_path", secrets_db_path)
                .field("encrypted_crc_policy", encrypted_crc_policy)
                .finish(),
            Self::MoveVault { dest_path } => f
                .debug_struct("MoveVault")
                .field("dest_path", dest_path)
                .finish(),
            Self::RekeyVault { new_key_file_path } => f
                .debug_struct("RekeyVault")
                .field("new_key_file_path", new_key_file_path)
                .finish(),
            Self::SavePasswordRules { rules } => f
                .debug_struct("SavePasswordRules")
                .field("rules_count", &rules.len())
                .finish_non_exhaustive(),
            Self::SaveArchives { temp_dir } => f
                .debug_struct("SaveArchives")
                .field("temp_dir", temp_dir)
                .finish(),
            Self::InstallPlugin { wasm_path } => f
                .debug_struct("InstallPlugin")
                .field("wasm_path", wasm_path)
                .finish(),
            Self::ClearCacheIndex => f.write_str("ClearCacheIndex"),
            Self::ClearCacheContent => f.write_str("ClearCacheContent"),
            Self::GarbageCollectCache => f.write_str("GarbageCollectCache"),
            Self::CleanOldSearchCache => f.write_str("CleanOldSearchCache"),
            Self::MigrateCacheEntries => f.write_str("MigrateCacheEntries"),
            Self::SaveGeneral {
                open_nested_in_new_tab,
                drop_behavior,
                restore_tabs_on_launch,
            } => f
                .debug_struct("SaveGeneral")
                .field("open_nested_in_new_tab", open_nested_in_new_tab)
                .field("drop_behavior", drop_behavior)
                .field("restore_tabs_on_launch", restore_tabs_on_launch)
                .finish(),
            Self::SaveNetwork {
                socks5_enabled,
                socks5_address,
                socks5_username,
                socks5_password,
            } => f
                .debug_struct("SaveNetwork")
                .field("socks5_enabled", socks5_enabled)
                .field("socks5_address", &redacted_optional(socks5_address))
                .field("socks5_username", &redacted_optional(socks5_username))
                .field("socks5_password", &redacted_optional(socks5_password))
                .finish(),
            Self::TestNetwork {
                socks5_enabled,
                socks5_address,
                socks5_username,
                socks5_password,
            } => f
                .debug_struct("TestNetwork")
                .field("socks5_enabled", socks5_enabled)
                .field("socks5_address", &redacted_optional(socks5_address))
                .field("socks5_username", &redacted_optional(socks5_username))
                .field("socks5_password", &redacted_optional(socks5_password))
                .finish(),
            Self::SaveKeyboardMouse { bindings } => f
                .debug_struct("SaveKeyboardMouse")
                .field("bindings", bindings)
                .finish(),
            Self::SaveServer {
                enabled,
                url,
                api_key,
            } => f
                .debug_struct("SaveServer")
                .field("enabled", enabled)
                .field("url", url)
                .field("api_key", &redacted_optional(api_key))
                .finish(),
            Self::TestServer { url, api_key } => f
                .debug_struct("TestServer")
                .field("url", url)
                .field("api_key", &redacted_optional(api_key))
                .finish(),
            Self::NavigateTo(page) => f.debug_tuple("NavigateTo").field(page).finish(),
            Self::SaveEditedRule => f.write_str("SaveEditedRule"),
        }
    }
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
    /// What to do when a file is dropped without aiming at a specific overlay zone.
    pub drop_behavior: Signal<arclain_core::DropBehavior>,
    /// Whether to restore the previous tab session on launch.
    pub restore_tabs_on_launch: Signal<bool>,
}

impl Default for GeneralSettingsState {
    fn default() -> Self {
        Self {
            open_nested_in_new_tab: Signal::new(false),
            drop_behavior: Signal::new(arclain_core::DropBehavior::default()),
            restore_tabs_on_launch: Signal::new(true),
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
    /// Default DB paths captured once at state construction so the
    /// render path can show them as hint text without recomputing
    /// (which would touch `dirs::config_dir()` and env-var lookups
    /// every frame).
    pub default_paths: Option<arclain_core::DbPaths>,
}

impl Default for SecuritySettingsState {
    fn default() -> Self {
        Self {
            key_file_path: Signal::new(String::new()),
            secrets_db_path: Signal::new(String::new()),
            encrypted_crc_policy: Signal::new(EncryptedCrcPolicy::default()),
            info: Signal::new(String::new()),
            error: Signal::new(String::new()),
            default_paths: arclain_core::DbPaths::calculate_defaults("arclain").ok(),
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
    /// App-wide default for what the pipeline executor does when a predicted
    /// output path already exists (per-pipeline overrides still win).
    pub default_collision_policy: Signal<arclain_core::OutputCollisionPolicy>,
    /// Lazily loaded flag — `false` until the first render reads the stored
    /// value from `app_config`.
    pub collision_policy_loaded: Signal<bool>,
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
            default_collision_policy: Signal::new(arclain_core::OutputCollisionPolicy::Smart),
            collision_policy_loaded: Signal::new(false),
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

    #[test]
    fn settings_action_debug_redacts_proxy_credentials() {
        const ADDRESS_USER: &str = "proxy-address-user-b064";
        const ADDRESS_PASSWORD: &str = "proxy-address-password-e912";
        const USERNAME: &str = "proxy-debug-user-2e91";
        const PASSWORD: &str = "proxy-debug-password-7a43";

        for action in [
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some(format!(
                    "{ADDRESS_USER}:{ADDRESS_PASSWORD}@proxy.example:1080"
                )),
                socks5_username: Some(USERNAME.to_string()),
                socks5_password: Some(PASSWORD.to_string()),
            },
            SettingsAction::TestNetwork {
                socks5_enabled: true,
                socks5_address: Some("proxy.example:1080".to_string()),
                socks5_username: Some(USERNAME.to_string()),
                socks5_password: Some(PASSWORD.to_string()),
            },
        ] {
            let diagnostic = format!("{action:?}");
            for credential in [ADDRESS_USER, ADDRESS_PASSWORD, USERNAME, PASSWORD] {
                assert!(
                    !diagnostic.contains(credential),
                    "credential leaked: {diagnostic}"
                );
            }
            assert!(diagnostic.contains("[REDACTED]"));
        }
    }

    #[test]
    fn settings_action_debug_redacts_other_secret_bearing_variants() {
        const RULE_PASSWORD: &str = "rule-password-b129";
        const API_KEY: &str = "server-api-key-d605";
        let actions = [
            SettingsAction::SavePasswordRules {
                rules: vec![PasswordRule {
                    name: "rule".to_string(),
                    pattern: ".*".to_string(),
                    password: RULE_PASSWORD.to_string(),
                    priority: 1,
                    enabled: true,
                }],
            },
            SettingsAction::SaveServer {
                enabled: true,
                url: Some("https://server.example".to_string()),
                api_key: Some(API_KEY.to_string()),
            },
            SettingsAction::TestServer {
                url: "https://server.example".to_string(),
                api_key: Some(API_KEY.to_string()),
            },
        ];

        for action in actions {
            let diagnostic = format!("{action:?}");
            assert!(!diagnostic.contains(RULE_PASSWORD));
            assert!(!diagnostic.contains(API_KEY));
        }
    }

    // =========================================================================
    // EncryptedCrcPolicy
    // =========================================================================

    #[test]
    fn encrypted_crc_policy_default() {
        assert_eq!(EncryptedCrcPolicy::default(), EncryptedCrcPolicy::OnAccess);
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
