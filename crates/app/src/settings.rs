//! Settings, secrets, and vault DTOs, plus the pure (no I/O, no
//! `AppRuntime` access) conversion/validation logic they need.
//!
//! This module deliberately holds no `&AppRuntime` methods: everything
//! here is plain data and total functions over it, exactly like
//! `crate::operations::{convert, organize, pipeline}` hold their request
//! DTOs and `validate()` methods. The `AppRuntime`-touching execution
//! layer (reading/writing the live vault state, calling `arclain_core`
//! services, performing the actual redb/SQLite writes) lives in
//! `crate::runtime::settings_ops`, a submodule of `runtime` that can see
//! `AppRuntime`'s private fields; `crate::runtime`'s own `impl ArclainApp`
//! exposes the thin public `settings`/`update_settings`/... methods in
//! its delimited "Task 10" section.
//!
//! ## Single-authority vault state
//!
//! Earlier tasks' `SessionStore` handed its one copy of `ConfigDbs` to
//! `crates/ui` via `take_legacy_composition` and never retained a live
//! copy of its own -- fine as long as nothing behind the facade needed
//! to read or write settings/secrets/pass-rules after bootstrap, but
//! this task's whole job is exactly that. [`MutableSettings`] is the
//! fix: `SessionStore` now retains its own live, mutable copy of
//! everything this task's facade surface can change (`user_config`,
//! `pass_rules`, `encrypted_crc_policy`, `db_paths`, `dbs`), behind one
//! lock so a vault move/rekey -- which changes `dbs`, `db_paths`, and
//! `pass_rules` together -- can never be observed half-updated.
//! `take_legacy_composition` now *clones* out of this store instead of
//! taking it, so it can be called again to refresh `crates/ui`'s mirror
//! after a facade-driven mutation (see `ConfigDbs`'s own doc comment in
//! `arclain_db::bootstrap` for why cloning it is cheap and never opens a
//! second physical connection).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use arclain_core::{ConfigDbs, DbPaths, PassRule, UserConfig};

use crate::challenge::SecretInput;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};

// ============================================================================
// DTOs (see task-10-brief.md / contract.md -- field shapes are load-bearing,
// other Stage 1 tasks and any future frontend code against these exactly).
// ============================================================================

/// A full, point-in-time view of every non-secret application setting.
/// `revision` is the optimistic-concurrency token [`ArclainApp::
/// update_settings`](crate::runtime::ArclainApp::update_settings) expects
/// back unchanged in [`SettingsPatch::expected_revision`].
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SettingsSnapshot {
    pub revision: u64,
    pub archive: ArchiveSettingsDto,
    pub network: NetworkSettingsDto,
    pub security: SecuritySettingsDto,
}

/// One field's patch instruction. `Keep` and an omitted/absent field are
/// deliberately the same "leave unchanged" meaning -- `Keep` just makes
/// that explicit for a caller building a patch programmatically instead
/// of by literal omission.
///
/// **Which fields may `Clear`:** a value is only removable if the
/// underlying setting has a genuine "unset" state to fall back to.
/// - `Option<T>`-shaped settings (a directory override, a proxy address)
///   can `Clear`: it sets the field to `None`, matching "no override".
/// - Collection-shaped settings with a meaningful empty state (the
///   per-plugin proxy map) can `Clear`: it resets to an empty
///   collection.
/// - Plain scalar settings with no "unset" state of their own (a `bool`
///   toggle, `BackendModeDto`, the CRC policy string) reject `Clear` as
///   [`ApplicationErrorKind::InvalidInput`] -- there is nothing sensible
///   to clear *to*. Use `Set` to change one of these, or `Keep` to leave
///   it alone.
///
/// Each patch-applying function in this module documents which case it
/// is on a per-field basis; the rule above is applied uniformly rather
/// than decided ad hoc per field.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "operation", content = "value", rename_all = "snake_case")]
pub enum PatchValue<T> {
    Clear,
    Keep,
    Set(T),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendModeDto {
    Cli,
    Native,
}

impl BackendModeDto {
    fn from_user_config(value: &str) -> Self {
        match value {
            "cli" => Self::Cli,
            _ => Self::Native,
        }
    }

