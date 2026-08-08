//! WASM runtime wrapper using wasmtime component model
//!
//! This module provides the WASM runtime with full host function support.

use crate::conversions::convert_plugin_rule_definition;
use crate::host_functions::HostFunctions;
use crate::types::{PluginCapability, PluginError, PluginExtensionPoint, PluginMetadata, Result};
use wirt::conversions::{
    convert_plugin_action, convert_plugin_layout,
    convert_plugin_rule_definition as convert_wirt_rule, convert_top_tab_config,
};
use wirt::PluginWorld;

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

const FUEL_PER_EXPORT: u64 = 10_000_000;
// The epoch pair below is a liveness dead-man switch, NOT a work budget:
// `FUEL_PER_EXPORT` above is the load-bearing bound on how much a plugin
// may compute per export, and it is deterministic regardless of machine
// speed or load. Wall-clock enters only because fuel cannot see time
// spent on the host side of a hostcall: a guest *looping* over cheap
// hostcalls burns almost no fuel while pinning the plugin worker thread
// for as long as the loop keeps running. That loop is what this ceiling
// reaps -- and it is the only thing it can reap. The trap lands at guest
// epoch checks (function entries and loop backedges), so a single
// hostcall that never returns pins the worker at ANY deadline value;
// bounding an individual hostcall is each hostcall implementation's own
// job (the network layer's per-request timeout, etc.), never this
// mechanism's.
//
// Sizing rule: this ceiling must dwarf the slowest *legitimate* export,
// not typical work, because tripping it is catastrophic -- an epoch trap
// (`Trap::Interrupt`) permanently poisons the instance (see
// `resource_quota_reason`), and the shipped product has no recovery path
// (`unavailable_reason` has no production consumer and `reload_plugin`
// no production caller), so a false positive kills the plugin until the
// application restarts. The network layer's per-request contract allows
// a single hostcall `arclain_network::DEFAULT_REQUEST_TIMEOUT` (30s) --
// and that bound covers whole transfers, which resume beyond it via
// ranged requests rather than running longer -- so an export making a
// few sequential requests legitimately runs minutes. The ceiling
// therefore sits at minutes; the sizing test pins its floor.
//
// Do not re-tune fuel and this pair toward each other. When this was 10
// ticks (~100ms), a well-behaved guest that had consumed 2 of its
// 10,000,000 fuel was trapped on wall-clock alone -- one slow hostcall or
// a loaded machine was enough to brick a correct plugin mid-call.
const EPOCH_TICKS_PER_EXPORT: u64 = 30_000;
const EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(10);
const MAX_UI_ELEMENTS: usize = 10_000;
const MAX_ACTIONS: usize = 1_024;
const MAX_SERIALIZED_RESULT_BYTES: usize = 1024 * 1024;
// Measured from the largest host-side representations permitted by the current
// WIT result shapes: layout 2,568,576 bytes, top tabs 3,256,216 bytes, rules
// 2,535,880 bytes, and actions 1,155,072 bytes. Eight MiB leaves more than 2x
// headroom without retaining Wasmtime's unsafe 128 MiB default.
const HOSTCALL_FUEL_BYTES: usize = 8 * 1024 * 1024;
// Wasmtime 47.0.1 keeps this error type private, so classification is pinned to
// the exact static root-cause text from component/func/options.rs.
const WASMTIME_47_HOSTCALL_FUEL_EXHAUSTED: &str =
    "too much data is being copied between the host and the guest: fuel allocated for hostcalls has been exhausted";
// Wasmtime's ResourceLimiter count errors are likewise private. Keep these
// locked-version strings exact so unrelated guest errors cannot be mistaken for
// terminal quota failures or leak engine details through PluginError.
const WASMTIME_47_INSTANCE_COUNT_EXCEEDED: &str =
    "resource limit exceeded: instance count too high at 33";
const WASMTIME_47_MEMORY_COUNT_EXCEEDED: &str =
    "resource limit exceeded: memory count too high at 5";
const WASMTIME_47_TABLE_COUNT_EXCEEDED: &str = "resource limit exceeded: table count too high at 9";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaViolation {
    Result,
}

impl QuotaViolation {
    fn redacted_reason(self) -> &'static str {
        match self {
            Self::Result => "plugin result quota exceeded",
        }
    }
}

