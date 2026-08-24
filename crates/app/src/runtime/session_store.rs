//! Everything one `ArclainApp::bootstrap` call composed: the concrete
//! headless services (databases, backends, plugins, caches) `capabilities`
//! and `health` read from, plus [`LegacyComposition`] -- the transitional
//! handle `crates/ui`'s not-yet-migrated `AppState` construction pulls its
//! legacy-shaped fields from.
//!
//! [`LegacyComposition`] is not part of the frontend-neutral application
//! surface; a Flutter/Dart bridge must never use it. It exists only
//! because `crates/ui` has ~200 call sites reading
//! `SharedState.app_state` fields directly, and they migrate onto
//! `ArclainApp`'s own async operation methods incrementally. The
//! `core_services` member remains as a compatibility/test probe while
//! those legacy consumers are retired; the production UI no longer
//! installs it into its own services container.

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "plugin-host")]
use parking_lot::Mutex as SyncMutex;

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::services::Services as CoreServices;
use arclain_core::utilities::ChecksumService;
use arclain_core::{ConfigDbs, ContentCache, DbPaths, PassRule, ResourceManager, UserConfig};
#[cfg(feature = "plugin-host")]
use arclain_plugins::{PluginEventScheduler, PluginManager};

/// One archive backend's reported read/write capabilities, as a frontend
/// needs to display them -- for example, showing "cannot create new .rar
/// archives" instead of failing silently the first time the user tries.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct BackendCapabilityDto {
    pub backend: String,
    pub formats: Vec<String>,
    pub can_list: bool,
    pub can_extract: bool,
    pub can_create: bool,
    pub can_modify: bool,
}

/// Whether an external CLI tool arclain shells out to was found, and
/// where.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ExternalToolStatusDto {
    pub tool: String,
    pub available: bool,
    pub resolved_path: Option<PathBuf>,
}

/// A point-in-time report of what this running application can actually
/// do -- which archive formats support which operations, and whether the
/// external tools/plugins those operations may depend on are present.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AppCapabilities {
    pub archive_backends: Vec<BackendCapabilityDto>,
    pub external_tools: Vec<ExternalToolStatusDto>,
    #[cfg(feature = "plugin-host")]
    pub plugins_available: bool,
}

/// A coarse liveness/readiness signal a frontend can poll (or show in a
/// status bar) without parsing [`AppCapabilities`] itself.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct HealthSnapshot {
    pub ready: bool,
    pub degraded_components: Vec<String>,
}