    fn as_user_config_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Native => "native",
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ArchiveSettingsDto {
    pub backend_mode: BackendModeDto,
    pub cache_directory: Option<PathBuf>,
    pub temp_directory: Option<PathBuf>,
    pub transfer_directory: Option<PathBuf>,
    pub sevenzip_path: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ArchiveSettingsPatch {
    pub backend_mode: PatchValue<BackendModeDto>,
    pub cache_directory: PatchValue<PathBuf>,
    pub temp_directory: PatchValue<PathBuf>,
    pub transfer_directory: PatchValue<PathBuf>,
    pub sevenzip_path: PatchValue<PathBuf>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct NetworkSettingsDto {
    pub socks5_enabled: bool,
    pub socks5_address: Option<String>,
    pub socks5_username: Option<String>,
    pub socks5_password_configured: bool,
    pub plugin_proxy_enabled: BTreeMap<String, bool>,
    pub gameta_server_enabled: bool,
    pub gameta_server_url: Option<String>,
    pub gameta_api_key_configured: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct NetworkSettingsPatch {
    pub socks5_enabled: PatchValue<bool>,
    pub socks5_address: PatchValue<String>,
    pub socks5_username: PatchValue<String>,
    pub plugin_proxy_enabled: PatchValue<BTreeMap<String, bool>>,
    pub gameta_server_enabled: PatchValue<bool>,
    pub gameta_server_url: PatchValue<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SecuritySettingsDto {
    pub secrets_database_path: Option<PathBuf>,
    pub key_file_path: Option<PathBuf>,
    pub encrypted_crc_policy: String,
    pub vault_available: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SecuritySettingsPatch {
    pub secrets_database_path: PatchValue<PathBuf>,
    pub key_file_path: PatchValue<PathBuf>,
    pub encrypted_crc_policy: PatchValue<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SettingsPatch {
    pub expected_revision: u64,
    pub archive: Option<ArchiveSettingsPatch>,
    pub network: Option<NetworkSettingsPatch>,
    pub security: Option<SecuritySettingsPatch>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct OrganizationProfileSummary {
    pub id: String,
    pub name: String,
    pub output_format: String,
}

/// A password rule's non-secret shape: never the stored password itself,
/// only whether one is configured. See [`PasswordRuleInput`] for the
/// write side.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct PasswordRuleSummary {
    pub name: String,
    pub pattern: String,
    pub priority: u32,
    pub enabled: bool,
    pub password_configured: bool,
}

/// A password rule create/update request.
///
/// `password: None` means "keep the currently-stored password
/// unchanged" when `name` matches an existing rule (the standard
/// convention for a secret-bearing edit form: leave the field blank to
/// not touch it) -- creating a *new* rule (no existing rule with that
/// `name`) with `password: None` is rejected as
/// [`ApplicationErrorKind::InvalidInput`], since a password-matching
/// rule with no password can never match anything and is almost
/// certainly a caller bug. To intentionally remove a rule, call
/// [`ArclainApp::delete_password_rule`](crate::runtime::ArclainApp::delete_password_rule)
/// instead of upserting an empty password.
///
/// Not `Clone`/`Serialize`/`Deserialize`: it carries a live
/// [`SecretInput`], and those restrictions are contagious on purpose --
/// see `SecretInput`'s own doc comment.
#[derive(Debug)]
pub struct PasswordRuleInput {
    pub name: String,
    pub pattern: String,
    pub priority: u32,
    pub enabled: bool,
    pub password: Option<SecretInput>,
}

/// One archive to reopen when a frontend restores a previous session.
/// Frontend-neutral: carries only what [`ArclainApp::
/// start_open_archive`](crate::runtime::ArclainApp::start_open_archive)
/// needs, never a frontend-specific tab/window identity or any
/// interface-only detail (theme, panel layout, tab order/pinning stay
/// entirely on the frontend's own side -- see this module's top-level
/// doc comment). Deliberately minimal: today's egui frontend's own
/// `tabs.json` already carries exactly this path per restored tab; this
/// type gives that one non-visual slice an application-owned shape a
/// non-egui frontend (a future Flutter client, a CLI `--restore` flag)
/// could reuse without inventing its own.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SessionArchiveEntry {
    pub source_path: PathBuf,
}

/// Serializes `entries` to `path` as pretty JSON, creating parent
/// directories as needed. Pure I/O, no `AppRuntime` access -- usable by
/// any frontend that wants an application-shaped session file instead of
/// inventing its own.
pub fn save_session_restore_list(
    path: &Path,
    entries: &[SessionArchiveEntry],
) -> Result<(), ApplicationError> {
    let json = serde_json::to_string_pretty(entries).map_err(|error| {
        ApplicationError::new(
            ApplicationErrorKind::Internal,
            "failed to serialize session",
        )
        .with_diagnostic(error.to_string())
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| persistence_io_error(path, error))?;
    }
    std::fs::write(path, json).map_err(|error| persistence_io_error(path, error))
}

/// Reads back what [`save_session_restore_list`] wrote. A missing file is
/// reported as an empty list (nothing to restore), matching how a
/// missing `tabs.json` behaves today -- not every other read failure:
/// existing-but-corrupt content is still a [`ApplicationErrorKind::
/// Persistence`] error, so callers can distinguish "first launch" from
/// "something is wrong with a file that should be readable".
pub fn load_session_restore_list(
    path: &Path,
) -> Result<Vec<SessionArchiveEntry>, ApplicationError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(persistence_io_error(path, error)),
    };
    serde_json::from_str(&content).map_err(|error| {
        ApplicationError::new(
            ApplicationErrorKind::Persistence,
            "failed to parse session file",
        )
        .with_diagnostic(error.to_string())
        .with_recoverability(Recoverability::UserAction)
    })
}

fn persistence_io_error(path: &Path, error: std::io::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence, "session file I/O failed")
        .with_diagnostic(format!("{}: {error}", path.display()))
        .with_recoverability(Recoverability::Retry)
}

// ============================================================================
// MutableSettings: the single-authority live vault/settings state.
// ============================================================================

/// Everything this task's facade surface can change at runtime, behind
/// one lock in `SessionStore` (`parking_lot::RwLock<MutableSettings>`).
/// See this module's top-level doc comment for why this replaced the
/// previous one-shot-taken `dbs` field and the bootstrap-frozen
/// `user_config`/`pass_rules` fields `SessionStore` held before this
/// task.
pub(crate) struct MutableSettings {
    /// Bumped by every mutation that changes anything reachable through
    /// [`SettingsSnapshot`] -- `update_settings` itself, and also
    /// `set_gameta_api_key`/`set_socks5_password`/`move_vault`/
    /// `rekey_vault`, each of which changes a `configured`/vault-path
    /// flag the snapshot reports. Password-rule CRUD does *not* bump
    /// this: password rules have their own `password_rules()` read
    /// method entirely separate from `settings()`, so a rule change
    /// could never make a caller's cached `SettingsSnapshot` stale.
    pub(crate) revision: u64,
    pub(crate) user_config: UserConfig,
    pub(crate) pass_rules: Vec<PassRule>,
    pub(crate) encrypted_crc_policy: String,
    pub(crate) db_paths: Option<DbPaths>,
    pub(crate) dbs: Option<ConfigDbs>,
}

impl MutableSettings {
    pub(crate) fn new(
        user_config: UserConfig,
        pass_rules: Vec<PassRule>,
        encrypted_crc_policy: String,
        db_paths: Option<DbPaths>,
        dbs: Option<ConfigDbs>,
    ) -> Self {
        Self {
            revision: 0,
            user_config,
            pass_rules,
            encrypted_crc_policy,
            db_paths,
            dbs,
        }
    }
}

// ============================================================================
// Pure DTO <-> domain conversions.
// ============================================================================

pub(crate) fn archive_dto(user_config: &UserConfig) -> ArchiveSettingsDto {
    ArchiveSettingsDto {
        backend_mode: BackendModeDto::from_user_config(&user_config.backend_mode),
        cache_directory: user_config.cache_directory.clone().map(PathBuf::from),
        temp_directory: user_config.temp_dir.clone().map(PathBuf::from),
        transfer_directory: user_config.transfer_dir.clone().map(PathBuf::from),
        sevenzip_path: user_config.sevenzip_path.clone().map(PathBuf::from),
    }
}

pub(crate) fn network_dto(
    user_config: &UserConfig,
    socks5_password_configured: bool,
    gameta_api_key_configured: bool,
) -> NetworkSettingsDto {
    NetworkSettingsDto {
        socks5_enabled: user_config.socks5_enabled,
        socks5_address: user_config.socks5_address.clone(),
        socks5_username: user_config.socks5_username.clone(),
        socks5_password_configured,
        plugin_proxy_enabled: user_config
            .get_plugin_proxy_settings()
            .into_iter()
            .collect(),
        gameta_server_enabled: user_config.gameta_server_enabled,
        gameta_server_url: user_config.gameta_server_url.clone(),
        gameta_api_key_configured,
    }
}

pub(crate) fn security_dto(mutable: &MutableSettings) -> SecuritySettingsDto {
    SecuritySettingsDto {
        secrets_database_path: mutable.db_paths.as_ref().map(|p| p.secrets_db.clone()),
        key_file_path: mutable.db_paths.as_ref().and_then(|p| p.key_file.clone()),
        encrypted_crc_policy: mutable.encrypted_crc_policy.clone(),
        vault_available: mutable.dbs.is_some(),
    }
}

pub(crate) fn summarize_pass_rule(rule: &PassRule) -> PasswordRuleSummary {
    PasswordRuleSummary {
        name: rule.name.clone(),
        pattern: rule.pattern.clone(),
        priority: rule.priority,
        enabled: rule.enabled,
        password_configured: !rule.password.is_empty(),
    }
}

pub(crate) fn summarize_profile(
    profile: &arclain_core::features::organization::ArchiveProfile,
) -> OrganizationProfileSummary {
    OrganizationProfileSummary {
        id: profile.id.to_string(),
        name: profile.name.clone(),
        output_format: profile.format.as_str().to_string(),
    }
}

// ============================================================================
// PatchValue application helpers -- see PatchValue's own doc comment for
// the Clear/Keep/Set rule these enforce uniformly.
// ============================================================================

/// Applies a patch to an `Option<T>`-shaped setting. `Clear` is always
/// valid here: it sets the field to `None`.
fn apply_optional<T>(current: &mut Option<T>, patch: PatchValue<T>) {
    match patch {
        PatchValue::Keep => {}
        PatchValue::Clear => *current = None,
        PatchValue::Set(value) => *current = Some(value),
    }
}

/// Applies a patch to a plain scalar setting with no "unset" state of its
/// own. `Clear` is rejected as `InvalidInput` -- see `PatchValue`'s doc
/// comment for why.
fn apply_required<T>(
    current: &mut T,
    patch: PatchValue<T>,
    field: &'static str,
) -> Result<(), ApplicationError> {
    match patch {
        PatchValue::Keep => {}
        PatchValue::Set(value) => *current = value,
        PatchValue::Clear => return Err(clear_not_supported_error(field)),
    }
    Ok(())
}

/// Applies a patch to a collection-shaped setting with a meaningful empty
/// state. `Clear` resets to that empty state.
fn apply_collection<T: Default>(current: &mut T, patch: PatchValue<T>) {
    match patch {
        PatchValue::Keep => {}
        PatchValue::Clear => *current = T::default(),
        PatchValue::Set(value) => *current = value,
    }
}

fn clear_not_supported_error(field: &'static str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "this setting has no empty state to clear",
    )
    .with_diagnostic(format!(
        "field {field:?} is a plain value with no \"unset\" state; use Set to change it or Keep \
         to leave it unchanged"
    ))
    .with_recoverability(Recoverability::UserAction)
    .with_field(field)
}

/// Applies [`ArchiveSettingsPatch`] to a working copy of `user_config`.
/// Pure: no I/O, no partial application on error (returns before mutating
/// further fields once one is rejected, but even the fields already
/// applied are on the *caller's* working copy, never the live
/// `MutableSettings` -- `settings_ops::run_update_settings` only commits
/// the working copy after every patch in the whole request validates and
/// every disk write succeeds).
pub(crate) fn apply_archive_patch(
    user_config: &mut UserConfig,
    patch: ArchiveSettingsPatch,
) -> Result<(), ApplicationError> {
    let mut backend_mode = BackendModeDto::from_user_config(&user_config.backend_mode);
    apply_required(
        &mut backend_mode,
        patch.backend_mode,
        "archive.backend_mode",
    )?;
    user_config.backend_mode = backend_mode.as_user_config_str().to_string();

    let mut cache_directory = user_config.cache_directory.clone().map(PathBuf::from);
    apply_optional(&mut cache_directory, patch.cache_directory);
    user_config.cache_directory = cache_directory.map(path_to_string);

    let mut temp_directory = user_config.temp_dir.clone().map(PathBuf::from);
    apply_optional(&mut temp_directory, patch.temp_directory);
    user_config.temp_dir = temp_directory.map(path_to_string);

    let mut transfer_directory = user_config.transfer_dir.clone().map(PathBuf::from);
    apply_optional(&mut transfer_directory, patch.transfer_directory);
    user_config.transfer_dir = transfer_directory.map(path_to_string);

    let mut sevenzip_path = user_config.sevenzip_path.clone().map(PathBuf::from);
    apply_optional(&mut sevenzip_path, patch.sevenzip_path);
    user_config.sevenzip_path = sevenzip_path.map(path_to_string);

    Ok(())
}

/// Applies the non-secret fields of [`NetworkSettingsPatch`] to a working
/// copy of `user_config`. Does not touch the SOCKS5 password or gameta
/// API key -- those are [`ArclainApp::set_socks5_password`]/
/// [`ArclainApp::set_gameta_api_key`]'s job specifically so a plain
/// address/toggle change never needs to carry a secret through this path
/// at all (see both methods' own doc comments in `crate::runtime`).
pub(crate) fn apply_network_patch(
    user_config: &mut UserConfig,
    patch: NetworkSettingsPatch,
) -> Result<(), ApplicationError> {
    apply_required(
        &mut user_config.socks5_enabled,
        patch.socks5_enabled,
        "network.socks5_enabled",
    )?;
    apply_optional(&mut user_config.socks5_address, patch.socks5_address);
    apply_optional(&mut user_config.socks5_username, patch.socks5_username);

    let mut plugin_proxy_enabled: BTreeMap<String, bool> = user_config
        .get_plugin_proxy_settings()
        .into_iter()
        .collect();
    apply_collection(&mut plugin_proxy_enabled, patch.plugin_proxy_enabled);
    user_config.set_plugin_proxy_settings(&plugin_proxy_enabled.into_iter().collect());

    apply_required(
        &mut user_config.gameta_server_enabled,
        patch.gameta_server_enabled,
        "network.gameta_server_enabled",
    )?;
    apply_optional(&mut user_config.gameta_server_url, patch.gameta_server_url);

    Ok(())
}

/// The subset of [`SecuritySettingsPatch`] that is a plain, in-memory
/// value change (`encrypted_crc_policy`). `secrets_database_path`/
/// `key_file_path` are handled separately by
/// `settings_ops::repoint_vault_paths` -- unlike a directory override,
/// changing either means re-opening the encrypted vault at a new
/// location, which needs I/O and can fail in ways a pure function can't
/// perform.
pub(crate) fn apply_security_value_patch(
    encrypted_crc_policy: &mut String,
    patch: &SecuritySettingsPatch,
) -> Result<(), ApplicationError> {
    apply_required(
        encrypted_crc_policy,
        patch.encrypted_crc_policy.clone(),
        "security.encrypted_crc_policy",
    )
}

/// Whether `patch` touches either vault-path field (anything other than
/// `Keep` on both). `settings_ops::run_update_settings` uses this to
/// decide whether the (I/O-requiring) vault repoint step runs at all.
pub(crate) fn security_patch_touches_vault_paths(patch: &SecuritySettingsPatch) -> bool {
    !matches!(patch.secrets_database_path, PatchValue::Keep)
        || !matches!(patch.key_file_path, PatchValue::Keep)
}

/// Whether `patch` touches any of the three SOCKS5 identity fields
/// (address/username/enabled). `settings_ops::run_update_settings` uses
/// this to decide whether the proxy change needs to go through
/// `NetworkProxyPersistenceService` (which also re-stages the existing
/// password so it survives an address/username-only change) rather than
/// a plain `ConfigService::save_user_config`.
pub(crate) fn network_patch_touches_socks5_identity(patch: &NetworkSettingsPatch) -> bool {
    !matches!(patch.socks5_enabled, PatchValue::Keep)
        || !matches!(patch.socks5_address, PatchValue::Keep)
        || !matches!(patch.socks5_username, PatchValue::Keep)
}

fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

// ============================================================================
// Password-rule input validation.
// ============================================================================

/// Rejects a structurally invalid [`PasswordRuleInput`] before any I/O:
/// an empty `name` or `pattern`, or a pattern that isn't a valid regex
/// (the same engine `arclain_core::utilities::PassRule::to_regex` and the
/// archive-open auto-password matcher use, so an accepted pattern is
/// guaranteed to actually compile when matching runs later).
pub(crate) fn validate_password_rule_input(
    rule: &PasswordRuleInput,
) -> Result<(), ApplicationError> {
    if rule.name.trim().is_empty() {
        return Err(invalid_input_error("name", "rule name must not be empty"));
    }
    if rule.pattern.trim().is_empty() {
        return Err(invalid_input_error(
            "pattern",
            "rule pattern must not be empty",
        ));
    }
    if regex::Regex::new(&rule.pattern).is_err() {
        return Err(invalid_input_error(
            "pattern",
            "rule pattern is not a valid regular expression",
        ));
    }
    Ok(())
}

fn invalid_input_error(field: &'static str, summary: &'static str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, summary)
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_user_config() -> UserConfig {
        UserConfig::new()
    }

    #[test]
    fn backend_mode_round_trips_through_user_config_strings() {
        assert_eq!(
            BackendModeDto::from_user_config("native"),
            BackendModeDto::Native
        );
        assert_eq!(BackendModeDto::from_user_config("cli"), BackendModeDto::Cli);
        // Unknown/legacy values fall back to Native rather than panicking.
        assert_eq!(
            BackendModeDto::from_user_config("bogus"),
            BackendModeDto::Native
        );
        assert_eq!(BackendModeDto::Native.as_user_config_str(), "native");
        assert_eq!(BackendModeDto::Cli.as_user_config_str(), "cli");
    }

    #[test]
    fn archive_dto_reflects_first_run_defaults() {
        let dto = archive_dto(&default_user_config());
        assert_eq!(dto.backend_mode, BackendModeDto::Native);
        assert!(dto.cache_directory.is_none());
        assert!(dto.temp_directory.is_none());
        assert!(dto.transfer_directory.is_none());
        assert!(dto.sevenzip_path.is_none());
    }

    #[test]
    fn clear_is_rejected_for_scalar_fields_without_an_empty_state() {
        let mut user_config = default_user_config();
        let patch = ArchiveSettingsPatch {
            backend_mode: PatchValue::Clear,
            cache_directory: PatchValue::Keep,
            temp_directory: PatchValue::Keep,
            transfer_directory: PatchValue::Keep,
            sevenzip_path: PatchValue::Keep,
        };

        let error = apply_archive_patch(&mut user_config, patch).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("archive.backend_mode"));
    }