#[derive(Debug)]
enum ResultValidationError {
    Quota(QuotaViolation),
    Serialization(serde_json::Error),
}

#[derive(Default)]
struct ResultSizeWriter {
    bytes_written: usize,
    exceeded: bool,
}

impl std::io::Write for ResultSizeWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes_written.saturating_add(buffer.len()) > MAX_SERIALIZED_RESULT_BYTES {
            self.exceeded = true;
            return Err(std::io::Error::other("plugin result quota exceeded"));
        }
        self.bytes_written += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_serialized_result<T: Serialize + ?Sized>(
    value: &T,
) -> std::result::Result<(), ResultValidationError> {
    let mut writer = ResultSizeWriter::default();
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return if writer.exceeded {
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        } else {
            Err(ResultValidationError::Serialization(error))
        };
    }
    Ok(())
}

fn validate_layout_result(
    layout: &crate::types::PluginLayout,
) -> std::result::Result<(), ResultValidationError> {
    fn charge_work(
        total: &mut usize,
        additional: usize,
        limit: usize,
    ) -> std::result::Result<(), ResultValidationError> {
        *total = total
            .checked_add(additional)
            .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        if *total > limit {
            return Err(ResultValidationError::Quota(QuotaViolation::Result));
        }
        Ok(())
    }

    fn charge_elements(
        roots: &[crate::types::PluginUiElement],
        work: &mut usize,
    ) -> std::result::Result<(), ResultValidationError> {
        use crate::types::PluginUiElement;

        let mut stack = vec![roots.iter()];
        while let Some(element) = stack.last_mut().and_then(Iterator::next) {
            charge_work(work, 1, MAX_UI_ELEMENTS)?;
            match element {
                PluginUiElement::RadioGroup { options, .. }
                | PluginUiElement::Dropdown { options, .. } => {
                    charge_work(work, options.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Tabs { tabs, .. } => {
                    charge_work(work, tabs.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::ListContainer { items, .. } => stack.push(items.iter()),
                PluginUiElement::TagChips { tags, max_display } => {
                    let display_limit = max_display
                        .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX))
                        .unwrap_or(tags.len());
                    let visible = display_limit.min(tags.len());
                    let overflow_label = usize::from(visible < tags.len());
                    charge_work(work, visible, MAX_UI_ELEMENTS)?;
                    charge_work(work, overflow_label, MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Toolbar { buttons } => {
                    charge_work(work, buttons.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Carousel { images, .. } => {
                    charge_work(work, images.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::KeyValueList { items, .. }
                | PluginUiElement::MetadataGrid { items, .. } => {
                    charge_work(work, items.len(), MAX_UI_ELEMENTS)?;
                }
                _ => {}
            }

            while stack.last().is_some_and(|elements| elements.len() == 0) {
                stack.pop();
            }
        }
        Ok(())
    }

    let mut work = 0usize;
    match layout {
        crate::types::PluginLayout::Single { elements } => {
            charge_elements(elements, &mut work)?;
        }
        crate::types::PluginLayout::Split {
            sidebar, content, ..
        } => {
            charge_elements(sidebar, &mut work)?;
            charge_elements(content, &mut work)?;
        }
    }

    validate_serialized_result(layout)
}

fn validate_actions_result(
    actions: &[crate::types::PluginAction],
) -> std::result::Result<(), ResultValidationError> {
    let mut work = 0usize;
    for action in actions {
        work = work
            .checked_add(1)
            .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        if let crate::types::PluginAction::OpenLightbox { images, .. } = action {
            work = work
                .checked_add(images.len())
                .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        }
        if work > MAX_ACTIONS {
            return Err(ResultValidationError::Quota(QuotaViolation::Result));
        }
    }
    validate_serialized_result(actions)
}

fn validate_top_tabs_result(
    tabs: &[crate::types::TopTabConfig],
) -> std::result::Result<(), ResultValidationError> {
    if tabs.len() > MAX_UI_ELEMENTS {
        return Err(ResultValidationError::Quota(QuotaViolation::Result));
    }
    validate_serialized_result(tabs)
}

#[derive(Debug, Default)]
struct InstanceAvailability {
    reason: Option<&'static str>,
}

impl InstanceAvailability {
    fn ensure_available(&self) -> Result<()> {
        match self.reason {
            Some(reason) => Err(PluginError::Unavailable(reason.to_string())),
            None => Ok(()),
        }
    }

    fn mark_unavailable<T>(&mut self, reason: &'static str) -> Result<T> {
        self.reason = Some(reason);
        Err(PluginError::Unavailable(reason.to_string()))
    }

    fn reason(&self) -> Option<&'static str> {
        self.reason
    }
}

/// Classifies a wasmtime call failure as *terminal* -- the instance must
/// never be called again -- returning the redacted, host-generated
/// reason to report if so. `None` means the failure is ordinary (a guest
/// error the caller can map to `PluginError::ExecutionError` and move on
/// from; the instance stays usable).
///
/// Every `wasmtime::Trap` variant is terminal, not just `OutOfFuel`/
/// `Interrupt`: a wasmtime *component* instance is permanently poisoned
/// by any trap at all (an out-of-bounds guest panic included) -- a
/// second call into the same `Store` after, say, an unreachable-code
/// trap fails again with wasmtime's own internal "cannot enter component
/// instance" error, not a fresh attempt. Before this covered every
/// variant, only the two quota-shaped traps marked the instance
/// `Unavailable`; a genuine guest panic fell through to the generic
/// `_ => None` arm below, leaving `InstanceAvailability` reporting
/// "available" while the underlying `Store` was already permanently
/// unusable -- so the *next* call would attempt the WASM call again and
/// surface that confusing wasmtime-internal string instead of this
/// crate's own redacted, stable `PluginError::Unavailable` reason.
fn resource_quota_reason(error: &wasmtime::Error) -> Option<&'static str> {
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::OutOfFuel => Some("plugin fuel quota exceeded"),
            wasmtime::Trap::Interrupt => Some("plugin execution deadline exceeded"),
            _ => Some("plugin execution trapped"),
        };
    }

    match error.root_cause().to_string().as_str() {
        WASMTIME_47_HOSTCALL_FUEL_EXHAUSTED => {
            return Some("plugin hostcall data quota exceeded");
        }
        WASMTIME_47_INSTANCE_COUNT_EXCEEDED => {
            return Some("plugin instance quota exceeded");
        }
        WASMTIME_47_MEMORY_COUNT_EXCEEDED => {
            return Some("plugin memory quota exceeded");
        }
        WASMTIME_47_TABLE_COUNT_EXCEEDED => {
            return Some("plugin table quota exceeded");
        }
        _ => {}
    }

    error
        .downcast_ref::<crate::host_functions::StoreQuotaExceeded>()
        .map(|quota| match quota.kind {
            crate::host_functions::StoreQuotaKind::Memory => "plugin memory quota exceeded",
            crate::host_functions::StoreQuotaKind::Table => "plugin table quota exceeded",
        })
}

fn call_with_quotas<T>(
    store: &mut Store<HostFunctions>,
    availability: &mut InstanceAvailability,
    call: impl FnOnce(&mut Store<HostFunctions>) -> wasmtime::Result<T>,
    validate: impl FnOnce(&T) -> std::result::Result<(), ResultValidationError>,
    map_ordinary_error: impl FnOnce(String) -> PluginError,
) -> Result<T> {
    availability.ensure_available()?;
    if prepare_export_quota(store).is_err() {
        return availability.mark_unavailable("plugin execution quota unavailable");
    }

    let value = match call(store) {
        Ok(value) => value,
        Err(error) => {
            if let Some(reason) = resource_quota_reason(&error) {
                return availability.mark_unavailable(reason);
            }
            return Err(map_ordinary_error(error.to_string()));
        }
    };

    match validate(&value) {
        Ok(()) => {}
        Err(ResultValidationError::Quota(violation)) => {
            return availability.mark_unavailable(violation.redacted_reason());
        }
        Err(ResultValidationError::Serialization(error)) => {
            return Err(PluginError::Serialization(error));
        }
    }
    Ok(value)
}

fn prepare_export_quota(store: &mut Store<HostFunctions>) -> wasmtime::Result<()> {
    store.set_fuel(FUEL_PER_EXPORT)?;
    store.set_epoch_deadline(EPOCH_TICKS_PER_EXPORT);
    Ok(())
}

fn new_plugin_store(
    engine: &Engine,
    host_functions: HostFunctions,
) -> Result<Store<HostFunctions>> {
    let mut store = Store::new(engine, host_functions);
    store.set_hostcall_fuel(HOSTCALL_FUEL_BYTES);
    store.limiter(|host| &mut host.store_limiter);
    prepare_export_quota(&mut store)
        .map_err(|_| PluginError::WasmError("failed to configure plugin fuel quota".to_string()))?;
    store.epoch_deadline_trap();
    Ok(store)
}

struct EpochTicker {
    control: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
    #[cfg(test)]
    exited: Arc<std::sync::atomic::AtomicBool>,
}

impl EpochTicker {
    fn spawn(engine: Engine) -> std::io::Result<Self> {
        let control = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let thread_control = control.clone();
        #[cfg(test)]
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(test)]
        let thread_exited = exited.clone();
        let worker = std::thread::Builder::new()
            .name("arclain-wasm-epoch".to_string())
            .spawn(move || {
                let (stop, wake) = &*thread_control;
                let mut stopped = stop.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    let (next_stop, wait) = wake
                        .wait_timeout(stopped, EPOCH_TICK_INTERVAL)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    stopped = next_stop;
                    if *stopped {
                        break;
                    }
                    if wait.timed_out() {
                        engine.increment_epoch();
                    }
                }
                #[cfg(test)]
                thread_exited.store(true, std::sync::atomic::Ordering::Release);
            })?;

        Ok(Self {
            control,
            worker: Some(worker),
            #[cfg(test)]
            exited,
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        let (stop, wake) = &*self.control;
        *stop.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!("WASM epoch ticker terminated unexpectedly");
            }
        }
    }
}

/// WASM runtime for executing plugins
pub struct WasmRuntime {
    engine: Engine,
    epoch_ticker: Arc<EpochTicker>,
}

impl WasmRuntime {
    /// Create a new WASM runtime
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true); // Enable component model
        config.consume_fuel(true);
        config.epoch_interruption(true);

        let engine = Engine::new(&config).map_err(|e| PluginError::WasmError(e.to_string()))?;
        let epoch_ticker = Arc::new(EpochTicker::spawn(engine.clone()).map_err(|_| {
            PluginError::WasmError("failed to start WASM epoch ticker".to_string())
        })?);

        info!("WASM runtime initialized (Component Model enabled)");
        Ok(Self {
            engine,
            epoch_ticker,
        })
    }

    #[cfg(test)]
    fn epoch_ticker_exit_probe(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.epoch_ticker.exited.clone()
    }

    /// Load a WASM component from a file
    pub fn load_module(&self, id: String, path: &Path) -> Result<LoadedPlugin> {
        debug!("Loading WASM component from: {}", path.display());

        let component = Component::from_file(&self.engine, path)
            .map_err(|e| PluginError::LoadError(format!("Failed to load component: {}", e)))?;

        info!("WASM component loaded successfully: {}", path.display());

        Ok(LoadedPlugin {
            id,
            component,
            engine: self.engine.clone(),
            epoch_ticker: self.epoch_ticker.clone(),
            _path: path.to_path_buf(),
        })
    }

    /// Load a WASM component from bytes
    pub fn load_module_from_bytes(&self, id: String, bytes: &[u8]) -> Result<LoadedPlugin> {
        debug!("Loading WASM component from bytes ({} bytes)", bytes.len());

        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|e| PluginError::LoadError(e.to_string()))?;

        info!("WASM component loaded successfully from bytes");

        Ok(LoadedPlugin {
            id,
            component,
            engine: self.engine.clone(),
            epoch_ticker: self.epoch_ticker.clone(),
            _path: std::path::PathBuf::from("<bytes>"),
        })
    }
}

