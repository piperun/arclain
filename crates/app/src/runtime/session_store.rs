//! Everything one `ArclainApp::bootstrap` call composed: the concrete
//! headless services (databases, backends, plugins, caches) `capabilities`
//! and `health` read from, plus [`LegacyComposition`] -- the transitional
//! handle `crates/ui`'s not-yet-migrated `AppState`/`Services` construction
//! pulls its legacy-shaped fields from.
//!
//! [`LegacyComposition`] is not part of the frontend-neutral operation
//! surface `contract.md` describes; a Flutter/Dart bridge must never use
//! it. It exists only because `crates/ui` has ~200 call sites reading
//! `SharedState.app_state`/`SharedState.services` fields directly, and
//! migrating every one of them is explicitly out of scope for this task
//! (later Stage 1 tasks retire them incrementally onto `ArclainApp`'s own
//! async operation methods). Each retired call site is one field here
//! that becomes removable.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex as SyncMutex;

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::services::Services as CoreServices;
use arclain_core::utilities::ChecksumService;
use arclain_core::{ConfigDbs, ContentCache, DbPaths, PassRule, ResourceManager, UserConfig};
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
    plugins_available: bool,
) -> AppCapabilities {
    use arclain_core::archive::BackendCapabilities;

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
        plugins_available,
    }
}

/// Builds a [`HealthSnapshot`] from already-known component state. Pure,
/// for the same reason [`compute_capabilities`] is. `ready` is exactly
/// "nothing is listed as degraded" -- every optional component that is
/// unavailable is named in `degraded_components` so a frontend doesn't
/// need to re-derive the same reasoning from raw booleans.
pub(crate) fn compute_health(
    sevenzip_available: bool,
    unrar_available: bool,
    plugins_available: bool,
    database_ready: bool,
) -> HealthSnapshot {
    let mut degraded_components = Vec::new();
    if !sevenzip_available {
        degraded_components.push("sevenzip".to_string());
    }
    if !unrar_available {
        degraded_components.push("unrar".to_string());
    }
    if !plugins_available {
        degraded_components.push("plugins".to_string());
    }
    if !database_ready {
        degraded_components.push("database".to_string());
    }

    HealthSnapshot {
        ready: degraded_components.is_empty(),
        degraded_components,
    }
}

/// Transitional bundle handed to `crates/ui`'s legacy `AppState`/
/// `Services` construction. See the module doc comment.
pub struct LegacyComposition {
    pub core_services: Arc<CoreServices>,
    pub plugin_manager: Option<Arc<SyncMutex<PluginManager>>>,
    pub content_cache: Option<Arc<ContentCache>>,
    pub resource_manager: Option<Arc<ResourceManager>>,
    pub checksum_service: Option<Arc<ChecksumService>>,
    pub user_config: UserConfig,
    pub pass_rules: Vec<PassRule>,
    pub backend_selector: BackendSelector,
    pub fallback_backend: SevenZipCli,
    pub encrypted_crc_policy: String,
    pub db_paths: Option<DbPaths>,
    /// `None` if this composition's `dbs` was already taken by an
    /// earlier call to [`ArclainApp::take_legacy_composition`]
    /// (`crates/ui` calls this exactly once at startup, so in practice
    /// this is always `Some` the one time it matters).
    pub dbs: Option<ConfigDbs>,
    pub plugin_event_scheduler: Option<PluginEventScheduler>,
}

/// Everything one bootstrap composed. Owned by `AppRuntime` for the
/// application's lifetime; `capabilities()`/`health()` read the small
/// always-retained fields directly, and
/// [`take_legacy_composition`](Self::take_legacy_composition) hands the
/// rest to `crates/ui` (see the module doc comment).
pub(crate) struct SessionStore {
    pub(crate) core_services: Arc<CoreServices>,
    pub(crate) plugin_manager: Option<Arc<SyncMutex<PluginManager>>>,
    pub(crate) content_cache: Option<Arc<ContentCache>>,
    pub(crate) resource_manager: Option<Arc<ResourceManager>>,
    pub(crate) checksum_service: Option<Arc<ChecksumService>>,
    pub(crate) user_config: UserConfig,
    pub(crate) pass_rules: Vec<PassRule>,
    pub(crate) backend_selector: BackendSelector,
    pub(crate) fallback_backend: SevenZipCli,
    /// Whether `arclain_core::backends::UnrarCli::detect()` found an
    /// UnRAR CLI executable. Not `Option<PathBuf>`: `UnrarCli` exposes no
    /// path accessor -- see [`compute_capabilities`]'s doc comment.
    pub(crate) unrar_available: bool,
    pub(crate) encrypted_crc_policy: String,
    pub(crate) db_paths: Option<DbPaths>,
    pub(crate) dbs: SyncMutex<Option<ConfigDbs>>,
    pub(crate) plugin_event_scheduler: Option<PluginEventScheduler>,
    /// Snapshotted once at bootstrap time: whether `open_databases` (and
    /// `init_db_services`) succeeded. Cached rather than re-derived from
    /// `dbs` because `dbs` itself is one-time-taken by
    /// `take_legacy_composition` -- after that, `SessionStore` no longer
    /// holds a live database handle at all (nor should it: `crates/ui`'s
    /// `vault_ops.rs` re-keys/replaces the databases at runtime directly
    /// on its own copy, entirely independent of this facade today; see
    /// this task's report for that limitation).
    pub(crate) database_ready: bool,
}

impl SessionStore {
    pub(crate) fn capabilities(&self) -> AppCapabilities {
        compute_capabilities(
            // A live SessionStore always has a `fallback_backend`:
            // bootstrap fails otherwise.
            Some(self.fallback_backend.exe_path()),
            self.unrar_available,
            self.plugin_manager.is_some(),
        )
    }

    pub(crate) fn health(&self) -> HealthSnapshot {
        compute_health(
            true,
            self.unrar_available,
            self.plugin_manager.is_some(),
            self.database_ready,
        )
    }

    pub(crate) fn take_legacy_composition(&self) -> LegacyComposition {
        LegacyComposition {
            core_services: self.core_services.clone(),
            plugin_manager: self.plugin_manager.clone(),
            content_cache: self.content_cache.clone(),
            resource_manager: self.resource_manager.clone(),
            checksum_service: self.checksum_service.clone(),
            user_config: self.user_config.clone(),
            pass_rules: self.pass_rules.clone(),
            backend_selector: self.backend_selector.clone(),
            fallback_backend: self.fallback_backend.clone(),
            encrypted_crc_policy: self.encrypted_crc_policy.clone(),
            db_paths: self.db_paths.clone(),
            dbs: self.dbs.lock().take(),
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
    fn missing_unrar_leaves_capabilities_full_when_7z_present_but_flags_health() {
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

        let health = compute_health(true, false, true, true);
        assert!(!health.ready);
        assert_eq!(health.degraded_components, vec!["unrar"]);
    }

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