    #[test]
    fn clear_resets_an_optional_directory_override_to_none() {
        let mut user_config = default_user_config();
        user_config.cache_directory = Some("/old/cache".to_string());
        let patch = ArchiveSettingsPatch {
            backend_mode: PatchValue::Keep,
            cache_directory: PatchValue::Clear,
            temp_directory: PatchValue::Keep,
            transfer_directory: PatchValue::Keep,
            sevenzip_path: PatchValue::Keep,
        };

        apply_archive_patch(&mut user_config, patch).unwrap();

        assert!(user_config.cache_directory.is_none());
    }

    #[test]
    fn set_overrides_a_directory_and_keep_leaves_others_untouched() {
        let mut user_config = default_user_config();
        user_config.temp_dir = Some("/old/temp".to_string());
        let patch = ArchiveSettingsPatch {
            backend_mode: PatchValue::Keep,
            cache_directory: PatchValue::Set(PathBuf::from("/new/cache")),
            temp_directory: PatchValue::Keep,
            transfer_directory: PatchValue::Keep,
            sevenzip_path: PatchValue::Keep,
        };

        apply_archive_patch(&mut user_config, patch).unwrap();

        assert_eq!(user_config.cache_directory.as_deref(), Some("/new/cache"));
        assert_eq!(user_config.temp_dir.as_deref(), Some("/old/temp"));
    }