/// A loaded WASM plugin ready for execution
pub struct LoadedPlugin {
    pub id: String,
    component: Component,
    engine: Engine,
    epoch_ticker: Arc<EpochTicker>,
    _path: std::path::PathBuf,
}

impl LoadedPlugin {
    /// Instantiate a plugin with its host-function state.
    ///
    /// Pre-bridge this had a `_with_backend` sibling that handed an
    /// `Arc<dyn ArchiveBackend>` to the host so `list_archive_files`
    /// could re-list the archive. That path is gone: the host reads
    /// the active tab's already-listed entries through the
    /// `ActiveTabBridge`, so no plugin needs raw backend access. The
    /// backend param (and the `with_backend` constructors it fed)
    /// were dead after that rewire.
    pub fn instantiate(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        settings: HashMap<String, String>,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    ) -> Result<PluginInstance> {
        let host_funcs = HostFunctions::new(
            self.id.clone(),
            capabilities.into_iter().collect(),
            requests_per_minute,
            settings,
        )?;

        self.instantiate_with_host_functions(host_funcs, active_tab_bridge)
    }

    pub(crate) fn instantiate_with_plugin_log_dir(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        settings: HashMap<String, String>,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
        plugin_log_dir: &Path,
    ) -> Result<PluginInstance> {
        let host_funcs = HostFunctions::new_with_plugin_log_dir(
            self.id.clone(),
            capabilities.into_iter().collect(),
            requests_per_minute,
            settings,
            plugin_log_dir,
        )?;

        self.instantiate_with_host_functions(host_funcs, active_tab_bridge)
    }

