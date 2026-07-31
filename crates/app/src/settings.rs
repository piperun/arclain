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
//! `pass_rules`, `encrypted_crc_policy`, `default_collision_policy`,
//! `db_paths`, `dbs`), behind one
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
    pub general: GeneralSettingsDto,
}

/// One explicit maintenance operation for the application's persistent
/// cache. Kept as a closed enum so frontends cannot supply arbitrary
/// paths, SQL fragments, or retention windows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMaintenanceTask {
    ClearIndex,
    ClearContent,
    GarbageCollect,
    CleanOldSearch,
    RepairEntries,
}

/// Typed outcome of [`crate::ArclainApp::maintain_cache`]. Counts are
/// reported where the underlying maintenance operation can provide one;
/// clear operations are acknowledgements because their stores do not
/// expose a reliable pre-clear row/blob count.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CacheMaintenanceReport {
    IndexCleared,
    ContentCleared,
    OrphansRemoved {
        entries: usize,
    },
    OldSearchEntriesRemoved {
        entries: usize,
    },
    EntriesRepaired {
        cache_types: usize,
        product_ids: usize,
    },
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
/// **One documented exception:** [`SecuritySettingsPatch::
/// secrets_database_path`] and [`SecuritySettingsPatch::key_file_path`]
/// are `Option<PathBuf>`-shaped, so by the first bullet above `Clear`
/// is accepted -- but it does not mean `None`. A vault must always
/// resolve to some concrete on-disk location, so `Clear` on either of
/// these two means "reset to this install's computed default location"
/// instead. See [`SecuritySettingsPatch`]'s own doc comment and
/// [`apply_vault_path_patch`] for the full rationale.
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
    /// What a batch pipeline does when a producing step is about to
    /// write over an existing path, unless the pipeline itself overrides
    /// it. Stored as the same string token `arclain_core::
    /// OutputCollisionPolicy::to_settings_str`/`from_settings_str` use
    /// ("fail" | "skip" | "overwrite" | "smart").
    ///
    /// Unlike every other field here this one does *not* live on the
    /// `user_config` row -- it is an `app_config` key/value entry
    /// (`COLLISION_POLICY_CONFIG_KEY`), the same storage
    /// [`SecuritySettingsDto::encrypted_crc_policy`] uses. It is grouped
    /// with the archive settings anyway because that is the page a user
    /// changes it on and the concern it belongs to; where the bytes land
    /// is a persistence detail, not a grouping criterion.
    pub default_collision_policy: String,
}

/// See [`GeneralSettingsDto::default`] -- same rationale, same
/// derived-not-restated construction.
impl Default for ArchiveSettingsDto {
    fn default() -> Self {
        archive_dto(&UserConfig::default(), &default_collision_policy_token())
    }
}

/// See [`ArchiveSettingsDto`]. Every field's `Clear` is rejected as
/// `InvalidInput` except the four directory/path overrides, matching
/// [`PatchValue`]'s general rule.
///
/// `default_collision_policy` is additionally *validated* on `Set`: an
/// unrecognized token is rejected rather than stored, because
/// `arclain_core::OutputCollisionPolicy::from_settings_str` silently maps
/// anything it does not recognize to "no app default at all", which would
/// turn a typo into a quietly different pipeline behavior instead of an
/// error the caller can see. `Keep` never validates, so an already-stored
/// unrecognized value round-trips untouched.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ArchiveSettingsPatch {
    pub backend_mode: PatchValue<BackendModeDto>,
    pub cache_directory: PatchValue<PathBuf>,
    pub temp_directory: PatchValue<PathBuf>,
    pub transfer_directory: PatchValue<PathBuf>,
    pub sevenzip_path: PatchValue<PathBuf>,
    pub default_collision_policy: PatchValue<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct NetworkSettingsDto {
    pub socks5_enabled: bool,
    pub socks5_address: Option<String>,
    pub socks5_username: Option<String>,
    pub socks5_password_configured: bool,
    /// The *stored* per-plugin proxy overrides: sparse, containing only
    /// plugins a user has explicitly opted in or out. Not the routing
    /// map -- see [`Self::effective_plugin_proxy_enabled`] for that, and
    /// for why the difference matters to anything rendering a toggle.
    pub plugin_proxy_enabled: BTreeMap<String, bool>,
    pub gameta_server_enabled: bool,
    pub gameta_server_url: Option<String>,
    pub gameta_api_key_configured: bool,
}

impl NetworkSettingsDto {
    /// The routing map actually in effect: which plugins' traffic goes
    /// through the SOCKS5 proxy right now.
    ///
    /// This is *not* [`Self::plugin_proxy_enabled`]. That field is the
    /// sparse stored override set; the effective map additionally
    /// applies the "on by default" plugins and the global kill switch,
    /// so a plugin with no stored entry can still be routed and every
    /// entry disappears while the proxy is disabled. A frontend showing
    /// a per-plugin toggle must read this one -- reading the raw
    /// overrides would render a default-on plugin as off until the user
    /// touched it.
    ///
    /// Resolved by `arclain_core::utilities::apply_default_proxied_plugins`
    /// -- the same function the live HTTP client's routing goes through
    /// -- so a rendered toggle can never disagree with what the network
    /// stack actually does.
    pub fn effective_plugin_proxy_enabled(&self) -> BTreeMap<String, bool> {
        arclain_core::utilities::apply_default_proxied_plugins(
            self.socks5_enabled,
            self.plugin_proxy_enabled
                .iter()
                .map(|(id, enabled)| (id.clone(), *enabled))
                .collect(),
        )
        .into_iter()
        .collect()
    }

    /// Whether `plugin_id`'s traffic is routed through the proxy right
    /// now -- [`Self::effective_plugin_proxy_enabled`] for a single
    /// plugin, without building the map. Frontends call this once per
    /// rendered toggle, so it allocates nothing.
    ///
    /// Same three rules, in the same order as
    /// `apply_default_proxied_plugins`: the global switch wins, then a
    /// stored override, then the default-on list. Pinned against the map
    /// itself by `the_two_effective_proxy_reads_always_agree`.
    pub fn plugin_proxy_effective(&self, plugin_id: &str) -> bool {
        self.socks5_enabled
            && match self.plugin_proxy_enabled.get(plugin_id) {
                Some(stored) => *stored,
                None => arclain_core::utilities::DEFAULT_PROXIED_PLUGINS.contains(&plugin_id),
            }
    }
}