    #[test]
    fn clear_on_a_bool_toggle_is_rejected() {
        let mut user_config = default_user_config();
        let patch = NetworkSettingsPatch {
            socks5_enabled: PatchValue::Clear,
            socks5_address: PatchValue::Keep,
            socks5_username: PatchValue::Keep,
            plugin_proxy_enabled: PatchValue::Keep,
            gameta_server_enabled: PatchValue::Keep,
            gameta_server_url: PatchValue::Keep,
        };

        let error = apply_network_patch(&mut user_config, patch).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("network.socks5_enabled"));
    }

    #[test]
    fn clear_on_the_plugin_proxy_map_resets_it_to_empty() {
        let mut user_config = default_user_config();
        user_config.set_plugin_proxy_enabled("dlsite", false);
        let patch = NetworkSettingsPatch {
            socks5_enabled: PatchValue::Keep,
            socks5_address: PatchValue::Keep,
            socks5_username: PatchValue::Keep,
            plugin_proxy_enabled: PatchValue::Clear,
            gameta_server_enabled: PatchValue::Keep,
            gameta_server_url: PatchValue::Keep,
        };

        apply_network_patch(&mut user_config, patch).unwrap();

        assert!(user_config.get_plugin_proxy_settings().is_empty());
    }

    #[test]
    fn security_value_patch_rejects_clear_on_crc_policy() {
        let mut policy = "on_access".to_string();
        let patch = SecuritySettingsPatch {
            secrets_database_path: PatchValue::Keep,
            key_file_path: PatchValue::Keep,
            encrypted_crc_policy: PatchValue::Clear,
        };

        let error = apply_security_value_patch(&mut policy, &patch).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(
            error.field.as_deref(),
            Some("security.encrypted_crc_policy")
        );
    }