    pub(crate) fn instantiate_for_metadata_validation(&self) -> Result<PluginInstance> {
        let host_funcs = HostFunctions::new_for_metadata_validation(self.id.clone())?;
        self.instantiate_with_host_functions(host_funcs, None)
    }

    fn instantiate_with_host_functions(
        &self,
        mut host_funcs: HostFunctions,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    ) -> Result<PluginInstance> {
        host_funcs.active_tab = active_tab_bridge;

        let mut store = new_plugin_store(&self.engine, host_funcs)?;
        let mut linker = Linker::new(&self.engine);

        // Link WASI functions
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Link host functions
        PluginWorld::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostFunctions| state)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Instantiate the component
        prepare_export_quota(&mut store).map_err(|_| {
            PluginError::Unavailable("plugin execution quota unavailable".to_string())
        })?;
        let plugin =
            PluginWorld::instantiate(&mut store, &self.component, &linker).map_err(|error| {
                match resource_quota_reason(&error) {
                    Some(reason) => PluginError::Unavailable(reason.to_string()),
                    None => PluginError::InitError(error.to_string()),
                }
            })?;

        debug!("Plugin instance created");

        Ok(PluginInstance {
            store,
            plugin,
            metadata: None,
            availability: InstanceAvailability::default(),
            _epoch_ticker: self.epoch_ticker.clone(),
        })
    }
}