/// Builds [`AppCapabilities`] from already-known component state. Pure
/// and total: given the same inputs it always returns the same DTO, with
/// no dependency on the real filesystem or `PATH` -- which is exactly
/// what makes the five scenarios in this module's tests (native-only,
/// missing 7z, missing unrar, degraded plugins, fully ready) hermetically
/// testable without needing to control what the test-running machine
/// actually has installed.
///
/// `sevenzip_available` unconditionally makes every format's capability
/// report `full_featured` (matching `BackendCapabilities::full_featured`)
/// -- inherited from `arclain_core::backends::sevenz_cli::SevenZipCli`'s
/// own `capabilities()` impl, which reports full read/write support
/// unconditionally regardless of which archive format it is actually
/// backing (7-Zip's CLI cannot, for example, truly *create* a `.rar`
/// archive). That modeling choice predates this task; mirrored here
/// rather than corrected, since correcting it is a behavior change this
/// refactor does not make.
///
/// `sevenzip_resolved_path` carries 7-Zip's real, resolved executable
/// path (`Some` means available; `arclain_core::backends::sevenz_cli::
/// SevenZipCli` exposes `exe_path()` for exactly this). `unrar_available`
/// is a plain bool rather than a resolved path for a narrower reason:
/// `arclain_core::backends::UnrarCli` exposes no path accessor at all
/// (unlike `SevenZipCli`), so only its availability -- not its location
/// -- is knowable from this crate today. `ExternalToolStatusDto::
/// resolved_path` is therefore always `None` for `"unrar"`.
pub(crate) fn compute_capabilities(
    sevenzip_resolved_path: Option<&std::path::Path>,
    unrar_available: bool,
    plugin_host_available: bool,
) -> AppCapabilities {
    use arclain_core::archive::BackendCapabilities;

    #[cfg(not(feature = "plugin-host"))]
    let _ = plugin_host_available;

    let sevenzip_available = sevenzip_resolved_path.is_some();
    let full = BackendCapabilities::full_featured();
    let read_only = BackendCapabilities::read_only();
    // 7-Zip present -> every chain's fallback link is full-featured, and
    // a fallback union with a full-featured backend is always
    // full-featured. 7-Zip absent -> only each format's always-present
    // native backend applies.
    let zip_and_rar_caps = if sevenzip_available { full } else { read_only };

    let backend_dto =
        |backend: &str, formats: &[&str], caps: BackendCapabilities| BackendCapabilityDto {
            backend: backend.to_string(),
            formats: formats.iter().map(|format| format.to_string()).collect(),
            can_list: true,
            can_extract: caps.can_extract,
            can_create: caps.can_create,
            can_modify: caps.can_modify_files,
        };

    AppCapabilities {
        archive_backends: vec![
            backend_dto("zip", &["zip"], zip_and_rar_caps),
            backend_dto("rar", &["rar"], zip_and_rar_caps),
            // The native 7z backend (`SevenZBackend`) is always
            // full-featured on its own, independent of whether the CLI
            // fallback is available.
            backend_dto("7z", &["7z", "exe", "sfx"], full),
        ],
        external_tools: vec![
            ExternalToolStatusDto {
                tool: "7z".to_string(),
                available: sevenzip_available,
                resolved_path: sevenzip_resolved_path.map(std::path::Path::to_path_buf),
            },
            ExternalToolStatusDto {
                tool: "unrar".to_string(),
                available: unrar_available,
                resolved_path: None,
            },
        ],
        #[cfg(feature = "plugin-host")]
        plugins_available: plugin_host_available,
    }
}

/// Builds a [`HealthSnapshot`] from already-known component state. Pure,
/// for the same reason [`compute_capabilities`] is.
///
/// `degraded_components` names every unavailable component, required or
/// not, so a frontend doesn't need to re-derive the same reasoning from
/// raw booleans -- but `ready` reflects only the **required** ones:
/// `database`, `plugins` (the plugin *runtime*/engine constructing
/// successfully -- not whether any individual plugin is installed;
/// `PluginManager::new`+`init()` succeeds with zero plugins loaded just
/// as often as with several), and `sevenzip` (browsing an archive does
/// not need the CLI, but extracting, creating and converting one all do,
/// so an application without it cannot do the work a user came for).
/// "Required" here means *not fully operational without it*, not *cannot
/// start*: bootstrap succeeds with no 7-Zip and reports it here instead.
///
/// `unrar` is the one **optional** component: its absence is reported
/// in `degraded_components` (a frontend may still want to warn the user
/// that some RAR operations are unavailable), but never clears `ready`
/// -- unlike 7-Zip, there is no scenario where unrar's absence prevents
/// the application from doing useful archive work at all (RAR read
/// access still works through the native backend).
pub(crate) fn compute_health(
    sevenzip_available: bool,
    unrar_available: bool,
    plugin_host_available: bool,
    database_ready: bool,
) -> HealthSnapshot {
    #[cfg(not(feature = "plugin-host"))]
    let _ = plugin_host_available;

    let mut degraded_components = Vec::new();
    let mut required_degraded = false;

    if !sevenzip_available {
        degraded_components.push("sevenzip".to_string());
        required_degraded = true;
    }
    #[cfg(feature = "plugin-host")]
    if !plugin_host_available {
        degraded_components.push("plugins".to_string());
        required_degraded = true;
    }
    if !database_ready {
        degraded_components.push("database".to_string());
        required_degraded = true;
    }
    // Optional: reported, but does not by itself clear readiness.
    if !unrar_available {
        degraded_components.push("unrar".to_string());
    }

    HealthSnapshot {
        ready: !required_degraded,
        degraded_components,
    }
}

