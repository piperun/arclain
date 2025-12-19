//! Shared types for settings pages
//!
//! Contains enums, state structs, and action types used across settings pages.

use crate::features::password_management::dialogs::zip_pass_rules::PasswordRule;

/// Encrypted CRC policy for encrypted archives
#[derive(Copy, Clone, PartialEq, Eq)]
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
    /// Save general settings
    SaveGeneral { open_nested_in_new_tab: bool },
    /// Navigate to another settings page
    NavigateTo(crate::core::navigation::SettingsPage),
}

/// State for the general settings page
#[derive(Default)]
pub struct GeneralSettingsState {
    /// Whether to open nested archives in a new tab (true) or replace current view (false)
    pub open_nested_in_new_tab: bool,
}

/// State for the security settings page
#[derive(Default)]
pub struct SecuritySettingsState {
    pub key_file_path: String,
    pub secrets_db_path: String,
    pub encrypted_crc_policy: EncryptedCrcPolicy,
    pub info: String,
    pub error: String,
}

/// State for the archives settings page
#[derive(Default)]
pub struct ArchivesSettingsState {
    pub temp_dir: String,
    // Checksum settings
    pub checksum_enabled: bool,
    pub checksum_mode: ChecksumMode,
    pub checksum_algorithm: ChecksumAlgorithm,
    pub verify_after_extract: bool,
    pub verify_after_organize: bool,
}

/// Checksum verification mode (mirrors VerifyMode)
#[derive(Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Clone, Copy, PartialEq, Eq, Default)]
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