    #[test]
    fn security_patch_touches_vault_paths_detects_either_field() {
        let keep_both = SecuritySettingsPatch {
            secrets_database_path: PatchValue::Keep,
            key_file_path: PatchValue::Keep,
            encrypted_crc_policy: PatchValue::Keep,
        };
        assert!(!security_patch_touches_vault_paths(&keep_both));

        let clears_one = SecuritySettingsPatch {
            secrets_database_path: PatchValue::Clear,
            ..keep_both
        };
        assert!(security_patch_touches_vault_paths(&clears_one));
    }

    #[test]
    fn password_rule_input_rejects_empty_name_pattern_and_bad_regex() {
        let base = |name: &str, pattern: &str| PasswordRuleInput {
            name: name.to_string(),
            pattern: pattern.to_string(),
            priority: 10,
            enabled: true,
            password: Some(SecretInput::new("secret".to_string())),
        };

        assert_eq!(
            validate_password_rule_input(&base("", "pattern"))
                .unwrap_err()
                .field
                .as_deref(),
            Some("name")
        );
        assert_eq!(
            validate_password_rule_input(&base("name", ""))
                .unwrap_err()
                .field
                .as_deref(),
            Some("pattern")
        );
        assert_eq!(
            validate_password_rule_input(&base("name", "("))
                .unwrap_err()
                .field
                .as_deref(),
            Some("pattern")
        );
        assert!(validate_password_rule_input(&base("name", "valid.*pattern")).is_ok());
    }

