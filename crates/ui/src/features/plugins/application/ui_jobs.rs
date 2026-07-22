//! Background coordinator for plugin UI reads and mutations.

use crate::core::tabs::TabId;
use crate::features::plugins::domain::types::{PluginInfo, PluginsListState, RequestId};
use arclain_core::UserConfig;
use arclain_plugins::host_functions::EventContext;
use arclain_plugins::manager::PluginStatusSummary;
use arclain_plugins::types::{PluginAction, PluginExtensionPoint, PluginLayout, TopTabConfig};
use arclain_plugins::PluginManager;
use arclain_signals::Signal;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

const NETWORK_LOG_TTL: Duration = Duration::from_secs(1);
type OriginContextProvider = Arc<dyn Fn(TabId) -> Option<EventContext> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PluginUiTarget {
    MainPage,
    PluginButton,
    Panel,
    Dialog(String),
    Page(String),
}

impl PluginUiTarget {
    fn extension_point(&self) -> PluginExtensionPoint {
        match self {
            Self::MainPage => PluginExtensionPoint::MainPage,
            Self::PluginButton => PluginExtensionPoint::PluginButton,
            Self::Panel => PluginExtensionPoint::Panel,
            Self::Dialog(id) => PluginExtensionPoint::Dialog(id.clone()),
            Self::Page(id) => PluginExtensionPoint::Page(id.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum PluginUiRequest {
    PageInit {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
    },
    Layout {
        plugin_id: String,
        target: PluginUiTarget,
        origin_tab: Option<TabId>,
    },
    Snapshot {
        user_config: UserConfig,
    },
    ChromeSnapshot,
    NetworkLog,
    SetEnabled {
        plugin_id: String,
        enabled: bool,
    },
    Install {
        wasm_path: PathBuf,
    },
    UiEvent {
        plugin_id: String,
        event_id: String,
        value: Option<String>,
        origin_tab: TabId,
    },
}

#[derive(Clone, Debug)]
pub struct PluginUiEventCompletion {
    pub actions: Vec<PluginAction>,
    pub settings: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginUiFailureContext {
    PageInit {
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
    },
    Layout {
        plugin_id: String,
        target: PluginUiTarget,
        origin_tab: Option<TabId>,
    },
    Snapshot {
        visibility: Option<String>,
    },
    ChromeSnapshot,
    NetworkLog,
    SetEnabled {
        plugin_id: String,
        enabled: bool,
    },
    Install {
        wasm_path: PathBuf,
    },
    UiEvent {
        plugin_id: String,
        event_id: String,
        origin_tab: TabId,
    },
}

#[derive(Clone, Debug)]
pub enum PluginUiResult {
    PageInitialized {
        request_id: RequestId,
        plugin_id: String,
        page_id: String,
        origin_tab: TabId,
        actions: std::result::Result<Vec<PluginAction>, String>,
    },
    LayoutLoaded {
        request_id: RequestId,
        target: PluginUiTarget,
        layout: std::result::Result<PluginLayout, String>,
    },
    SnapshotLoaded {
        request_id: RequestId,
        plugins: Vec<PluginInfo>,
    },
    ChromeSnapshotLoaded {
        request_id: RequestId,
        summary: PluginStatusSummary,
        top_tabs: Vec<(String, TopTabConfig)>,
    },
    NetworkLogLoaded {
        request_id: RequestId,
        entries: Vec<(SystemTime, String)>,
    },
    MutationFinished {
        request_id: RequestId,
        result: std::result::Result<(), String>,
    },
    UiEventFinished {
        request_id: RequestId,
        plugin_id: String,
        origin_tab: TabId,
        result: std::result::Result<PluginUiEventCompletion, String>,
    },
    Failed {
        request_id: RequestId,
        context: PluginUiFailureContext,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RequestKey {
    PageInit(RequestId, String, String, TabId),
    Layout(String, PluginUiTarget, Option<TabId>),
    Snapshot(Option<String>),
    ChromeSnapshot,
    NetworkLog,
    SetEnabled(RequestId, String, bool),
    Install(RequestId, PathBuf),
    UiEvent(RequestId, String, String, TabId),
}

impl PluginUiRequest {
    fn key(&self, request_id: RequestId) -> RequestKey {
        match self {
            Self::PageInit {
                plugin_id,
                page_id,
                origin_tab,
            } => RequestKey::PageInit(request_id, plugin_id.clone(), page_id.clone(), *origin_tab),
            Self::Layout {
                plugin_id,
                target,
                origin_tab,
            } => RequestKey::Layout(plugin_id.clone(), target.clone(), *origin_tab),
            Self::Snapshot { user_config } => {
                RequestKey::Snapshot(user_config.plugin_visibility.clone())
            }
            Self::ChromeSnapshot => RequestKey::ChromeSnapshot,
            Self::NetworkLog => RequestKey::NetworkLog,
            Self::SetEnabled { plugin_id, enabled } => {
                RequestKey::SetEnabled(request_id, plugin_id.clone(), *enabled)
            }
            Self::Install { wasm_path } => RequestKey::Install(request_id, wasm_path.clone()),
            Self::UiEvent {
                plugin_id,
                event_id,
                origin_tab,
                ..
            } => RequestKey::UiEvent(request_id, plugin_id.clone(), event_id.clone(), *origin_tab),
        }
    }
}

impl RequestKey {
    fn failure_context(&self) -> PluginUiFailureContext {
        match self {
            Self::PageInit(_, plugin_id, page_id, origin_tab) => PluginUiFailureContext::PageInit {
                plugin_id: plugin_id.clone(),
                page_id: page_id.clone(),
                origin_tab: *origin_tab,
            },
            Self::Layout(plugin_id, target, origin_tab) => PluginUiFailureContext::Layout {
                plugin_id: plugin_id.clone(),
                target: target.clone(),
                origin_tab: *origin_tab,
            },
            Self::Snapshot(visibility) => PluginUiFailureContext::Snapshot {
                visibility: visibility.clone(),
            },
            Self::ChromeSnapshot => PluginUiFailureContext::ChromeSnapshot,
            Self::NetworkLog => PluginUiFailureContext::NetworkLog,
            Self::SetEnabled(_, plugin_id, enabled) => PluginUiFailureContext::SetEnabled {
                plugin_id: plugin_id.clone(),
                enabled: *enabled,
            },
            Self::Install(_, wasm_path) => PluginUiFailureContext::Install {
                wasm_path: wasm_path.clone(),
            },
            Self::UiEvent(_, plugin_id, event_id, origin_tab) => PluginUiFailureContext::UiEvent {
                plugin_id: plugin_id.clone(),
                event_id: event_id.clone(),
                origin_tab: *origin_tab,
            },
        }
    }
}

impl PluginUiFailureContext {
    fn operation_name(&self) -> &'static str {
        match self {
            Self::PageInit { .. } => "page init",
            Self::Layout { .. } => "layout",
            Self::Snapshot { .. } => "plugin snapshot",
            Self::ChromeSnapshot => "chrome snapshot",
            Self::NetworkLog => "network log",
            Self::SetEnabled { .. } => "set enabled",
            Self::Install { .. } => "install",
            Self::UiEvent { .. } => "UI event",
        }
    }
}

struct Completed {
    key: RequestKey,
    result: PluginUiResult,
}

struct QueuedJob {
    key: RequestKey,
    request_id: RequestId,
    request: PluginUiRequest,
    origin_context: Option<EventContext>,
}

#[derive(Clone, Debug)]
pub(crate) enum PluginUiMutation {
    SetEnabled { plugin_id: String, enabled: bool },
    Install,
}

#[derive(Default)]
struct PluginUiCache {
    layouts: HashMap<
        (String, PluginUiTarget, Option<TabId>),
        std::result::Result<Arc<PluginLayout>, Arc<str>>,
    >,
    snapshots: HashMap<Option<String>, std::result::Result<Arc<Vec<PluginInfo>>, Arc<str>>>,
    chrome: Option<
        std::result::Result<(PluginStatusSummary, Arc<Vec<(String, TopTabConfig)>>), Arc<str>>,
    >,
    network_log: Option<(Instant, Arc<Vec<(SystemTime, String)>>)>,
    network_log_failure: Option<Arc<str>>,
    completed_mutations: HashMap<RequestId, PluginUiMutation>,
}

#[derive(Clone)]
pub struct PluginUiJobs {
    manager: Option<Arc<Mutex<PluginManager>>>,
    runtime: Arc<tokio::runtime::Runtime>,
    pending: Arc<Mutex<HashMap<RequestKey, RequestId>>>,
    sender: mpsc::Sender<Completed>,
    receiver: Arc<Mutex<mpsc::Receiver<Completed>>>,
    cache: Arc<Mutex<PluginUiCache>>,
    completion_epoch: Signal<u64>,
    ordered_sender: mpsc::Sender<QueuedJob>,
    origin_context_provider: Option<OriginContextProvider>,
}

impl PluginUiJobs {
    pub fn new(
        manager: Option<Arc<Mutex<PluginManager>>>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (ordered_sender, ordered_receiver) = mpsc::channel::<QueuedJob>();
        let ordered_manager = manager.clone();
        let ordered_result_sender = sender.clone();
        let ordered_epoch = Signal::new(0).with_name("plugin_ui_completion_epoch");
        let worker_epoch = ordered_epoch.clone();
        std::thread::Builder::new()
            .name("plugin-ui-ordered-jobs".to_string())
            .spawn(move || {
                while let Ok(job) = ordered_receiver.recv() {
                    let completed = execute_job(ordered_manager.clone(), job);
                    publish_completed(&ordered_result_sender, &worker_epoch, completed);
                }
            })
            .expect("failed to start plugin UI ordered worker");

        Self {
            manager,
            runtime,
            pending: Arc::new(Mutex::new(HashMap::new())),
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            cache: Arc::new(Mutex::new(PluginUiCache::default())),
            completion_epoch: ordered_epoch,
            ordered_sender,
            origin_context_provider: None,
        }
    }

    /// Attach a resolver that snapshots the originating tab's archive
    /// context when work is queued. WASM host calls then remain pinned to
    /// that tab even if the user switches tabs before the worker runs.
    pub fn with_origin_context_provider<F>(mut self, provider: F) -> Self
    where
        F: Fn(TabId) -> Option<EventContext> + Send + Sync + 'static,
    {
        self.origin_context_provider = Some(Arc::new(provider));
        self
    }

    pub fn request(&self, request: PluginUiRequest) -> RequestId {
        self.request_with_id(RequestId::next(), request)
    }

    pub(crate) fn request_with_id(
        &self,
        request_id: RequestId,
        request: PluginUiRequest,
    ) -> RequestId {
        let key = request.key(request_id);
        {
            let mut pending = self.pending.lock();
            if let Some(existing) = pending.get(&key) {
                return *existing;
            }
            pending.insert(key.clone(), request_id);
        }

        let origin_tab = match &request {
            PluginUiRequest::PageInit { origin_tab, .. } => Some(*origin_tab),
            PluginUiRequest::Layout { origin_tab, .. } => *origin_tab,
            PluginUiRequest::UiEvent { origin_tab, .. } => Some(*origin_tab),
            _ => None,
        };
        let origin_context = origin_tab.and_then(|tab_id| {
            self.origin_context_provider
                .as_ref()
                .and_then(|provider| provider(tab_id))
        });
        let job = QueuedJob {
            key,
            request_id,
            request,
            origin_context,
        };

        if matches!(
            &job.request,
            PluginUiRequest::PageInit { .. }
                | PluginUiRequest::SetEnabled { .. }
                | PluginUiRequest::Install { .. }
                | PluginUiRequest::UiEvent { .. }
        ) {
            if let Err(mpsc::SendError(job)) = self.ordered_sender.send(job) {
                let completed = failed_completed(
                    job,
                    "plugin UI ordered worker channel is unavailable".to_string(),
                );
                publish_completed(&self.sender, &self.completion_epoch, completed);
            }
            return request_id;
        }

        let manager = self.manager.clone();
        let sender = self.sender.clone();
        let completion_epoch = self.completion_epoch.clone();
        self.runtime.spawn(async move {
            let fallback_key = job.key.clone();
            let fallback_request_id = job.request_id;
            let completed = tokio::task::spawn_blocking(move || execute_job(manager, job)).await;
            let completed = completed.unwrap_or_else(|error| {
                failed_completed_parts(
                    fallback_key,
                    fallback_request_id,
                    format!("plugin UI worker failed: {error}"),
                )
            });
            publish_completed(&sender, &completion_epoch, completed);
        });
        request_id
    }

    pub fn completion_signal(&self) -> &Signal<u64> {
        &self.completion_epoch
    }

    pub fn drain(&self) -> Vec<PluginUiResult> {
        let mut results = Vec::new();
        let receiver = self.receiver.lock();
        while let Ok(completed) = receiver.try_recv() {
            let request_id = result_request_id(&completed.result);
            let mut pending = self.pending.lock();
            if pending.get(&completed.key) == Some(&request_id) {
                pending.remove(&completed.key);
                drop(pending);
                self.cache_completed(&completed);
                results.push(completed.result);
            }
        }
        results
    }

    pub fn layout(
        &self,
        plugin_id: &str,
        target: PluginUiTarget,
        origin_tab: Option<TabId>,
    ) -> Option<std::result::Result<Arc<PluginLayout>, Arc<str>>> {
        let cache_key = (plugin_id.to_string(), target.clone(), origin_tab);
        if let Some(layout) = self.cache.lock().layouts.get(&cache_key).cloned() {
            return Some(layout);
        }
        self.request(PluginUiRequest::Layout {
            plugin_id: plugin_id.to_string(),
            target,
            origin_tab,
        });
        None
    }

    pub fn invalidate_layout(
        &self,
        plugin_id: &str,
        target: &PluginUiTarget,
        origin_tab: Option<TabId>,
    ) {
        let key = RequestKey::Layout(plugin_id.to_string(), target.clone(), origin_tab);
        self.cache
            .lock()
            .layouts
            .remove(&(plugin_id.to_string(), target.clone(), origin_tab));
        self.pending.lock().remove(&key);
    }

    pub fn invalidate_all_layouts(&self) {
        self.cache.lock().layouts.clear();
        self.pending
            .lock()
            .retain(|key, _| !matches!(key, RequestKey::Layout(..)));
    }

    pub fn plugin_snapshot(
        &self,
        user_config: &UserConfig,
    ) -> Option<std::result::Result<Arc<Vec<PluginInfo>>, Arc<str>>> {
        let key = user_config.plugin_visibility.clone();
        let snapshot = self.cache.lock().snapshots.get(&key).cloned();
        if snapshot.is_none() {
            self.request(PluginUiRequest::Snapshot {
                user_config: user_config.clone(),
            });
        }
        snapshot
    }

    pub fn invalidate_plugin_snapshots(&self) {
        self.cache.lock().snapshots.clear();
        self.pending
            .lock()
            .retain(|key, _| !matches!(key, RequestKey::Snapshot(_)));
    }

    pub fn chrome_snapshot(
        &self,
    ) -> Option<
        std::result::Result<(PluginStatusSummary, Arc<Vec<(String, TopTabConfig)>>), Arc<str>>,
    > {
        let snapshot = self.cache.lock().chrome.clone();
        if snapshot.is_none() {
            self.request(PluginUiRequest::ChromeSnapshot);
        }
        snapshot
    }

    pub fn invalidate_chrome_snapshot(&self) {
        self.cache.lock().chrome = None;
        self.pending.lock().remove(&RequestKey::ChromeSnapshot);
    }

    pub fn network_log(
        &self,
    ) -> Option<std::result::Result<Arc<Vec<(SystemTime, String)>>, Arc<str>>> {
        self.network_log_at(Instant::now())
    }

    fn network_log_at(
        &self,
        now: Instant,
    ) -> Option<std::result::Result<Arc<Vec<(SystemTime, String)>>, Arc<str>>> {
        let cache = self.cache.lock();
        if let Some(error) = cache.network_log_failure.clone() {
            return Some(Err(error));
        }
        let cached = cache.network_log.clone();
        drop(cache);
        if let Some((fetched_at, entries)) = cached {
            if now.saturating_duration_since(fetched_at) < NETWORK_LOG_TTL {
                return Some(Ok(entries));
            }
            self.invalidate_network_log();
        }
        self.request(PluginUiRequest::NetworkLog);
        None
    }

    pub fn invalidate_network_log(&self) {
        let mut cache = self.cache.lock();
        cache.network_log = None;
        cache.network_log_failure = None;
        drop(cache);
        self.pending.lock().remove(&RequestKey::NetworkLog);
    }

    pub(crate) fn take_mutation(&self, request_id: RequestId) -> Option<PluginUiMutation> {
        self.cache.lock().completed_mutations.remove(&request_id)
    }

    fn cache_completed(&self, completed: &Completed) {
        let mut cache = self.cache.lock();
        match (&completed.key, &completed.result) {
            (
                RequestKey::Layout(plugin_id, target, origin_tab),
                PluginUiResult::LayoutLoaded { layout, .. },
            ) => {
                cache.layouts.insert(
                    (plugin_id.clone(), target.clone(), *origin_tab),
                    layout
                        .as_ref()
                        .map(|layout| Arc::new(layout.clone()))
                        .map_err(|error| Arc::<str>::from(error.as_str())),
                );
            }
            (
                RequestKey::ChromeSnapshot,
                PluginUiResult::ChromeSnapshotLoaded {
                    summary, top_tabs, ..
                },
            ) => {
                cache.chrome = Some(Ok((*summary, Arc::new(top_tabs.clone()))));
            }
            (RequestKey::Snapshot(visibility), PluginUiResult::SnapshotLoaded { plugins, .. }) => {
                cache
                    .snapshots
                    .insert(visibility.clone(), Ok(Arc::new(plugins.clone())));
            }
            (RequestKey::NetworkLog, PluginUiResult::NetworkLogLoaded { entries, .. }) => {
                cache.network_log = Some((Instant::now(), Arc::new(entries.clone())));
                cache.network_log_failure = None;
            }
            (
                RequestKey::SetEnabled(_, plugin_id, enabled),
                PluginUiResult::MutationFinished { request_id, .. },
            ) => {
                cache.completed_mutations.insert(
                    *request_id,
                    PluginUiMutation::SetEnabled {
                        plugin_id: plugin_id.clone(),
                        enabled: *enabled,
                    },
                );
            }
            (RequestKey::Install(_, _), PluginUiResult::MutationFinished { request_id, .. }) => {
                cache
                    .completed_mutations
                    .insert(*request_id, PluginUiMutation::Install);
            }
            (_, PluginUiResult::Failed { context, error, .. }) => {
                let error = Arc::<str>::from(error.as_str());
                match context {
                    PluginUiFailureContext::Layout {
                        plugin_id,
                        target,
                        origin_tab,
                    } => {
                        cache
                            .layouts
                            .insert((plugin_id.clone(), target.clone(), *origin_tab), Err(error));
                    }
                    PluginUiFailureContext::Snapshot { visibility } => {
                        cache.snapshots.insert(visibility.clone(), Err(error));
                    }
                    PluginUiFailureContext::ChromeSnapshot => {
                        cache.chrome = Some(Err(error));
                    }
                    PluginUiFailureContext::NetworkLog => {
                        cache.network_log = None;
                        cache.network_log_failure = Some(error);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn result_request_id(result: &PluginUiResult) -> RequestId {
    match result {
        PluginUiResult::PageInitialized { request_id, .. }
        | PluginUiResult::LayoutLoaded { request_id, .. }
        | PluginUiResult::SnapshotLoaded { request_id, .. }
        | PluginUiResult::ChromeSnapshotLoaded { request_id, .. }
        | PluginUiResult::NetworkLogLoaded { request_id, .. }
        | PluginUiResult::MutationFinished { request_id, .. }
        | PluginUiResult::UiEventFinished { request_id, .. }
        | PluginUiResult::Failed { request_id, .. } => *request_id,
    }
}

fn publish_completed(
    sender: &mpsc::Sender<Completed>,
    completion_epoch: &Signal<u64>,
    completed: Completed,
) {
    if sender.send(completed).is_ok() {
        completion_epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
    }
}

fn execute_job(manager: Option<Arc<Mutex<PluginManager>>>, job: QueuedJob) -> Completed {
    let QueuedJob {
        key,
        request_id,
        request,
        origin_context,
    } = job;
    let context = key.failure_context();
    let result = catch_worker_failure(request_id, context, || {
        execute(manager, request_id, request, origin_context)
    });
    Completed { key, result }
}

fn catch_worker_failure(
    request_id: RequestId,
    context: PluginUiFailureContext,
    operation: impl FnOnce() -> PluginUiResult,
) -> PluginUiResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).unwrap_or_else(|panic| {
        PluginUiResult::Failed {
            request_id,
            context,
            error: panic_message(panic),
        }
    })
}

fn failed_completed(job: QueuedJob, error: String) -> Completed {
    failed_completed_parts(job.key, job.request_id, error)
}

fn failed_completed_parts(key: RequestKey, request_id: RequestId, error: String) -> Completed {
    let context = key.failure_context();
    let error = format!("{}: {error}", context.operation_name());
    Completed {
        key,
        result: PluginUiResult::Failed {
            request_id,
            context,
            error,
        },
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    format!("plugin UI worker panicked: {detail}")
}

fn with_event_context<R>(
    instance: &mut arclain_plugins::PluginInstance,
    context: Option<EventContext>,
    operation: impl FnOnce(&mut arclain_plugins::PluginInstance) -> R,
) -> R {
    instance.set_event_context(context);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(instance)));
    instance.set_event_context(None);
    match result {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn execute(
    manager: Option<Arc<Mutex<PluginManager>>>,
    request_id: RequestId,
    request: PluginUiRequest,
    origin_context: Option<EventContext>,
) -> PluginUiResult {
    match request {
        PluginUiRequest::PageInit {
            plugin_id,
            page_id,
            origin_tab,
        } => {
            let actions = manager
                .ok_or_else(|| "plugin manager unavailable".to_string())
                .and_then(|manager| {
                    let instance = manager
                        .lock()
                        .get_plugin_instance(&plugin_id)
                        .ok_or_else(|| format!("plugin not found: {plugin_id}"))?;
                    let mut instance = instance.lock();
                    with_event_context(&mut instance, origin_context, |instance| {
                        instance
                            .send_ui_event("__page_init", Some(page_id.clone()))
                            .map_err(|error| error.to_string())
                    })
                });
            PluginUiResult::PageInitialized {
                request_id,
                plugin_id,
                page_id,
                origin_tab,
                actions,
            }
        }
        PluginUiRequest::Layout {
            plugin_id, target, ..
        } => {
            let layout = manager
                .ok_or_else(|| "plugin manager unavailable".to_string())
                .and_then(|manager| {
                    let instance = {
                        let manager = manager.lock();
                        manager.get_plugin_instance(&plugin_id)
                    };
                    instance.ok_or_else(|| format!("plugin not found: {plugin_id}"))
                })
                .and_then(|instance| {
                    let mut instance = instance.lock();
                    with_event_context(&mut instance, origin_context, |instance| {
                        instance
                            .get_ui_layout(target.extension_point())
                            .map_err(|error| error.to_string())
                    })
                });
            PluginUiResult::LayoutLoaded {
                request_id,
                target,
                layout,
            }
        }
        PluginUiRequest::Snapshot { user_config } => {
            let Some(manager) = manager else {
                return PluginUiResult::Failed {
                    request_id,
                    context: PluginUiFailureContext::Snapshot {
                        visibility: user_config.plugin_visibility.clone(),
                    },
                    error: "plugin manager unavailable".to_string(),
                };
            };
            let plugins = {
                let manager = manager.lock();
                let mut state = PluginsListState::default();
                state.update_from_manager(&manager, &user_config);
                state.plugins
            };
            PluginUiResult::SnapshotLoaded {
                request_id,
                plugins,
            }
        }
        PluginUiRequest::ChromeSnapshot => {
            let Some(manager) = manager else {
                return PluginUiResult::Failed {
                    request_id,
                    context: PluginUiFailureContext::ChromeSnapshot,
                    error: "plugin manager unavailable".to_string(),
                };
            };
            let (summary, instances) = {
                let manager = manager.lock();
                (manager.status_summary(), manager.enabled_plugin_snapshot())
            };
            let top_tabs = instances.get_all_top_tabs();
            PluginUiResult::ChromeSnapshotLoaded {
                request_id,
                summary,
                top_tabs,
            }
        }
        PluginUiRequest::NetworkLog => {
            let Some(manager) = manager else {
                return PluginUiResult::Failed {
                    request_id,
                    context: PluginUiFailureContext::NetworkLog,
                    error: "plugin manager unavailable".to_string(),
                };
            };
            let instances = { manager.lock().enabled_plugin_snapshot() };
            let entries = instances.get_network_log();
            PluginUiResult::NetworkLogLoaded {
                request_id,
                entries,
            }
        }
        PluginUiRequest::SetEnabled { plugin_id, enabled } => {
            let result = manager
                .ok_or_else(|| "plugin manager unavailable".to_string())
                .and_then(|manager| {
                    let manager = manager.lock();
                    let result = if enabled {
                        manager.enable_plugin(&plugin_id)
                    } else {
                        manager.disable_plugin(&plugin_id)
                    };
                    result.map_err(|error| error.to_string())
                });
            PluginUiResult::MutationFinished { request_id, result }
        }
        PluginUiRequest::Install { wasm_path } => {
            let result = manager
                .ok_or_else(|| "plugin manager unavailable".to_string())
                .and_then(|manager| {
                    manager
                        .lock()
                        .install_plugin(&wasm_path)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                });
            PluginUiResult::MutationFinished { request_id, result }
        }
        PluginUiRequest::UiEvent {
            plugin_id,
            event_id,
            value,
            origin_tab,
        } => {
            let result = manager
                .ok_or_else(|| "plugin manager unavailable".to_string())
                .and_then(|manager| {
                    let instance = {
                        let manager = manager.lock();
                        manager.get_plugin_instance(&plugin_id)
                    };
                    let instance =
                        instance.ok_or_else(|| format!("plugin not found: {plugin_id}"))?;
                    let actions = {
                        let mut instance = instance.lock();
                        with_event_context(&mut instance, origin_context, |instance| {
                            instance
                                .send_ui_event(&event_id, value)
                                .map_err(|error| error.to_string())
                        })?
                    };
                    let settings = manager.lock().get_settings_for(&plugin_id);
                    Ok(PluginUiEventCompletion { actions, settings })
                });
            PluginUiResult::UiEventFinished {
                request_id,
                plugin_id,
                origin_tab,
                result,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_jobs() -> PluginUiJobs {
        PluginUiJobs::new(
            None,
            Arc::new(tokio::runtime::Runtime::new().expect("create runtime")),
        )
    }

    #[test]
    fn expired_network_log_is_revoked_and_refetched() {
        let jobs = test_jobs();
        let fetched_at = Instant::now();
        let entries = Arc::new(vec![(SystemTime::UNIX_EPOCH, "request".to_string())]);
        jobs.cache.lock().network_log = Some((fetched_at, entries.clone()));

        let cached = jobs
            .network_log_at(fetched_at + NETWORK_LOG_TTL - Duration::from_millis(1))
            .expect("fresh log must remain cached")
            .expect("fresh log must be successful");
        assert!(Arc::ptr_eq(&cached, &entries));

        assert!(
            jobs.network_log_at(fetched_at + NETWORK_LOG_TTL).is_none(),
            "expired logs must not be rendered indefinitely"
        );
        assert!(jobs.pending.lock().contains_key(&RequestKey::NetworkLog));
    }

    #[test]
    fn worker_panic_becomes_an_explicit_failure_result() {
        let request_id = RequestId(42);
        let result =
            catch_worker_failure(request_id, PluginUiFailureContext::ChromeSnapshot, || {
                panic!("fixture panic")
            });

        assert!(matches!(
            result,
            PluginUiResult::Failed { request_id: id, error, .. }
                if id == request_id && error.contains("fixture panic")
        ));
    }

    #[test]
    fn layout_request_snapshots_its_explicit_origin_tab() {
        let observed = Arc::new(AtomicU64::new(0));
        let observed_by_provider = observed.clone();
        let jobs = test_jobs().with_origin_context_provider(move |tab_id| {
            observed_by_provider.store(tab_id.0, Ordering::SeqCst);
            None
        });

        jobs.request(PluginUiRequest::Layout {
            plugin_id: "plugin".to_string(),
            target: PluginUiTarget::Panel,
            origin_tab: Some(TabId(73)),
        });

        assert_eq!(observed.load(Ordering::SeqCst), 73);
    }

    #[test]
    fn ui_event_request_snapshots_its_explicit_origin_tab() {
        let observed = Arc::new(AtomicU64::new(0));
        let observed_by_provider = observed.clone();
        let jobs = test_jobs().with_origin_context_provider(move |tab_id| {
            observed_by_provider.store(tab_id.0, Ordering::SeqCst);
            None
        });

        jobs.request(PluginUiRequest::UiEvent {
            plugin_id: "plugin".to_string(),
            event_id: "clicked".to_string(),
            value: None,
            origin_tab: TabId(91),
        });

        assert_eq!(observed.load(Ordering::SeqCst), 91);
    }

    #[test]
    fn disconnected_ordered_worker_publishes_a_contextual_page_init_failure() {
        let mut jobs = test_jobs();
        let (disconnected_sender, disconnected_receiver) = mpsc::channel();
        drop(disconnected_receiver);
        jobs.ordered_sender = disconnected_sender;
        let mut page_state = crate::features::plugins::domain::state::PluginDialogState::default();
        let request_id = page_state.open_page("plugin", "page", TabId(7));

        jobs.request_with_id(
            request_id,
            PluginUiRequest::PageInit {
                plugin_id: "plugin".to_string(),
                page_id: "page".to_string(),
                origin_tab: TabId(7),
            },
        );

        let results = jobs.drain();
        assert_eq!(results.len(), 1, "channel failure must publish a result");
        assert!(matches!(
            &results[0],
            PluginUiResult::Failed {
                request_id: id,
                context: PluginUiFailureContext::PageInit {
                    plugin_id,
                    page_id,
                    origin_tab,
                },
                error,
            }
                if *id == request_id
                    && plugin_id == "plugin"
                    && page_id == "page"
                    && *origin_tab == TabId(7)
                    && error.contains("ordered worker")
                    && error.contains("page init")
        ));
        let PluginUiResult::Failed { error, .. } = &results[0] else {
            unreachable!();
        };
        assert!(page_state.apply_page_init_failure(request_id, error.clone()));
        assert!(!page_state.page_init_pending());
        assert!(page_state.page_init_error().is_some());
        assert!(!page_state.page_layout_ready());
    }
}