/// An instantiated plugin that can receive events
pub struct PluginInstance {
    store: Store<HostFunctions>,
    plugin: PluginWorld,
    metadata: Option<PluginMetadata>,
    availability: InstanceAvailability,
    _epoch_ticker: Arc<EpochTicker>,
}

impl PluginInstance {
    fn call_export<T>(
        &mut self,
        call: impl FnOnce(&PluginWorld, &mut Store<HostFunctions>) -> wasmtime::Result<T>,
        validate: impl FnOnce(&T) -> std::result::Result<(), ResultValidationError>,
        map_ordinary_error: impl FnOnce(String) -> PluginError,
    ) -> Result<T> {
        let Self {
            store,
            plugin,
            availability,
            ..
        } = self;
        call_with_quotas(
            store,
            availability,
            |store| call(plugin, store),
            validate,
            map_ordinary_error,
        )
    }

    /// Return the host-generated reason this instance was made terminal.
    /// Guest-provided error details are never stored here.
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.availability.reason()
    }

    /// Initialize the plugin
    pub fn init(&mut self) -> Result<()> {
        self.call_export(
            |plugin, store| plugin.call_init(store),
            |_| Ok(()),
            PluginError::InitError,
        )?;

        debug!("Plugin initialized successfully");
        Ok(())
    }

    /// Get plugin metadata.
    ///
    /// Cached per-instance after the first WIT call. Plugins built
    /// against the post-2026-05-07 WIT export `get-metadata` and
    /// self-report id/name/version/author/description; the host caches
    /// the result so repeated calls don't re-cross the WASM boundary
    /// (the manager's `install_plugin` flow asks once at install time
    /// to derive a stable plugin id for the manifest it writes).
    pub fn get_metadata(&mut self) -> Result<PluginMetadata> {
        self.availability.ensure_available()?;
        if let Some(metadata) = &self.metadata {
            return Ok(metadata.clone());
        }

        let metadata = self.call_export(
            |plugin, store| {
                plugin
                    .call_get_metadata(store)
                    .map(|metadata| PluginMetadata {
                        id: metadata.id,
                        name: metadata.name,
                        version: metadata.version,
                        author: metadata.author,
                        description: metadata.description,
                    })
            },
            validate_serialized_result,
            PluginError::ExecutionError,
        )?;
        self.metadata = Some(metadata.clone());
        Ok(metadata)
    }

    /// Get the organization rules supplied by this plugin.
    pub fn get_default_rules(&mut self) -> Result<Vec<arclain_core::OrganizationRule>> {
        self.call_export(
            |plugin, store| {
                plugin.call_get_default_rules(store).map(|rules| {
                    rules
                        .into_iter()
                        .map(convert_wirt_rule)
                        .map(convert_plugin_rule_definition)
                        .collect()
                })
            },
            validate_serialized_result,
            PluginError::ExecutionError,
        )
    }

    /// Get UI layout for a specific extension point
    pub fn get_ui_layout(
        &mut self,
        extension_point: PluginExtensionPoint,
    ) -> Result<crate::types::PluginLayout> {
        let ep_str = match extension_point {
            PluginExtensionPoint::MainPage => "MainPage".to_string(),
            PluginExtensionPoint::PluginButton => "PluginButton".to_string(),
            PluginExtensionPoint::Panel => "Panel".to_string(),
            PluginExtensionPoint::Dialog(ref id) => format!("Dialog:{}", id),
            PluginExtensionPoint::Page(ref id) => format!("Page:{}", id),
        };

        self.call_export(
            |plugin, store| {
                plugin
                    .call_get_ui_layout(store, &ep_str)
                    .map(convert_plugin_layout)
            },
            validate_layout_result,
            PluginError::ExecutionError,
        )
    }

    /// Send a UI event to the plugin and get actions back
    pub fn send_ui_event(
        &mut self,
        element_id: &str,
        value: Option<String>,
    ) -> Result<Vec<crate::types::PluginAction>> {
        debug!(
            "PluginInstance::send_ui_event: Calling plugin handler for {}",
            element_id
        );

        let actions = self.call_export::<Vec<crate::types::PluginAction>>(
            |plugin, store| {
                plugin
                    .call_on_ui_event(store, element_id, value.as_deref())
                    .map(|actions| {
                        actions
                            .into_iter()
                            .map(convert_plugin_action)
                            .collect::<Vec<_>>()
                    })
            },
            |actions| validate_actions_result(actions),
            |message| {
                error!(
                    "PluginInstance::send_ui_event: Failed to call plugin: {}",
                    message
                );
                PluginError::ExecutionError(message)
            },
        )?;

        debug!(
            "PluginInstance::send_ui_event: Plugin returned {} actions",
            actions.len()
        );

        Ok(actions)
    }

    /// Clean up the plugin
    pub fn cleanup(&mut self) -> Result<()> {
        // Cleanup is handled by Drop in Component Model usually,
        // or we can add a specific cleanup function to WIT
        Ok(())
    }

    /// Set the content cache for host functions
    pub fn set_content_cache(&mut self, cache: Option<Arc<arclain_data::ContentCache>>) {
        let host = self.store.data_mut();
        match cache {
            Some(c) => host.set_content_cache(c),
            None => host.content_cache = None,
        }
    }

    /// Set the resource manager for host functions
    pub fn set_resource_manager(&mut self, manager: Option<Arc<arclain_data::ResourceManager>>) {
        let host = self.store.data_mut();
        match manager {
            Some(m) => host.set_resource_manager(m),
            None => host.resource_manager = None,
        }
    }

    /// Install the bridge to the host's per-tab signal tree for this
    /// instance. Replaces the pre-bridge `set_metadata_signal` +
    /// `set_archive_context` pair — see `crate::active_tab`.
    pub fn set_active_tab_bridge(&mut self, bridge: Arc<dyn crate::ActiveTabBridge>) {
        self.store.data_mut().set_active_tab_bridge(bridge);
    }

    /// Install (or clear) the per-event context for this instance.
    /// The dispatch worker wraps `send_ui_event` with set / clear
    /// calls so host-function reads inside the handler resolve to
    /// the originating tab.
    pub fn set_event_context(&mut self, ctx: Option<crate::host_functions::EventContext>) {
        self.store.data_mut().set_event_context(ctx);
    }

    #[cfg(test)]
    pub(crate) fn has_event_context_for_test(&self) -> bool {
        self.store.data().event_context.is_some()
    }

    /// Set the async HTTP client for host functions
    pub fn set_async_http_client(&mut self, client: Option<Arc<arclain_network::AsyncHttpClient>>) {
        let host = self.store.data_mut();
        match client {
            Some(c) => host.set_async_http_client(c),
            None => host.async_http_client = None,
        }
    }

    /// Set the library service for host functions
    #[cfg(feature = "gameta")]
    pub fn set_library_service(&mut self, lib_svc: Option<Arc<arclain_core::LibraryService>>) {
        let host = self.store.data_mut();
        match lib_svc {
            Some(c) => host.set_library_service(c),
            None => host.library_service = None,
        }
    }

    /// Set the gameta server client for host functions
    pub fn set_gameta_client(
        &mut self,
        client: Option<Arc<arclain_network::features::gameta_client::GametaClient>>,
    ) {
        let host = self.store.data_mut();
        match client {
            Some(c) => host.set_gameta_client(c),
            None => host.gameta_client = None,
        }
    }

    /// Get gameta client reference (if configured)
    pub fn get_gameta_client(
        &self,
    ) -> Option<Arc<arclain_network::features::gameta_client::GametaClient>> {
        let data = self.store.data();
        data.gameta_client.clone()
    }

    pub fn try_acquire_network_host_service(
        &self,
        service_scope: &str,
    ) -> std::result::Result<(), String> {
        let data = self.store.data();
        let client = data
            .async_http_client
            .as_ref()
            .ok_or_else(|| "plugin network policy client unavailable".to_string())?;
        client
            .try_acquire_plugin_host_service(data.plugin_id.as_str(), service_scope)
            .map_err(|error| error.to_string())
    }

    pub fn data_materialization_limit(&self) -> usize {
        self.store.data().data_service.materialization_limit()
    }

    /// Check the immutable manifest capabilities attached to this instance.
    pub fn has_capabilities(&self, required: &[PluginCapability]) -> bool {
        self.store.data().has_capabilities(required)
    }

    /// Get a handle to the active-tab bridge if one has been
    /// installed. Used by the dispatch worker to snapshot per-tab
    /// signals at event-fire time.
    pub fn get_active_tab_bridge(&self) -> Option<Arc<dyn crate::ActiveTabBridge>> {
        let data = self.store.data();
        data.active_tab.clone()
    }

    /// Get network logs from the plugin
    pub fn get_network_log(&self) -> Vec<(std::time::SystemTime, String)> {
        let data = self.store.data();
        let logs = data.network_log.lock();
        logs.clone()
    }

    /// Get current settings from the plugin
    pub fn get_settings(&self) -> Option<std::collections::HashMap<String, String>> {
        let data = self.store.data();
        let settings = data.settings.lock();
        Some(settings.clone())
    }

    /// Cheap clone of the plugin's `settings_dirty` flag — checked by
    /// `PluginManager::get_all_settings` without taking the instance
    /// lock (audit P14).
    pub fn settings_dirty_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.store.data().settings_dirty.clone()
    }

    /// Get top-level tabs registered by this plugin
    pub fn get_top_tabs(&mut self) -> Result<Vec<crate::types::TopTabConfig>> {
        self.call_export(
            |plugin, store| {
                plugin.call_get_top_tabs(store).map(|tabs| {
                    tabs.into_iter()
                        .map(convert_top_tab_config)
                        .collect::<Vec<_>>()
                })
            },
            |tabs| validate_top_tabs_result(tabs),
            PluginError::ExecutionError,
        )
    }
}