/// Transitional bundle handed to `crates/ui`'s legacy `AppState`
/// construction. See the module doc comment.
pub struct LegacyComposition {
    pub core_services: Arc<CoreServices>,
    #[cfg(feature = "plugin-host")]
    pub plugin_manager: Option<Arc<SyncMutex<PluginManager>>>,
    pub content_cache: Option<Arc<ContentCache>>,
    pub resource_manager: Option<Arc<ResourceManager>>,
    pub checksum_service: Option<Arc<ChecksumService>>,
    pub user_config: UserConfig,
    pub pass_rules: Vec<PassRule>,
    pub backend_selector: BackendSelector,
    /// `None` when bootstrap found no 7-Zip -- see [`SessionStore::
    /// fallback_backend`].
    pub fallback_backend: Option<SevenZipCli>,
    pub encrypted_crc_policy: String,
    pub db_paths: Option<DbPaths>,
    /// `None` only if bootstrap itself could not open the encrypted
    /// vault (no key file, corrupt databases, ...). Otherwise always
    /// `Some` -- including on a second or later call to
    /// [`ArclainApp::take_legacy_composition`]: unlike before this task,
    /// this is a clone of the facade's own live vault state, not a
    /// one-shot-taken value, so `crates/ui` can call this again to
    /// refresh its mirror after a facade-driven settings/vault mutation.
    pub dbs: Option<ConfigDbs>,
    #[cfg(feature = "plugin-host")]
    pub plugin_event_scheduler: Option<PluginEventScheduler>,
}

/// Everything one bootstrap composed. Owned by `AppRuntime` for the
/// application's lifetime; `capabilities()`/`health()` read the small
/// always-retained fields directly, and
/// [`take_legacy_composition`](Self::take_legacy_composition) hands the
/// rest to `crates/ui` (see the module doc comment).
pub(crate) struct SessionStore {
    pub(crate) core_services: Arc<CoreServices>,
    #[cfg(feature = "plugin-host")]
    pub(crate) plugin_manager: Option<Arc<SyncMutex<PluginManager>>>,
    pub(crate) content_cache: Option<Arc<ContentCache>>,
    pub(crate) resource_manager: Option<Arc<ResourceManager>>,
    pub(crate) checksum_service: Option<Arc<ChecksumService>>,
    pub(crate) backend_selector: BackendSelector,
    /// The 7-Zip CLI bootstrap resolved, or `None` when there was none to
    /// resolve. Absence is recorded rather than fatal: the native
    /// backends list and index zip/rar/7z without it, and the operations
    /// that do need it check availability at invocation time.
    pub(crate) fallback_backend: Option<SevenZipCli>,
    /// Whether `arclain_core::backends::UnrarCli::detect()` found an
    /// UnRAR CLI executable. Not `Option<PathBuf>`: `UnrarCli` exposes no
    /// path accessor -- see [`compute_capabilities`]'s doc comment.
    pub(crate) unrar_available: bool,
    #[cfg(feature = "plugin-host")]
    pub(crate) plugin_event_scheduler: Option<PluginEventScheduler>,
    /// Snapshotted once at bootstrap time: whether `open_databases` (and
    /// `init_db_services`) succeeded. Cached rather than re-derived from
    /// `mutable.dbs` because unlike that field, this coarse flag isn't
    /// something a live vault move/rekey should ever change back to
    /// `false` (a *working* vault only ever moves to another working
    /// vault -- see `crate::settings::security_dto`'s own
    /// `vault_available`, which *does* read `mutable.dbs.is_some()` live,
    /// for the settings-facing signal this task actually needs).
    pub(crate) database_ready: bool,
    /// Everything Task 10 (settings/secrets/vault) can change at
    /// runtime, behind one lock so a vault move/rekey -- which changes
    /// `dbs`, `db_paths`, and `pass_rules` together -- can never be
    /// observed half-updated. This is the single-authority fix
    /// `crate::settings`'s own module doc comment describes: before this
    /// task, the fields now bundled here were either bootstrap-frozen
    /// (`user_config`, `pass_rules`) or one-shot-taken by
    /// `take_legacy_composition` (`dbs`), so nothing behind the facade
    /// itself could observe a settings/vault change `crates/ui` made
    /// after bootstrap.
    pub(crate) mutable: parking_lot::RwLock<crate::settings::MutableSettings>,
}