    #[test]
    fn summarize_pass_rule_never_carries_the_raw_password() {
        let rule = PassRule {
            name: "n".to_string(),
            pattern: "p".to_string(),
            password: "super-secret-value".to_string(),
            priority: 1,
            enabled: true,
        };

        let summary = summarize_pass_rule(&rule);
        let serialized = serde_json::to_string(&summary).unwrap();

        assert!(summary.password_configured);
        assert!(!serialized.contains("super-secret-value"));
        assert!(!format!("{summary:?}").contains("super-secret-value"));
    }

    #[test]
    fn session_restore_list_round_trips_and_missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("session.json");

        assert_eq!(load_session_restore_list(&path).unwrap(), Vec::new());

        let entries = vec![
            SessionArchiveEntry {
                source_path: PathBuf::from("/a.zip"),
            },
            SessionArchiveEntry {
                source_path: PathBuf::from("/b.7z"),
            },
        ];
        save_session_restore_list(&path, &entries).unwrap();

        assert_eq!(load_session_restore_list(&path).unwrap(), entries);
    }

    #[test]
    fn session_restore_list_corrupt_content_is_a_persistence_error_not_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.json");
        std::fs::write(&path, b"not json").unwrap();

        let error = load_session_restore_list(&path).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::Persistence);
    }
}