/// What a gameta server reported about itself when
/// [`crate::ArclainApp::test_gameta_connection`] probed it: the health
/// body's fields, verbatim and uninterpreted.
///
/// Both fields are the server's own words. In particular `status` is
/// **not** inspected by the probe -- a server answering
/// `{"status":"degraded", ...}` still returns `Ok`, exactly as it did
/// before this surface existed. The value is carried here so a frontend
/// that wants to react to it can, without the probe silently inventing a
/// failure mode the settings page never had.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GametaServerInfo {
    /// The server's self-reported status string (typically `"ok"`).
    pub status: String,
    /// The server's version string, as the settings page displays it.
    pub version: String,
}

/// The running application's startup connection state for the configured
/// gameta server.
///
/// This is deliberately distinct from [`GametaServerInfo`]: that type is
/// the result of explicitly probing values currently typed into a settings
/// form, while this one reports the already-composed client's cached startup
/// health. Reading it performs no network request.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GametaConnectionStatusDto {
    /// Gameta integration is disabled in the persisted configuration.
    Disabled,
    /// Startup composed a usable client. The version is absent only when
    /// the client had no cached version to report.
    Connected { version: Option<String> },
    /// Integration is enabled, but startup could not compose a usable
    /// client (for example, its health check failed).
    Unavailable,
}

/// Converts the independently-composed configuration/client facts into the
/// one startup status a frontend renders. Configuration wins deliberately:
/// a disabled integration is `Disabled` even if a stale client handle were
/// ever supplied by a future composition path.
pub(crate) fn gameta_connection_status(
    enabled: bool,
    client_available: bool,
    version: Option<String>,
) -> GametaConnectionStatusDto {
    if !enabled {
        GametaConnectionStatusDto::Disabled
    } else if client_available {
        GametaConnectionStatusDto::Connected { version }
    } else {
        GametaConnectionStatusDto::Unavailable
    }
}

/// The candidate SOCKS5 proxy [`crate::ArclainApp::probe_network`] routes
/// its probe through. Field names mirror the proxy configuration the
/// settings form holds, with the stored `host:port` authority already
/// split into its two parts.
///
/// Not `Clone`, `Serialize`, or `Deserialize`: [`SecretInput`] is none of
/// those on purpose, and that restriction is contagious here by design --
/// a candidate password must be consumed by the probe it was built for,
/// never queued, copied, or logged.
#[derive(Debug)]
pub struct Socks5Candidate {
    /// The proxy host exactly as it must appear in an authority: an IPv6
    /// literal keeps its brackets (`[::1]`).
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<SecretInput>,
}

/// One step of a network probe, as the settings page's result panel
/// renders it: a name, whether it passed, and an optional detail line.
/// Mirrors `arclain_network`'s own `ConnectionTestStep`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ProbeStepDto {
    /// `DNS`, `TCP`, `SOCKS5`, or `HTTP` -- which steps appear depends on
    /// whether the probe went through a proxy.
    pub name: String,
    pub passed: bool,
    /// The resolved address, an error string, or nothing.
    pub message: Option<String>,
}

/// What [`crate::ArclainApp::probe_network`] observed: the full per-step
/// trace, plus the egress address the probe came out of.
///
/// There is no `success` flag: a probe stops at its first failed step, so
/// [`Self::succeeded`] reads it off the trace itself, and every failure is
/// a failed step a frontend can point at rather than a boolean it has to
/// explain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct NetworkProbeReport {
    pub steps: Vec<ProbeStepDto>,
    /// The public IP the probe's request appeared to come from. `None`
    /// unless every step passed.
    pub ip: Option<String>,
    /// The country that IP resolved to. `None` unless every step passed.
    pub country: Option<String>,
}

impl NetworkProbeReport {
    /// Whether the probe reached the far end. Every step passing is the
    /// same condition the underlying probe uses to set its own success
    /// flag -- `runtime::settings_ops::probe_report`, which builds this
    /// report, debug-asserts the two agree, so a change on either side
    /// surfaces in tests rather than as a silently misreported verdict.
    pub fn succeeded(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|step| step.passed)
    }
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
    /// Where a `Clear` on [`SecuritySettingsPatch::secrets_database_path`]
    /// would put the vault -- this install's computed default location.
    /// Reported so a frontend can show it (as a placeholder for an empty
    /// override field, say) without resolving OS config directories of
    /// its own. Resolved once at bootstrap, so it never changes for the
    /// life of an application instance. `None` if it could not be
    /// resolved at all, which is the same condition that makes `Clear`
    /// fail.
    pub default_secrets_database_path: Option<PathBuf>,
    /// See [`Self::default_secrets_database_path`], for the key file.
    pub default_key_file_path: Option<PathBuf>,
    pub encrypted_crc_policy: String,
    pub vault_available: bool,
}

/// `secrets_database_path`/`key_file_path`'s `Clear` is this patch
/// surface's one deliberate exception to [`PatchValue`]'s general
/// "`Option<T>` `Clear` means `None`" rule: a vault must always resolve
/// to *some* concrete location on disk, so there is no "unset" state to
/// clear to, the way there is for e.g. [`ArchiveSettingsPatch::
/// cache_directory`]. `Clear` on either of these two fields instead
/// means "reset to this install's computed default location"
/// (`DbPaths::calculate_defaults`) -- applied by
/// [`apply_vault_path_patch`], called from
/// `settings_ops::repoint_vault_paths` (not by [`apply_security_value_patch`],
/// which only covers `encrypted_crc_policy`), since it needs a `DbPaths`
/// to patch and a computed `defaults` to fall back to, neither of which
/// a pure per-field function has on its own. Pinned by this module's own
/// `clear_on_vault_paths_resets_to_the_computed_defaults_not_to_unset`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SecuritySettingsPatch {
    pub secrets_database_path: PatchValue<PathBuf>,
    pub key_file_path: PatchValue<PathBuf>,
    pub encrypted_crc_policy: PatchValue<String>,
}