impl SessionStore {
    /// Whether a 7-Zip executable is available to this instance right
    /// now: one was resolved at bootstrap *and* it is still on disk. A
    /// live filesystem check rather than a cached flag, because both
    /// halves can be false independently -- bootstrap succeeds with no
    /// 7-Zip at all (`fallback_backend` is `None`), and one that *was*
    /// resolved can be deleted or moved after bootstrap and before a
    /// later `capabilities()`/`health()` call.
    fn sevenzip_still_available(&self) -> bool {
        self.sevenzip_path().is_some()
    }

    /// The resolved 7-Zip executable, or `None` if none was found at
    /// bootstrap or it has since disappeared.
    fn sevenzip_path(&self) -> Option<&std::path::Path> {
        self.fallback_backend
            .as_ref()
            .map(SevenZipCli::exe_path)
            .filter(|path| path.exists())
    }

    pub(crate) fn capabilities(&self) -> AppCapabilities {
        let sevenzip_path = self.sevenzip_path();
        #[cfg(feature = "plugin-host")]
        let plugin_host_available = self.plugin_manager.is_some();
        #[cfg(not(feature = "plugin-host"))]
        let plugin_host_available = false;
        compute_capabilities(sevenzip_path, self.unrar_available, plugin_host_available)
    }

    pub(crate) fn health(&self) -> HealthSnapshot {
        #[cfg(feature = "plugin-host")]
        let plugin_host_available = self.plugin_manager.is_some();
        #[cfg(not(feature = "plugin-host"))]
        let plugin_host_available = false;
        compute_health(
            self.sevenzip_still_available(),
            self.unrar_available,
            plugin_host_available,
            self.database_ready,
        )
    }