/// The general/interface preferences that used to be five of `crates/ui`'s
/// direct `ConfigService::save_user_config` writers (see `GeneralSettingsPatch`'s
/// own doc comment): hotkey bindings, and the drop/nested-archive/session-
/// restore preferences the "General" settings page shows. Deliberately
/// distinct from [`ArchiveSettingsDto`] (archive backend/directory
/// overrides) and [`NetworkSettingsDto`] (proxy/server) -- these fields
/// share no validation or persistence concern with either, only the same
/// `user_config` row.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct GeneralSettingsDto {
    /// Hotkey bindings, pre-serialized to the same JSON string
    /// `arclain_core::UserConfig::hotkey_bindings` stores -- this facade
    /// does not parse or validate individual bindings (the egui
    /// keyboard/mouse settings page owns that shape), only persists
    /// whatever blob it is handed.
    pub hotkey_bindings: Option<String>,
    pub open_nested_in_new_tab: bool,
    /// One of `"new_tab"`, `"replace"`, or `"ask_each_time"`. Carried as
    /// a plain token rather than an enum: which of the three a given
    /// frontend can actually offer is its own business (a headless
    /// client has no tabs to drop onto at all). Unvalidated on the way
    /// through -- an unrecognized token round-trips as-is, and every
    /// reader is expected to treat one it does not know as `"new_tab"`,
    /// the behavior with no prerequisites.
    pub drop_behavior: String,
    pub restore_tabs_on_launch: bool,
}

/// The placeholder a frontend holds in a reactive cell before it has
/// read the real settings -- so it need not invent its own idea of what
/// "not loaded yet" looks like, and cannot drift from what this facade
/// would report for the same absent state.
///
/// Built from `UserConfig::default()`, deliberately **not**
/// `UserConfig::new()`: `default()` is what `bootstrap` itself falls
/// back to when there is no stored row (`UserConfig::load(..)
/// .unwrap_or_default()`), while `new()` is the richer "first run"
/// constructor used when *writing* a fresh row. This impl matches the
/// read path, because that is the value it stands in for.
impl Default for GeneralSettingsDto {
    fn default() -> Self {
        general_dto(&UserConfig::default())
    }
}

/// See [`GeneralSettingsDto::default`] -- same rationale, same
/// derived-not-restated construction. The two `_configured` secret flags
/// are `false`: a profile with no stored settings has no stored secrets
/// either.
impl Default for NetworkSettingsDto {
    fn default() -> Self {
        network_dto(&UserConfig::default(), false, false)
    }
}

/// See [`GeneralSettingsDto::default`]. Every path is `None` and the
/// vault reads as unavailable: nothing has been resolved yet, which is
/// exactly what a placeholder should claim.
impl Default for SecuritySettingsDto {
    fn default() -> Self {
        security_dto(&MutableSettings::new(
            UserConfig::default(),
            Vec::new(),
            DEFAULT_ENCRYPTED_CRC_POLICY.to_string(),
            default_collision_policy_token(),
            None,
            None,
            None,
        ))
    }
}

/// See [`GeneralSettingsDto`]'s own doc comment. Every field's `Clear` is
/// rejected as `InvalidInput` except `hotkey_bindings`, matching
/// [`PatchValue`]'s general rule: `hotkey_bindings` is the one
/// `Option<T>`-shaped field here (an unset binding map is meaningful --
/// "use every action's built-in default"), while the other three are
/// plain scalars with no "unset" state of their own.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct GeneralSettingsPatch {
    pub hotkey_bindings: PatchValue<String>,
    pub open_nested_in_new_tab: PatchValue<bool>,
    pub drop_behavior: PatchValue<String>,
    pub restore_tabs_on_launch: PatchValue<bool>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SettingsPatch {
    pub expected_revision: u64,
    pub archive: Option<ArchiveSettingsPatch>,
    pub network: Option<NetworkSettingsPatch>,
    pub security: Option<SecuritySettingsPatch>,
    pub general: Option<GeneralSettingsPatch>,
}

/// Re-exported at its historical path: archive profiles are the
/// organization feature's own domain (see [`crate::organization`], which
/// owns the type and the CRUD around it), but they are also what a
/// settings page lists, and `ArclainApp::organization_profiles` has
/// always been reachable from here.
pub use crate::organization::OrganizationProfileSummary;

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

/// One row in a complete password-rule editor save.
///
/// `original_name` identifies the stored rule this row was loaded from,
/// allowing a frontend to rename it without ever receiving its password.
/// `password: None` preserves that identified rule's stored password. A row
/// with no `original_name` is new and therefore must carry a password.
///
/// Not `Clone`/`Serialize`/`Deserialize`: like [`PasswordRuleInput`], this
/// may carry a live [`SecretInput`], and those restrictions are contagious
/// on purpose.
#[derive(Debug)]
pub struct PasswordRuleEditInput {
    pub original_name: Option<String>,
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

/// Removes a previously-written session file, treating "already absent"
/// as success. Used when a frontend's own "restore on launch" toggle is
/// disabled: no open-archive paths should remain discoverable in this
/// file once the user has opted out of session restore -- a file left
/// behind from an *earlier* session, before the toggle was disabled,
/// must actively be removed, not merely left unwritten-to from now on.
pub fn clear_session_restore_list(path: &Path) -> Result<(), ApplicationError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(persistence_io_error(path, error)),
    }
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
    /// The `app_config`-stored pipeline collision default -- see
    /// [`ArchiveSettingsDto::default_collision_policy`] for why an
    /// archive-settings field lives outside `user_config`.
    pub(crate) default_collision_policy: String,
    pub(crate) db_paths: Option<DbPaths>,
    /// This install's computed default vault locations, resolved once at
    /// bootstrap. Immutable for the life of the application: it is where
    /// a `Clear` on either vault-path patch field resets *to*, not a
    /// setting in its own right. Held here (rather than recomputed per
    /// read) because resolving it touches OS config directories and
    /// environment variables, which this module's conversions never do.
    pub(crate) default_db_paths: Option<DbPaths>,
    pub(crate) dbs: Option<ConfigDbs>,
}

impl MutableSettings {
    pub(crate) fn new(
        user_config: UserConfig,
        pass_rules: Vec<PassRule>,
        encrypted_crc_policy: String,
        default_collision_policy: String,
        db_paths: Option<DbPaths>,
        default_db_paths: Option<DbPaths>,
        dbs: Option<ConfigDbs>,
    ) -> Self {
        Self {
            revision: 0,
            user_config,
            pass_rules,
            encrypted_crc_policy,
            default_collision_policy,
            db_paths,
            default_db_paths,
            dbs,
        }
    }
}

// ============================================================================
// Pure DTO <-> domain conversions.
// ============================================================================

pub(crate) fn archive_dto(
    user_config: &UserConfig,
    default_collision_policy: &str,
) -> ArchiveSettingsDto {
    ArchiveSettingsDto {
        backend_mode: BackendModeDto::from_user_config(&user_config.backend_mode),
        cache_directory: user_config.cache_directory.clone().map(PathBuf::from),
        temp_directory: user_config.temp_dir.clone().map(PathBuf::from),
        transfer_directory: user_config.transfer_dir.clone().map(PathBuf::from),
        sevenzip_path: user_config.sevenzip_path.clone().map(PathBuf::from),
        default_collision_policy: default_collision_policy.to_string(),
    }
}

/// The encrypted-CRC policy a profile that has never stored one runs
/// under. Named here so `bootstrap`'s fallback and this module's
/// placeholder cannot disagree.
pub(crate) const DEFAULT_ENCRYPTED_CRC_POLICY: &str = "on_access";