    /// Builds a fresh [`LegacyComposition`] snapshot for `crates/ui`'s
    /// legacy `AppState`/`Services` construction. Non-destructive --
    /// every field here is cloned out of live state, never taken -- so
    /// this is safe to call more than once: once at startup (as before
    /// this task), and again any time `crates/ui`'s own mirror needs to
    /// catch up with a facade-driven settings/vault mutation (see
    /// `crate::settings`'s module doc comment). `dbs`/`db_paths`/
    /// `user_config`/`pass_rules`/`encrypted_crc_policy` all come from
    /// the same `mutable` lock a concurrent settings mutation also goes
    /// through, so a caller can never observe them torn relative to each
    /// other.
    pub(crate) fn take_legacy_composition(&self) -> LegacyComposition {
        let mutable = self.mutable.read();
        LegacyComposition {
            core_services: self.core_services.clone(),
            #[cfg(feature = "plugin-host")]
            plugin_manager: self.plugin_manager.clone(),
            content_cache: self.content_cache.clone(),
            resource_manager: self.resource_manager.clone(),
            checksum_service: self.checksum_service.clone(),
            user_config: mutable.user_config.clone(),
            pass_rules: mutable.pass_rules.clone(),
            backend_selector: self.backend_selector.clone(),
            fallback_backend: self.fallback_backend.clone(),
            encrypted_crc_policy: mutable.encrypted_crc_policy.clone(),
            db_paths: mutable.db_paths.clone(),
            dbs: mutable.dbs.clone(),
            #[cfg(feature = "plugin-host")]
            plugin_event_scheduler: self.plugin_event_scheduler.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Capability/health scenario coverage. See `tests/bootstrap.rs`'s
    // module doc comment for why these scenarios are unit-tested against
    // the pure compute functions here rather than through a real
    // `bootstrap()` call.

    const SEVENZIP_PATH: &str = "/opt/7zip/7zz";

    #[test]
    fn native_only_operation_reports_read_only_zip_and_rar_but_full_7z() {
        let capabilities = compute_capabilities(None, false, true);
        let zip = capabilities
            .archive_backends
            .iter()
            .find(|b| b.backend == "zip")
            .unwrap();
        let rar = capabilities
            .archive_backends
            .iter()
            .find(|b| b.backend == "rar")
            .unwrap();
        let sevenz = capabilities
            .archive_backends
            .iter()
            .find(|b| b.backend == "7z")
            .unwrap();
        assert!(zip.can_extract && !zip.can_create);
        assert!(rar.can_extract && !rar.can_create);
        assert!(
            sevenz.can_extract && sevenz.can_create,
            "native 7z backend is always full-featured"
        );

        let health = compute_health(false, false, true, true);
        assert!(!health.ready);
        assert_eq!(health.degraded_components, vec!["sevenzip", "unrar"]);
    }

    #[test]
    fn missing_7z_degrades_zip_and_rar_write_capability_and_health() {
        let capabilities = compute_capabilities(None, true, true);
        let zip = capabilities
            .archive_backends
            .iter()
            .find(|b| b.backend == "zip")
            .unwrap();
        assert!(!zip.can_create);
        let sevenzip_tool = capabilities
            .external_tools
            .iter()
            .find(|t| t.tool == "7z")
            .unwrap();
        assert!(!sevenzip_tool.available);

        let health = compute_health(false, true, true, true);
        assert!(!health.ready);
        assert_eq!(health.degraded_components, vec!["sevenzip"]);
    }

    #[test]
    fn missing_unrar_leaves_capabilities_full_when_7z_present_and_does_not_clear_readiness() {
        let sevenzip_path = std::path::Path::new(SEVENZIP_PATH);
        let capabilities = compute_capabilities(Some(sevenzip_path), false, true);
        let unrar_tool = capabilities
            .external_tools
            .iter()
            .find(|t| t.tool == "unrar")
            .unwrap();
        assert!(!unrar_tool.available);
        assert_eq!(unrar_tool.resolved_path, None);
        let rar = capabilities
            .archive_backends
            .iter()
            .find(|b| b.backend == "rar")
            .unwrap();
        assert!(
            rar.can_create,
            "7z CLI fallback present -> capability model reports full_featured"
        );

        // unrar is the one *optional* component (see `compute_health`'s
        // doc comment): its absence is still surfaced in
        // `degraded_components` for a frontend that wants to mention
        // it, but does not by itself make the application "not ready"
        // -- unlike a missing database, plugin runtime, or 7-Zip.
        let health = compute_health(true, false, true, true);
        assert!(health.ready);
        assert_eq!(health.degraded_components, vec!["unrar"]);
    }

    #[cfg(feature = "plugin-host")]
    #[test]
    fn degraded_plugins_are_reported_without_affecting_archive_capabilities() {
        let sevenzip_path = std::path::Path::new(SEVENZIP_PATH);
        let capabilities = compute_capabilities(Some(sevenzip_path), true, false);
        assert!(!capabilities.plugins_available);
        assert!(!capabilities.archive_backends.is_empty());

        let health = compute_health(true, true, false, true);
        assert!(!health.ready);
        assert_eq!(health.degraded_components, vec!["plugins"]);
    }

    #[cfg(feature = "plugin-host")]
    #[test]
    fn fully_ready_runtime_has_no_degraded_components() {
        let sevenzip_path = std::path::Path::new(SEVENZIP_PATH);
        let capabilities = compute_capabilities(Some(sevenzip_path), true, true);
        assert!(capabilities.plugins_available);
        assert!(capabilities
            .external_tools
            .iter()
            .all(|tool| tool.available));
        let sevenzip_tool = capabilities
            .external_tools
            .iter()
            .find(|t| t.tool == "7z")
            .unwrap();
        assert_eq!(sevenzip_tool.resolved_path.as_deref(), Some(sevenzip_path));

        let health = compute_health(true, true, true, true);
        assert!(health.ready);
        assert!(health.degraded_components.is_empty());
    }
}