/// The token stored when nothing has ever set the pipeline collision
/// default -- `arclain_core::OutputCollisionPolicy`'s own `Default`,
/// spelled the way settings storage spells it, so this crate never
/// hardcodes the word.
pub(crate) fn default_collision_policy_token() -> String {
    arclain_core::OutputCollisionPolicy::default()
        .to_settings_str()
        .to_string()
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

pub(crate) fn general_dto(user_config: &UserConfig) -> GeneralSettingsDto {
    GeneralSettingsDto {
        hotkey_bindings: user_config.hotkey_bindings.clone(),
        open_nested_in_new_tab: user_config.open_nested_in_new_tab,
        drop_behavior: user_config
            .drop_behavior
            .clone()
            .unwrap_or_else(|| "new_tab".to_string()),
        restore_tabs_on_launch: user_config.restore_tabs_on_launch,
    }
}

pub(crate) fn security_dto(mutable: &MutableSettings) -> SecuritySettingsDto {
    SecuritySettingsDto {
        secrets_database_path: mutable.db_paths.as_ref().map(|p| p.secrets_db.clone()),
        key_file_path: mutable.db_paths.as_ref().and_then(|p| p.key_file.clone()),
        default_secrets_database_path: mutable
            .default_db_paths
            .as_ref()
            .map(|p| p.secrets_db.clone()),
        default_key_file_path: mutable
            .default_db_paths
            .as_ref()
            .and_then(|p| p.key_file.clone()),
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
///
/// Takes `default_collision_policy` alongside `user_config` because the
/// archive settings surface deliberately spans two storage locations (see
/// [`ArchiveSettingsDto::default_collision_policy`]); keeping one patch
/// applier for the whole group means a caller can never apply half of an
/// archive patch by forgetting the other function.
pub(crate) fn apply_archive_patch(
    user_config: &mut UserConfig,
    default_collision_policy: &mut String,
    patch: ArchiveSettingsPatch,
) -> Result<(), ApplicationError> {
    let mut proposed_policy = default_collision_policy.clone();
    apply_required(
        &mut proposed_policy,
        patch.default_collision_policy,
        "archive.default_collision_policy",
    )?;
    if proposed_policy != *default_collision_policy
        && arclain_core::OutputCollisionPolicy::from_settings_str(&proposed_policy).is_none()
    {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "unrecognized pipeline collision policy",
        )
        .with_diagnostic(format!(
            "{proposed_policy:?} is not one of \"fail\", \"skip\", \"overwrite\", \"smart\""
        ))
        .with_recoverability(Recoverability::UserAction)
        .with_field("archive.default_collision_policy"));
    }
    *default_collision_policy = proposed_policy;

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
/// `key_file_path` are handled separately by [`apply_vault_path_patch`]
/// below, called from `settings_ops::repoint_vault_paths` -- unlike a
/// directory override, changing either means re-opening the encrypted
/// vault at a new location, which needs I/O and can fail in ways a pure
/// function can't perform.
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

/// Applies `secrets_database_path`/`key_file_path` to a working copy of
/// `DbPaths`. Pure, like every other `apply_*_patch` in this module,
/// even though the *fields* it patches are `Option<PathBuf>`-shaped like
/// [`ArchiveSettingsPatch::cache_directory`] -- unlike that field,
/// `Clear` here does **not** mean `None`; it means "reset to
/// `defaults`" (this install's own `DbPaths::calculate_defaults`
/// result). See [`SecuritySettingsPatch`]'s own doc comment and
/// [`PatchValue`]'s "one documented exception" note for the full
/// rationale -- a vault must always resolve to some concrete on-disk
/// location, so there is no "no vault path" state the way there is "no
/// cache directory override".
///
/// Takes `paths`/`defaults` rather than reading them itself because
/// resolving either requires I/O this module deliberately never
/// performs (see this module's own top-level doc comment on why patch
/// application stays pure) -- `settings_ops::repoint_vault_paths` is
/// the only caller, and the only place that has a current `DbPaths` to
/// patch and a computed `defaults` to fall back to.
pub(crate) fn apply_vault_path_patch(
    paths: &mut DbPaths,
    patch: &SecuritySettingsPatch,
    defaults: &DbPaths,
) {
    match &patch.secrets_database_path {
        PatchValue::Set(path) => paths.secrets_db = path.clone(),
        PatchValue::Clear => paths.secrets_db = defaults.secrets_db.clone(),
        PatchValue::Keep => {}
    }
    match &patch.key_file_path {
        PatchValue::Set(path) => paths.key_file = Some(path.clone()),
        PatchValue::Clear => paths.key_file = defaults.key_file.clone(),
        PatchValue::Keep => {}
    }
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

/// Applies [`GeneralSettingsPatch`] to a working copy of `user_config`.
/// Pure, like every other `apply_*_patch` in this module. `hotkey_bindings`
/// is the only field that accepts `Clear` -- see [`GeneralSettingsPatch`]'s
/// own doc comment.
pub(crate) fn apply_general_patch(
    user_config: &mut UserConfig,
    patch: GeneralSettingsPatch,
) -> Result<(), ApplicationError> {
    apply_optional(&mut user_config.hotkey_bindings, patch.hotkey_bindings);
    apply_required(
        &mut user_config.open_nested_in_new_tab,
        patch.open_nested_in_new_tab,
        "general.open_nested_in_new_tab",
    )?;
    let mut drop_behavior = user_config
        .drop_behavior
        .clone()
        .unwrap_or_else(|| "new_tab".to_string());
    apply_required(
        &mut drop_behavior,
        patch.drop_behavior,
        "general.drop_behavior",
    )?;
    user_config.drop_behavior = Some(drop_behavior);
    apply_required(
        &mut user_config.restore_tabs_on_launch,
        patch.restore_tabs_on_launch,
        "general.restore_tabs_on_launch",
    )?;
    Ok(())
}

/// Whether `patch` touches the per-plugin proxy opt-in/opt-out map.
/// `settings_ops::run_update_settings` uses this to decide whether the
/// live `AsyncHttpClient`'s routing map needs refreshing even when no
/// SOCKS5 identity field changed -- a patch that changes only this map
/// (no address/username/enabled change) takes the plain
/// `ConfigService::save_user_config` path, which
/// [`network_patch_touches_socks5_identity`] alone would never route
/// through live-routing re-application at all.
pub(crate) fn network_patch_touches_plugin_proxy_map(patch: &NetworkSettingsPatch) -> bool {
    !matches!(patch.plugin_proxy_enabled, PatchValue::Keep)
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
    validate_password_rule_fields(&rule.name, &rule.pattern)
}

/// Validates a complete password-rule edit before any secret is resolved or
/// persistence is attempted. Identity and result-name uniqueness are checked
/// across the whole list so the caller can safely replace it atomically.
pub(crate) fn validate_password_rule_edit_inputs(
    edits: &[PasswordRuleEditInput],
    existing_names: &std::collections::HashSet<&str>,
) -> Result<(), ApplicationError> {
    let mut used_original_names = std::collections::HashSet::new();
    let mut result_names = std::collections::HashSet::new();

    for edit in edits {
        validate_password_rule_fields(&edit.name, &edit.pattern)?;

        if !result_names.insert(edit.name.as_str()) {
            return Err(invalid_input_error(
                "name",
                "password rule names must be unique",
            ));
        }

        match edit.original_name.as_deref() {
            Some(original_name) => {
                if !existing_names.contains(original_name) {
                    return Err(password_rule_original_name_not_found_error());
                }
                if !used_original_names.insert(original_name) {
                    return Err(invalid_input_error(
                        "original_name",
                        "password rule original names must be unique",
                    ));
                }
            }
            None if edit.password.is_none() => {
                return Err(password_required_for_new_rule_error());
            }
            None => {}
        }
    }

    Ok(())
}

fn validate_password_rule_fields(name: &str, pattern: &str) -> Result<(), ApplicationError> {
    if name.trim().is_empty() {
        return Err(invalid_input_error("name", "rule name must not be empty"));
    }
    if pattern.trim().is_empty() {
        return Err(invalid_input_error(
            "pattern",
            "rule pattern must not be empty",
        ));
    }
    if regex::Regex::new(pattern).is_err() {
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

pub(crate) fn password_required_for_new_rule_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "a new password rule requires a password",
    )
    .with_diagnostic("password was None and no existing rule identity has one to keep")
    .with_recoverability(Recoverability::UserAction)
    .with_field("password")
}

pub(crate) fn password_rule_original_name_not_found_error() -> ApplicationError {
    invalid_input_error("original_name", "password rule original name was not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_user_config() -> UserConfig {
        UserConfig::new()
    }

    /// The report crosses to a frontend whole, so its wire shape is part
    /// of the surface: every step, in order, with its detail line, plus
    /// the egress pair the panel's footer is built from.
    #[test]
    fn network_probe_report_round_trips_through_serde() {
        let report = NetworkProbeReport {
            steps: vec![
                ProbeStepDto {
                    name: "DNS".to_string(),
                    passed: true,
                    message: Some("Resolved to 203.0.113.7:1080".to_string()),
                },
                ProbeStepDto {
                    name: "TCP".to_string(),
                    passed: false,
                    message: None,
                },
            ],
            ip: Some("198.51.100.9".to_string()),
            country: Some("Nowhere".to_string()),
        };

        let json = serde_json::to_string(&report).expect("serialize probe report");
        let restored: NetworkProbeReport =
            serde_json::from_str(&json).expect("deserialize probe report");

        assert_eq!(restored, report);
    }

    /// The probe hands this straight to a frontend, so its wire shape is
    /// part of the surface -- both fields must survive a round trip
    /// verbatim, since both are the server's own words.
    #[test]
    fn gameta_server_info_round_trips_through_serde() {
        let info = GametaServerInfo {
            status: "degraded".to_string(),
            version: "1.2.3-rc.4".to_string(),
        };

        let json = serde_json::to_string(&info).expect("serialize server info");
        let restored: GametaServerInfo =
            serde_json::from_str(&json).expect("deserialize server info");

        assert_eq!(restored, info);
    }

    #[test]
    fn gameta_connection_status_distinguishes_disabled_connected_and_unavailable() {
        assert_eq!(
            gameta_connection_status(false, false, None),
            GametaConnectionStatusDto::Disabled,
        );
        assert_eq!(
            gameta_connection_status(true, true, Some("2.4.6".to_string()),),
            GametaConnectionStatusDto::Connected {
                version: Some("2.4.6".to_string()),
            },
        );
        assert_eq!(
            gameta_connection_status(true, false, None),
            GametaConnectionStatusDto::Unavailable,
        );
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

    /// An archive patch that changes nothing, for tests that only care
    /// about one field.
    fn keep_archive_patch() -> ArchiveSettingsPatch {
        ArchiveSettingsPatch {
            backend_mode: PatchValue::Keep,
            cache_directory: PatchValue::Keep,
            temp_directory: PatchValue::Keep,
            transfer_directory: PatchValue::Keep,
            sevenzip_path: PatchValue::Keep,
            default_collision_policy: PatchValue::Keep,
        }
    }

    #[test]
    fn archive_dto_reflects_first_run_defaults() {
        let dto = archive_dto(&default_user_config(), &default_collision_policy_token());
        assert_eq!(dto.backend_mode, BackendModeDto::Native);
        assert!(dto.cache_directory.is_none());
        assert!(dto.temp_directory.is_none());
        assert!(dto.transfer_directory.is_none());
        assert!(dto.sevenzip_path.is_none());
        assert_eq!(dto.default_collision_policy, "smart");
    }

    /// The DTO carries the stored token through verbatim -- including one
    /// nothing recognizes, so a hand-edited `app_config` row is reported
    /// as it actually is rather than silently normalized on read.
    #[test]
    fn archive_dto_reports_the_stored_collision_policy_verbatim() {
        let dto = archive_dto(&default_user_config(), "overwrite");
        assert_eq!(dto.default_collision_policy, "overwrite");

        let dto = archive_dto(&default_user_config(), "hand-edited-nonsense");
        assert_eq!(dto.default_collision_policy, "hand-edited-nonsense");
    }

    #[test]
    fn clear_is_rejected_for_scalar_fields_without_an_empty_state() {
        let mut user_config = default_user_config();
        let mut collision_policy = default_collision_policy_token();
        let patch = ArchiveSettingsPatch {
            backend_mode: PatchValue::Clear,
            ..keep_archive_patch()
        };

        let error =
            apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("archive.backend_mode"));
    }

    #[test]
    fn clear_resets_an_optional_directory_override_to_none() {
        let mut user_config = default_user_config();
        let mut collision_policy = default_collision_policy_token();
        user_config.cache_directory = Some("/old/cache".to_string());
        let patch = ArchiveSettingsPatch {
            cache_directory: PatchValue::Clear,
            ..keep_archive_patch()
        };

        apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap();

        assert!(user_config.cache_directory.is_none());
    }

    #[test]
    fn set_overrides_a_directory_and_keep_leaves_others_untouched() {
        let mut user_config = default_user_config();
        let mut collision_policy = default_collision_policy_token();
        user_config.temp_dir = Some("/old/temp".to_string());
        let patch = ArchiveSettingsPatch {
            cache_directory: PatchValue::Set(PathBuf::from("/new/cache")),
            ..keep_archive_patch()
        };

        apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap();

        assert_eq!(user_config.cache_directory.as_deref(), Some("/new/cache"));
        assert_eq!(user_config.temp_dir.as_deref(), Some("/old/temp"));
    }

    /// Every token `arclain_core::OutputCollisionPolicy` round-trips is
    /// accepted, and the applied value is the token itself -- the facade
    /// stores what the settings page stores, not a re-spelling of it.
    #[test]
    fn every_known_collision_policy_token_is_accepted() {
        for policy in [
            arclain_core::OutputCollisionPolicy::Fail,
            arclain_core::OutputCollisionPolicy::Skip,
            arclain_core::OutputCollisionPolicy::Overwrite,
            arclain_core::OutputCollisionPolicy::Smart,
        ] {
            let token = policy.to_settings_str();
            let mut user_config = default_user_config();
            let mut collision_policy = "fail".to_string();
            let patch = ArchiveSettingsPatch {
                default_collision_policy: PatchValue::Set(token.to_string()),
                ..keep_archive_patch()
            };

            apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap();

            assert_eq!(collision_policy, token);
            assert_eq!(
                archive_dto(&user_config, &collision_policy).default_collision_policy,
                token
            );
        }
    }

    /// A typo would otherwise be stored and then read back by
    /// `from_settings_str` as "no app default at all", silently changing
    /// which policy pipelines run under. Rejecting it makes the mistake
    /// visible at the call that made it.
    #[test]
    fn setting_an_unrecognized_collision_policy_is_rejected() {
        let mut user_config = default_user_config();
        let mut collision_policy = default_collision_policy_token();
        let patch = ArchiveSettingsPatch {
            default_collision_policy: PatchValue::Set("smrt".to_string()),
            ..keep_archive_patch()
        };

        let error =
            apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(
            error.field.as_deref(),
            Some("archive.default_collision_policy")
        );
        assert_eq!(
            collision_policy,
            default_collision_policy_token(),
            "a rejected policy must leave the working copy untouched"
        );
    }

    /// `Keep` never validates: an unrecognized value already on disk
    /// survives an unrelated archive patch instead of blocking it.
    #[test]
    fn keeping_an_already_unrecognized_collision_policy_does_not_fail() {
        let mut user_config = default_user_config();
        let mut collision_policy = "hand-edited-nonsense".to_string();
        let patch = ArchiveSettingsPatch {
            cache_directory: PatchValue::Set(PathBuf::from("/new/cache")),
            ..keep_archive_patch()
        };

        apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap();

        assert_eq!(collision_policy, "hand-edited-nonsense");
    }

    /// The collision policy has no "unset" state to fall back to -- the
    /// pipeline always resolves *some* policy -- so it follows
    /// `PatchValue`'s plain-scalar rule rather than the `Option`-shaped
    /// one, even though the other archive fields around it are optional.
    #[test]
    fn clear_on_the_collision_policy_is_rejected() {
        let mut user_config = default_user_config();
        let mut collision_policy = default_collision_policy_token();
        let patch = ArchiveSettingsPatch {
            default_collision_policy: PatchValue::Clear,
            ..keep_archive_patch()
        };

        let error =
            apply_archive_patch(&mut user_config, &mut collision_policy, patch).unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(
            error.field.as_deref(),
            Some("archive.default_collision_policy")
        );
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

    /// Pins the "fold 2" contract: a patch touching *only* the
    /// per-plugin proxy map (no SOCKS5 identity field) must still be
    /// detected as needing a live-routing refresh --
    /// `network_patch_touches_socks5_identity` alone would say `false`
    /// for exactly this patch, which is the gap this function closes.
    #[test]
    fn network_patch_touches_plugin_proxy_map_is_independent_of_identity_fields() {
        let keep_all = NetworkSettingsPatch {
            socks5_enabled: PatchValue::Keep,
            socks5_address: PatchValue::Keep,
            socks5_username: PatchValue::Keep,
            plugin_proxy_enabled: PatchValue::Keep,
            gameta_server_enabled: PatchValue::Keep,
            gameta_server_url: PatchValue::Keep,
        };
        assert!(!network_patch_touches_plugin_proxy_map(&keep_all));
        assert!(!network_patch_touches_socks5_identity(&keep_all));

        let map_only = NetworkSettingsPatch {
            plugin_proxy_enabled: PatchValue::Clear,
            ..keep_all
        };
        assert!(
            network_patch_touches_plugin_proxy_map(&map_only),
            "clearing the plugin proxy map must be detected"
        );
        assert!(
            !network_patch_touches_socks5_identity(&map_only),
            "a plugin-map-only patch must not be mistaken for an identity change"
        );
    }

    /// Pins the "I6" bridge contract: unlike every other `Option<T>`-
    /// shaped field in this module, `Clear` on `secrets_database_path`/
    /// `key_file_path` resets to the computed default location, not to
    /// `None`/unset. See both `PatchValue`'s and `SecuritySettingsPatch`'s
    /// doc comments for why a vault path has no "unset" state to clear to.
    #[test]
    fn clear_on_vault_paths_resets_to_the_computed_defaults_not_to_unset() {
        let defaults = DbPaths::calculate_defaults("arclain-settings-rs-test")
            .expect("calculate_defaults is pure path computation and should not fail");
        let mut paths = defaults.clone();
        paths.secrets_db = PathBuf::from("/custom/secrets.redb");
        paths.key_file = Some(PathBuf::from("/custom/master.key"));

        let patch = SecuritySettingsPatch {
            secrets_database_path: PatchValue::Clear,
            key_file_path: PatchValue::Clear,
            encrypted_crc_policy: PatchValue::Keep,
        };
        apply_vault_path_patch(&mut paths, &patch, &defaults);

        assert_eq!(
            paths.secrets_db, defaults.secrets_db,
            "Clear on secrets_database_path must reset to the computed default, not to unset"
        );
        assert_eq!(
            paths.key_file, defaults.key_file,
            "Clear on key_file_path must reset to the computed default, not to unset"
        );
    }

    #[test]
    fn set_overrides_a_vault_path_and_keep_leaves_the_other_untouched() {
        let defaults = DbPaths::calculate_defaults("arclain-settings-rs-test")
            .expect("calculate_defaults is pure path computation and should not fail");
        let mut paths = defaults.clone();
        paths.secrets_db = PathBuf::from("/custom/secrets.redb");

        let patch = SecuritySettingsPatch {
            secrets_database_path: PatchValue::Keep,
            key_file_path: PatchValue::Set(PathBuf::from("/new/master.key")),
            encrypted_crc_policy: PatchValue::Keep,
        };
        apply_vault_path_patch(&mut paths, &patch, &defaults);

        assert_eq!(
            paths.secrets_db,
            PathBuf::from("/custom/secrets.redb"),
            "Keep must leave the existing override untouched"
        );
        assert_eq!(paths.key_file, Some(PathBuf::from("/new/master.key")));
    }

    #[test]
    fn general_dto_reflects_first_run_defaults() {
        let dto = general_dto(&default_user_config());
        assert_eq!(dto.hotkey_bindings, None);
        assert!(!dto.open_nested_in_new_tab);
        assert_eq!(dto.drop_behavior, "new_tab");
        assert!(dto.restore_tabs_on_launch);
    }

    /// The placeholder a frontend holds before its first read must be
    /// what this facade itself reports for a profile with no stored
    /// `user_config` row -- otherwise the UI briefly renders preferences
    /// no profile would ever have had.
    #[test]
    fn dto_defaults_match_what_an_unstored_profile_reports() {
        let unstored = UserConfig::default();

        let expected_general = general_dto(&unstored);
        let default_general = GeneralSettingsDto::default();
        assert_eq!(
            default_general.hotkey_bindings,
            expected_general.hotkey_bindings
        );
        assert_eq!(
            default_general.open_nested_in_new_tab,
            expected_general.open_nested_in_new_tab
        );
        assert_eq!(
            default_general.drop_behavior,
            expected_general.drop_behavior
        );
        assert_eq!(
            default_general.restore_tabs_on_launch,
            expected_general.restore_tabs_on_launch
        );
        // Never an empty string: a frontend switching on this token
        // must always get one of the three documented values.
        assert_eq!(default_general.drop_behavior, "new_tab");

        let expected_network = network_dto(&unstored, false, false);
        let default_network = NetworkSettingsDto::default();
        assert_eq!(
            default_network.socks5_enabled,
            expected_network.socks5_enabled
        );
        assert_eq!(
            default_network.socks5_address,
            expected_network.socks5_address
        );
        assert_eq!(
            default_network.plugin_proxy_enabled,
            expected_network.plugin_proxy_enabled
        );
        assert!(!default_network.socks5_password_configured);
        assert!(!default_network.gameta_api_key_configured);
    }

    /// A default-proxied plugin with no stored entry still reads as
    /// routed -- the whole reason the effective map exists separately
    /// from the stored overrides.
    #[test]
    fn the_effective_proxy_map_applies_the_defaults_the_stored_overrides_omit() {
        let network = NetworkSettingsDto {
            socks5_enabled: true,
            plugin_proxy_enabled: BTreeMap::from([("custom".to_string(), true)]),
            ..NetworkSettingsDto::default()
        };

        let effective = network.effective_plugin_proxy_enabled();
        assert_eq!(effective.get("dlsite"), Some(&true));
        assert_eq!(effective.get("custom"), Some(&true));
        assert!(network.plugin_proxy_effective("dlsite-metadata"));
        assert!(network.plugin_proxy_effective("custom"));
        assert!(!network.plugin_proxy_effective("unknown-plugin"));
    }

    /// The single-plugin read and the whole-map read are two
    /// implementations of one rule, so every case must agree -- a
    /// divergence would render a toggle that disagrees with what the
    /// network stack does.
    #[test]
    fn the_two_effective_proxy_reads_always_agree() {
        let overrides = [
            ("dlsite", false),
            ("dlsite-api", true),
            ("custom", true),
            ("custom-off", false),
        ];
        for socks5_enabled in [false, true] {
            let network = NetworkSettingsDto {
                socks5_enabled,
                plugin_proxy_enabled: overrides
                    .iter()
                    .map(|(id, enabled)| ((*id).to_string(), *enabled))
                    .collect(),
                ..NetworkSettingsDto::default()
            };
            let map = network.effective_plugin_proxy_enabled();
            for plugin_id in [
                "dlsite",
                "dlsite-api",
                "dlsite-html",
                "dlsite-metadata",
                "custom",
                "custom-off",
                "never-mentioned",
            ] {
                assert_eq!(
                    network.plugin_proxy_effective(plugin_id),
                    map.get(plugin_id).copied().unwrap_or(false),
                    "the two reads disagree for {plugin_id:?} with socks5_enabled={socks5_enabled}"
                );
            }
        }
    }

    /// An explicit stored `false` still wins over the default-on list,
    /// and disabling the global proxy clears every route.
    #[test]
    fn the_effective_proxy_map_honours_opt_outs_and_the_global_switch() {
        let opted_out = NetworkSettingsDto {
            socks5_enabled: true,
            plugin_proxy_enabled: BTreeMap::from([("dlsite".to_string(), false)]),
            ..NetworkSettingsDto::default()
        };
        assert!(!opted_out.plugin_proxy_effective("dlsite"));
        assert!(opted_out.plugin_proxy_effective("dlsite-api"));

        let globally_off = NetworkSettingsDto {
            socks5_enabled: false,
            plugin_proxy_enabled: BTreeMap::from([("dlsite".to_string(), true)]),
            ..NetworkSettingsDto::default()
        };
        assert!(globally_off.effective_plugin_proxy_enabled().is_empty());
        assert!(!globally_off.plugin_proxy_effective("dlsite"));
    }

    #[test]
    fn general_patch_set_updates_every_field() {
        let mut user_config = default_user_config();
        let patch = GeneralSettingsPatch {
            hotkey_bindings: PatchValue::Set("{\"open\":\"Ctrl+O\"}".to_string()),
            open_nested_in_new_tab: PatchValue::Set(true),
            drop_behavior: PatchValue::Set("replace".to_string()),
            restore_tabs_on_launch: PatchValue::Set(false),
        };

        apply_general_patch(&mut user_config, patch).unwrap();

        assert_eq!(
            user_config.hotkey_bindings.as_deref(),
            Some("{\"open\":\"Ctrl+O\"}")
        );
        assert!(user_config.open_nested_in_new_tab);
        assert_eq!(user_config.drop_behavior.as_deref(), Some("replace"));
        assert!(!user_config.restore_tabs_on_launch);
    }

    #[test]
    fn general_patch_keep_leaves_every_field_untouched() {
        let mut user_config = default_user_config();
        user_config.hotkey_bindings = Some("{\"open\":\"Ctrl+O\"}".to_string());
        user_config.open_nested_in_new_tab = true;
        user_config.drop_behavior = Some("replace".to_string());
        user_config.restore_tabs_on_launch = false;
        let before = user_config.clone();

        apply_general_patch(
            &mut user_config,
            GeneralSettingsPatch {
                hotkey_bindings: PatchValue::Keep,
                open_nested_in_new_tab: PatchValue::Keep,
                drop_behavior: PatchValue::Keep,
                restore_tabs_on_launch: PatchValue::Keep,
            },
        )
        .unwrap();

        assert_eq!(user_config.hotkey_bindings, before.hotkey_bindings);
        assert_eq!(
            user_config.open_nested_in_new_tab,
            before.open_nested_in_new_tab
        );
        assert_eq!(user_config.drop_behavior, before.drop_behavior);
        assert_eq!(
            user_config.restore_tabs_on_launch,
            before.restore_tabs_on_launch
        );
    }

    #[test]
    fn general_patch_clear_resets_hotkey_bindings_to_none() {
        let mut user_config = default_user_config();
        user_config.hotkey_bindings = Some("{\"open\":\"Ctrl+O\"}".to_string());

        apply_general_patch(
            &mut user_config,
            GeneralSettingsPatch {
                hotkey_bindings: PatchValue::Clear,
                open_nested_in_new_tab: PatchValue::Keep,
                drop_behavior: PatchValue::Keep,
                restore_tabs_on_launch: PatchValue::Keep,
            },
        )
        .unwrap();

        assert!(user_config.hotkey_bindings.is_none());
    }

    #[test]
    fn general_patch_clear_is_rejected_for_scalar_fields_without_an_empty_state() {
        let mut user_config = default_user_config();

        let error = apply_general_patch(
            &mut user_config,
            GeneralSettingsPatch {
                hotkey_bindings: PatchValue::Keep,
                open_nested_in_new_tab: PatchValue::Clear,
                drop_behavior: PatchValue::Keep,
                restore_tabs_on_launch: PatchValue::Keep,
            },
        )
        .unwrap_err();

        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(
            error.field.as_deref(),
            Some("general.open_nested_in_new_tab")
        );
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

    #[test]
    fn clear_session_restore_list_removes_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.json");
        save_session_restore_list(
            &path,
            &[SessionArchiveEntry {
                source_path: PathBuf::from("/a.zip"),
            }],
        )
        .unwrap();
        assert!(path.exists());

        clear_session_restore_list(&path).unwrap();

        assert!(!path.exists());
    }

    /// Removing a file that never existed (or was already removed) is
    /// success, not an error -- the caller (a disabled "restore on
    /// launch" toggle with no prior session file at all) should never
    /// have to distinguish "there was nothing to clear" from "clearing
    /// succeeded".
    #[test]
    fn clear_session_restore_list_on_a_missing_file_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("never-existed").join("session.json");

        clear_session_restore_list(&path).unwrap();
    }
}
