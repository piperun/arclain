//! Background coordinator for plugin UI reads and mutations.

use crate::core::tabs::TabId;
use crate::features::plugins::domain::types::{PluginInfo, PluginsListState, RequestId};
use arclain_core::UserConfig;
use arclain_plugins::host_functions::EventContext;
use arclain_plugins::manager::PluginStatusSummary;
use arclain_plugins::types::{PluginAction, PluginExtensionPoint, PluginLayout, TopTabConfig};
use arclain_plugins::PluginManager;
use arclain_signals::Signal;
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const NETWORK_LOG_TTL: Duration = Duration::from_secs(1);
const ORDERED_JOB_CAPACITY: usize = 32;
const MAX_PENDING_JOBS: usize = 96;
// One coalesced admission failure for each PluginUiFailureContext variant.
const REJECTED_RESULT_CAPACITY: usize = 8;
const COMPLETED_JOB_CAPACITY: usize = MAX_PENDING_JOBS + REJECTED_RESULT_CAPACITY;
const MAX_UI_PLUGIN_ID_BYTES: usize = 64;
const MAX_UI_EVENT_ID_BYTES: usize = 512;
const MAX_UI_EVENT_VALUE_BYTES: usize = 64 * 1024;
const MAX_UI_PAGE_ID_BYTES: usize = 512;
const MAX_UI_INSTALL_PATH_BYTES: usize = 32 * 1024;
const MAX_ORIGIN_ARCHIVE_PATH_BYTES: usize = 32 * 1024;
const MAX_ORIGIN_PASSWORD_BYTES: usize = 4 * 1024;
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
    Install {
        wasm_path: PathBuf,
    },
    UiEvent {
        plugin_id: String,
        event_id: String,
        value: Option<String>,
        origin_tab: TabId,
    },
    /// Host-generated state notification. Newer queued values replace older
    /// values for the same plugin/event/tab, unlike direct user actions.
    ReactiveUiEvent {
        plugin_id: String,
        event_id: String,
        value: Option<String>,
        origin_tab: TabId,
    },
}

#[derive(Clone, Debug)]
pub struct PluginUiEventCompletion {
    pub actions: Vec<PluginAction>,
    pub actions_limited: bool,
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
        actions_limited: bool,
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
    Install(RequestId, PathBuf),
    UiEvent(RequestId, String, String, TabId),
    ReactiveUiEvent(String, String, TabId),
    Rejected(RequestId),
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
            Self::Install { wasm_path } => RequestKey::Install(request_id, wasm_path.clone()),
            Self::UiEvent {
                plugin_id,
                event_id,
                origin_tab,
                ..
            } => RequestKey::UiEvent(request_id, plugin_id.clone(), event_id.clone(), *origin_tab),
            Self::ReactiveUiEvent {
                plugin_id,
                event_id,
                origin_tab,
                ..
            } => RequestKey::ReactiveUiEvent(plugin_id.clone(), event_id.clone(), *origin_tab),
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
            Self::Install(_, wasm_path) => PluginUiFailureContext::Install {
                wasm_path: wasm_path.clone(),
            },
            Self::UiEvent(_, plugin_id, event_id, origin_tab) => PluginUiFailureContext::UiEvent {
                plugin_id: plugin_id.clone(),
                event_id: event_id.clone(),
                origin_tab: *origin_tab,
            },
            Self::ReactiveUiEvent(plugin_id, event_id, origin_tab) => {
                PluginUiFailureContext::UiEvent {
                    plugin_id: plugin_id.clone(),
                    event_id: event_id.clone(),
                    origin_tab: *origin_tab,
                }
            }
            Self::Rejected(_) => PluginUiFailureContext::UiEvent {
                plugin_id: "<rejected>".to_string(),
                event_id: "<rejected>".to_string(),
                origin_tab: TabId(0),
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
            Self::Install { .. } => "install",
            Self::UiEvent { .. } => "UI event",
        }
    }
}

struct Completed {
    key: RequestKey,
    result: PluginUiResult,
    tracked: bool,
}

#[derive(Clone)]
struct CompletedQueue {
    results: Arc<Mutex<VecDeque<Completed>>>,
    capacity: usize,
}

enum CompletionQueueAdmission {
    Inserted,
    Replaced(Completed),
    Full(Completed),
}

impl CompletedQueue {
    fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "plugin UI completion queue must have capacity"
        );
        Self {
            results: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    fn try_push_or_replace(&self, completed: Completed) -> CompletionQueueAdmission {
        let mut results = self.results.lock();
        if let Some(index) = results
            .iter()
            .position(|queued| completed_results_coalesce(queued, &completed))
        {
            if result_request_id(&completed.result).0 < result_request_id(&results[index].result).0
            {
                return CompletionQueueAdmission::Replaced(completed);
            }
            let replaced = std::mem::replace(&mut results[index], completed);
            return CompletionQueueAdmission::Replaced(replaced);
        }
        if results.len() == self.capacity {
            return CompletionQueueAdmission::Full(completed);
        }
        results.push_back(completed);
        CompletionQueueAdmission::Inserted
    }

    fn drain(&self) -> Vec<Completed> {
        self.results.lock().drain(..).collect()
    }
}

fn completed_results_coalesce(queued: &Completed, incoming: &Completed) -> bool {
    match (&queued.key, &incoming.key) {
        (RequestKey::Rejected(_), RequestKey::Rejected(_)) => {
            rejected_failure_contexts_match(&queued.result, &incoming.result)
        }
        _ if queued.key != incoming.key => false,
        _ => matches!(
            &queued.key,
            RequestKey::Layout(..)
                | RequestKey::Snapshot(..)
                | RequestKey::ChromeSnapshot
                | RequestKey::NetworkLog
                | RequestKey::ReactiveUiEvent(..)
        ),
    }
}

fn rejected_failure_contexts_match(queued: &PluginUiResult, incoming: &PluginUiResult) -> bool {
    let (
        PluginUiResult::Failed {
            context: queued, ..
        },
        PluginUiResult::Failed {
            context: incoming, ..
        },
    ) = (queued, incoming)
    else {
        return false;
    };
    matches!(
        (queued, incoming),
        (
            PluginUiFailureContext::PageInit { .. },
            PluginUiFailureContext::PageInit { .. }
        ) | (
            PluginUiFailureContext::Layout { .. },
            PluginUiFailureContext::Layout { .. }
        ) | (
            PluginUiFailureContext::Snapshot { .. },
            PluginUiFailureContext::Snapshot { .. }
        ) | (
            PluginUiFailureContext::ChromeSnapshot,
            PluginUiFailureContext::ChromeSnapshot
        ) | (
            PluginUiFailureContext::NetworkLog,
            PluginUiFailureContext::NetworkLog
        ) | (
            PluginUiFailureContext::Install { .. },
            PluginUiFailureContext::Install { .. }
        ) | (
            PluginUiFailureContext::UiEvent { .. },
            PluginUiFailureContext::UiEvent { .. }
        )
    )
}

struct QueuedJob {
    key: RequestKey,
    request_id: RequestId,
    request: PluginUiRequest,
    origin_context: Option<EventContext>,
}

struct OrderedQueueState {
    jobs: VecDeque<QueuedJob>,
    closed: bool,
}

#[derive(Clone)]
struct OrderedJobQueue {
    shared: Arc<(Mutex<OrderedQueueState>, Condvar)>,
    capacity: usize,
}

enum OrderedQueueError {
    Full(QueuedJob),
    Closed(QueuedJob),
}

impl OrderedJobQueue {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ordered plugin UI queue must have capacity");
        Self {
            shared: Arc::new((
                Mutex::new(OrderedQueueState {
                    jobs: VecDeque::with_capacity(capacity),
                    closed: false,
                }),
                Condvar::new(),
            )),
            capacity,
        }
    }

    fn try_push_or_replace(&self, job: QueuedJob) -> Result<Option<QueuedJob>, OrderedQueueError> {
        let (state_lock, ready) = self.shared.as_ref();
        let mut state = state_lock.lock();
        if state.closed {
            return Err(OrderedQueueError::Closed(job));
        }
        if matches!(job.key, RequestKey::ReactiveUiEvent(..)) {
            if let Some(index) = state.jobs.iter().position(|queued| queued.key == job.key) {
                if state.jobs[index].request_id.0 > job.request_id.0 {
                    return Ok(Some(job));
                }
                let replaced = std::mem::replace(&mut state.jobs[index], job);
                ready.notify_one();
                return Ok(Some(replaced));
            }
        }
        if state.jobs.len() == self.capacity {
            return Err(OrderedQueueError::Full(job));
        }
        state.jobs.push_back(job);
        ready.notify_one();
        Ok(None)
    }

    fn recv(&self) -> Option<QueuedJob> {
        let (state_lock, ready) = self.shared.as_ref();
        let mut state = state_lock.lock();
        loop {
            if let Some(job) = state.jobs.pop_front() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            ready.wait(&mut state);
        }
    }

    fn close(&self) {
        let (state_lock, ready) = self.shared.as_ref();
        state_lock.lock().closed = true;
        ready.notify_all();
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PluginUiMutation {
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

fn field_within(value: &str, limit: usize) -> bool {
    value.len() <= limit
}

fn path_within(value: &std::path::Path, limit: usize) -> bool {
    value.as_os_str().as_encoded_bytes().len() <= limit
}

fn safe_failure_field(value: &str, limit: usize) -> String {
    if field_within(value, limit) {
        value.to_string()
    } else {
        "<oversized>".to_string()
    }
}

fn safe_failure_target(target: &PluginUiTarget) -> PluginUiTarget {
    match target {
        PluginUiTarget::Dialog(id) => {
            PluginUiTarget::Dialog(safe_failure_field(id, MAX_UI_PAGE_ID_BYTES))
        }
        PluginUiTarget::Page(id) => {
            PluginUiTarget::Page(safe_failure_field(id, MAX_UI_PAGE_ID_BYTES))
        }
        other => other.clone(),
    }
}

fn invalid_request_context(request: &PluginUiRequest) -> Option<PluginUiFailureContext> {
    match request {
        PluginUiRequest::PageInit {
            plugin_id,
            page_id,
            origin_tab,
        } if !field_within(plugin_id, MAX_UI_PLUGIN_ID_BYTES)
            || !field_within(page_id, MAX_UI_PAGE_ID_BYTES) =>
        {
            Some(PluginUiFailureContext::PageInit {
                plugin_id: safe_failure_field(plugin_id, MAX_UI_PLUGIN_ID_BYTES),
                page_id: safe_failure_field(page_id, MAX_UI_PAGE_ID_BYTES),
                origin_tab: *origin_tab,
            })
        }
        PluginUiRequest::Layout {
            plugin_id,
            target,
            origin_tab,
        } if !field_within(plugin_id, MAX_UI_PLUGIN_ID_BYTES)
            || matches!(target, PluginUiTarget::Dialog(id) | PluginUiTarget::Page(id)
                if !field_within(id, MAX_UI_PAGE_ID_BYTES)) =>
        {
            Some(PluginUiFailureContext::Layout {
                plugin_id: safe_failure_field(plugin_id, MAX_UI_PLUGIN_ID_BYTES),
                target: safe_failure_target(target),
                origin_tab: *origin_tab,
            })
        }
        PluginUiRequest::Install { wasm_path }
            if !path_within(wasm_path, MAX_UI_INSTALL_PATH_BYTES) =>
        {
            Some(PluginUiFailureContext::Install {
                wasm_path: PathBuf::from("<oversized>"),
            })
        }
        PluginUiRequest::UiEvent {
            plugin_id,
            event_id,
            value,
            origin_tab,
        }
        | PluginUiRequest::ReactiveUiEvent {
            plugin_id,
            event_id,
            value,
            origin_tab,
        } if !field_within(plugin_id, MAX_UI_PLUGIN_ID_BYTES)
            || !field_within(event_id, MAX_UI_EVENT_ID_BYTES)
            || value
                .as_deref()
                .is_some_and(|value| !field_within(value, MAX_UI_EVENT_VALUE_BYTES)) =>
        {
            Some(PluginUiFailureContext::UiEvent {
                plugin_id: safe_failure_field(plugin_id, MAX_UI_PLUGIN_ID_BYTES),
                event_id: safe_failure_field(event_id, MAX_UI_EVENT_ID_BYTES),
                origin_tab: *origin_tab,
            })
        }
        _ => None,
    }
}

fn origin_context_within_limits(context: &EventContext) -> bool {
    field_within(&context.archive_path, MAX_ORIGIN_ARCHIVE_PATH_BYTES)
        && context
            .password
            .as_deref()
            .is_none_or(|password| field_within(password, MAX_ORIGIN_PASSWORD_BYTES))
}

pub struct PluginUiJobs {
    manager: Option<Arc<Mutex<PluginManager>>>,
    runtime: Arc<tokio::runtime::Runtime>,
    pending: Arc<Mutex<HashMap<RequestKey, RequestId>>>,
    completed: CompletedQueue,
    outstanding: Arc<AtomicUsize>,
    cache: Arc<Mutex<PluginUiCache>>,
    completion_epoch: Signal<u64>,
    ordered_queue: OrderedJobQueue,
    origin_context_provider: Option<OriginContextProvider>,
    owners: Arc<AtomicUsize>,
}

impl Clone for PluginUiJobs {
    fn clone(&self) -> Self {
        self.owners.fetch_add(1, AtomicOrdering::Relaxed);
        Self {
            manager: self.manager.clone(),
            runtime: self.runtime.clone(),
            pending: self.pending.clone(),
            completed: self.completed.clone(),
            outstanding: self.outstanding.clone(),
            cache: self.cache.clone(),
            completion_epoch: self.completion_epoch.clone(),
            ordered_queue: self.ordered_queue.clone(),
            origin_context_provider: self.origin_context_provider.clone(),
            owners: self.owners.clone(),
        }
    }
}

impl Drop for PluginUiJobs {
    fn drop(&mut self) {
        if self.owners.fetch_sub(1, AtomicOrdering::AcqRel) == 1 {
            self.ordered_queue.close();
        }
    }
}

impl PluginUiJobs {
    pub fn new(
        manager: Option<Arc<Mutex<PluginManager>>>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self::new_with_capacities(
            manager,
            runtime,
            ORDERED_JOB_CAPACITY,
            COMPLETED_JOB_CAPACITY,
        )
    }

    fn new_with_capacities(
        manager: Option<Arc<Mutex<PluginManager>>>,
        runtime: Arc<tokio::runtime::Runtime>,
        ordered_capacity: usize,
        completed_capacity: usize,
    ) -> Self {
        let completed = CompletedQueue::new(completed_capacity);
        let ordered_queue = OrderedJobQueue::new(ordered_capacity);
        let ordered_manager = manager.clone();
        let worker_completed = completed.clone();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let worker_pending = pending.clone();
        let outstanding = Arc::new(AtomicUsize::new(0));
        let worker_outstanding = outstanding.clone();
        let ordered_epoch = Signal::new(0).with_name("plugin_ui_completion_epoch");
        let worker_epoch = ordered_epoch.clone();
        let worker_queue = ordered_queue.clone();
        std::thread::Builder::new()
            .name("plugin-ui-ordered-jobs".to_string())
            .spawn(move || {
                while let Some(job) = worker_queue.recv() {
                    let completed = execute_job(ordered_manager.clone(), job);
                    publish_completed(
                        &worker_completed,
                        &worker_pending,
                        &worker_outstanding,
                        &worker_epoch,
                        completed,
                    );
                }
            })
            .expect("failed to start plugin UI ordered worker");

        Self {
            manager,
            runtime,
            pending,
            completed,
            outstanding,
            cache: Arc::new(Mutex::new(PluginUiCache::default())),
            completion_epoch: ordered_epoch,
            ordered_queue,
            origin_context_provider: None,
            owners: Arc::new(AtomicUsize::new(1)),
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
        if let Some(context) = invalid_request_context(&request) {
            let completed = Completed {
                key: RequestKey::Rejected(request_id),
                result: PluginUiResult::Failed {
                    request_id,
                    context,
                    error: "plugin UI request exceeded a host field limit".to_string(),
                },
                tracked: false,
            };
            publish_completed(
                &self.completed,
                &self.pending,
                &self.outstanding,
                &self.completion_epoch,
                completed,
            );
            return request_id;
        }

        let key = request.key(request_id);
        let reactive = matches!(key, RequestKey::ReactiveUiEvent(..));
        {
            let mut pending = self.pending.lock();
            if !reactive {
                if let Some(existing) = pending.get(&key) {
                    return *existing;
                }
            } else if let Some(existing) = pending.get(&key) {
                if existing.0 >= request_id.0 {
                    return *existing;
                }
            }
            if (!pending.contains_key(&key) && pending.len() == MAX_PENDING_JOBS)
                || !try_reserve_outstanding(&self.outstanding)
            {
                drop(pending);
                let context = key.failure_context();
                let completed = Completed {
                    key: RequestKey::Rejected(request_id),
                    result: PluginUiResult::Failed {
                        request_id,
                        context,
                        error: "plugin UI pending-job capacity reached".to_string(),
                    },
                    tracked: false,
                };
                publish_completed(
                    &self.completed,
                    &self.pending,
                    &self.outstanding,
                    &self.completion_epoch,
                    completed,
                );
                return request_id;
            }
            pending.insert(key.clone(), request_id);
        }

        let origin_tab = match &request {
            PluginUiRequest::PageInit { origin_tab, .. } => Some(*origin_tab),
            PluginUiRequest::Layout { origin_tab, .. } => *origin_tab,
            PluginUiRequest::UiEvent { origin_tab, .. }
            | PluginUiRequest::ReactiveUiEvent { origin_tab, .. } => Some(*origin_tab),
            _ => None,
        };
        let origin_context = origin_tab.and_then(|tab_id| {
            self.origin_context_provider
                .as_ref()
                .and_then(|provider| provider(tab_id))
        });
        if origin_context
            .as_ref()
            .is_some_and(|context| !origin_context_within_limits(context))
        {
            let completed = rejected_completed_parts(
                key.clone(),
                request_id,
                "origin context exceeded a host field limit".to_string(),
            );
            remove_pending_if_current(&self.pending, &key, request_id);
            release_outstanding(&self.outstanding);
            publish_completed(
                &self.completed,
                &self.pending,
                &self.outstanding,
                &self.completion_epoch,
                completed,
            );
            return request_id;
        }
        let job = QueuedJob {
            key,
            request_id,
            request,
            origin_context,
        };

        if matches!(
            &job.request,
            PluginUiRequest::PageInit { .. }
                | PluginUiRequest::Install { .. }
                | PluginUiRequest::UiEvent { .. }
                | PluginUiRequest::ReactiveUiEvent { .. }
        ) {
            match self.ordered_queue.try_push_or_replace(job) {
                Ok(Some(_replaced)) => {
                    release_outstanding(&self.outstanding);
                }
                Ok(None) => {}
                Err(error) => {
                    let (job, reason) = match error {
                        OrderedQueueError::Full(job) => {
                            (job, "plugin UI ordered-job capacity reached")
                        }
                        OrderedQueueError::Closed(job) => {
                            (job, "plugin UI ordered worker is unavailable")
                        }
                    };
                    let rejected_key = job.key.clone();
                    let rejected_id = job.request_id;
                    let completed = rejected_completed(job, reason.to_string());
                    remove_pending_if_current(&self.pending, &rejected_key, rejected_id);
                    release_outstanding(&self.outstanding);
                    publish_completed(
                        &self.completed,
                        &self.pending,
                        &self.outstanding,
                        &self.completion_epoch,
                        completed,
                    );
                }
            }
            return request_id;
        }

        let manager = self.manager.clone();
        let completed_queue = self.completed.clone();
        let pending = self.pending.clone();
        let outstanding = self.outstanding.clone();
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
            publish_completed(
                &completed_queue,
                &pending,
                &outstanding,
                &completion_epoch,
                completed,
            );
        });
        request_id
    }

    pub fn completion_signal(&self) -> &Signal<u64> {
        &self.completion_epoch
    }

    pub fn drain(&self) -> Vec<PluginUiResult> {
        let mut results = Vec::new();
        for completed in self.completed.drain() {
            let request_id = result_request_id(&completed.result);
            let mut pending = self.pending.lock();
            let current = pending.get(&completed.key) == Some(&request_id);
            if current {
                pending.remove(&completed.key);
            }
            if completed.tracked {
                release_outstanding(&self.outstanding);
            }
            if current || !completed.tracked {
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

fn try_reserve_outstanding(outstanding: &AtomicUsize) -> bool {
    outstanding
        .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |current| {
            (current < MAX_PENDING_JOBS).then_some(current + 1)
        })
        .is_ok()
}

fn release_outstanding(outstanding: &AtomicUsize) {
    let previous = outstanding.fetch_sub(1, AtomicOrdering::AcqRel);
    debug_assert!(previous > 0, "plugin UI outstanding count underflowed");
}

fn remove_pending_if_current(
    pending: &Arc<Mutex<HashMap<RequestKey, RequestId>>>,
    key: &RequestKey,
    request_id: RequestId,
) {
    let mut pending = pending.lock();
    if pending.get(key) == Some(&request_id) {
        pending.remove(key);
    }
}

fn publish_completed(
    completed_queue: &CompletedQueue,
    pending: &Arc<Mutex<HashMap<RequestKey, RequestId>>>,
    outstanding: &Arc<AtomicUsize>,
    completion_epoch: &Signal<u64>,
    completed: Completed,
) {
    match completed_queue.try_push_or_replace(completed) {
        CompletionQueueAdmission::Inserted => {
            completion_epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
        }
        CompletionQueueAdmission::Replaced(replaced) => {
            if replaced.tracked {
                release_outstanding(outstanding);
            }
            completion_epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
        }
        CompletionQueueAdmission::Full(completed) => {
            if completed.tracked {
                let request_id = result_request_id(&completed.result);
                remove_pending_if_current(pending, &completed.key, request_id);
                release_outstanding(outstanding);
            }
        }
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
    Completed {
        key,
        result,
        tracked: true,
    }
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

fn rejected_completed(job: QueuedJob, error: String) -> Completed {
    rejected_completed_parts(job.key, job.request_id, error)
}

fn rejected_completed_parts(key: RequestKey, request_id: RequestId, error: String) -> Completed {
    let mut completed = failed_completed_parts(key, request_id, error);
    completed.key = RequestKey::Rejected(request_id);
    completed.tracked = false;
    completed
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
        tracked: true,
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
            let (actions, actions_limited) = match actions {
                Ok(actions) => {
                    let bounded =
                        arclain_plugins::action_policy::bound_plugin_actions_with_status(actions);
                    (Ok(bounded.actions), bounded.limited)
                }
                Err(error) => (Err(error), false),
            };
            PluginUiResult::PageInitialized {
                request_id,
                plugin_id,
                page_id,
                origin_tab,
                actions,
                actions_limited,
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
        }
        | PluginUiRequest::ReactiveUiEvent {
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
                    let actions =
                        arclain_plugins::action_policy::bound_plugin_actions_with_status({
                            let mut instance = instance.lock();
                            with_event_context(&mut instance, origin_context, |instance| {
                                instance
                                    .send_ui_event(&event_id, value)
                                    .map_err(|error| error.to_string())
                            })?
                        });
                    let settings = manager.lock().get_settings_for(&plugin_id);
                    Ok(PluginUiEventCompletion {
                        actions: actions.actions,
                        actions_limited: actions.limited,
                        settings,
                    })
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

    fn reactive_job(request_id: u64, value: &str) -> QueuedJob {
        let request_id = RequestId(request_id);
        let request = PluginUiRequest::ReactiveUiEvent {
            plugin_id: "plugin".to_string(),
            event_id: "changed".to_string(),
            value: Some(value.to_string()),
            origin_tab: TabId(7),
        };
        QueuedJob {
            key: request.key(request_id),
            request_id,
            request,
            origin_context: None,
        }
    }

    fn direct_job(request_id: u64) -> QueuedJob {
        let request_id = RequestId(request_id);
        let request = PluginUiRequest::UiEvent {
            plugin_id: "plugin".to_string(),
            event_id: format!("clicked-{request_id:?}"),
            value: None,
            origin_tab: TabId(7),
        };
        QueuedJob {
            key: request.key(request_id),
            request_id,
            request,
            origin_context: None,
        }
    }

    fn origin_context(entries: Arc<Vec<arclain_core::ArchiveEntry>>) -> EventContext {
        EventContext {
            archive_path: "archive.zip".to_string(),
            password: Some("secret".to_string()),
            entries,
            archive_session_id: 0,
        }
    }

    fn completed_ui_event(request_id: u64, reactive: bool) -> Completed {
        let request_id = RequestId(request_id);
        Completed {
            key: if reactive {
                RequestKey::ReactiveUiEvent("plugin".into(), "changed".into(), TabId(7))
            } else {
                RequestKey::UiEvent(
                    request_id,
                    "plugin".into(),
                    format!("clicked-{request_id:?}"),
                    TabId(7),
                )
            },
            result: PluginUiResult::UiEventFinished {
                request_id,
                plugin_id: "plugin".into(),
                origin_tab: TabId(7),
                result: Ok(PluginUiEventCompletion {
                    actions: Vec::new(),
                    actions_limited: false,
                    settings: None,
                }),
            },
            tracked: true,
        }
    }

    #[test]
    fn completion_queue_keeps_direct_results_and_coalesces_reactive_results() {
        let queue = CompletedQueue::new(2);
        assert!(matches!(
            queue.try_push_or_replace(completed_ui_event(1, true)),
            CompletionQueueAdmission::Inserted
        ));
        let replaced = queue.try_push_or_replace(completed_ui_event(2, true));
        assert!(matches!(
            replaced,
            CompletionQueueAdmission::Replaced(Completed {
                result: PluginUiResult::UiEventFinished {
                    request_id: RequestId(1),
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            queue.try_push_or_replace(completed_ui_event(3, false)),
            CompletionQueueAdmission::Inserted
        ));
        assert!(matches!(
            queue.try_push_or_replace(completed_ui_event(4, false)),
            CompletionQueueAdmission::Full(_)
        ));

        let results = queue.drain();
        assert_eq!(results.len(), 2);
        assert_eq!(result_request_id(&results[0].result), RequestId(2));
        assert_eq!(result_request_id(&results[1].result), RequestId(3));
    }

    #[test]
    fn completion_queue_coalesces_overload_failures_but_keeps_one_visible() {
        let queue = CompletedQueue::new(2);
        assert!(matches!(
            queue.try_push_or_replace(completed_ui_event(1, false)),
            CompletionQueueAdmission::Inserted
        ));
        assert!(matches!(
            queue.try_push_or_replace(rejected_completed_parts(
                RequestKey::NetworkLog,
                RequestId(2),
                "first overload".to_string(),
            )),
            CompletionQueueAdmission::Inserted
        ));
        assert!(matches!(
            queue.try_push_or_replace(rejected_completed_parts(
                RequestKey::NetworkLog,
                RequestId(3),
                "latest overload".to_string(),
            )),
            CompletionQueueAdmission::Replaced(_)
        ));

        let results = queue.drain();
        assert!(results.iter().any(|completed| matches!(
            &completed.result,
            PluginUiResult::Failed { error, .. } if error.contains("latest overload")
        )));
    }

    #[test]
    fn late_stale_completion_cannot_replace_a_newer_result() {
        let queue = CompletedQueue::new(1);
        assert!(matches!(
            queue.try_push_or_replace(completed_ui_event(2, true)),
            CompletionQueueAdmission::Inserted
        ));
        assert!(matches!(
            queue.try_push_or_replace(completed_ui_event(1, true)),
            CompletionQueueAdmission::Replaced(Completed {
                result: PluginUiResult::UiEventFinished {
                    request_id: RequestId(1),
                    ..
                },
                ..
            })
        ));

        let results = queue.drain();
        assert_eq!(result_request_id(&results[0].result), RequestId(2));
    }

    #[test]
    fn ordered_queue_is_bounded_and_replaces_queued_reactive_event_with_newest() {
        let queue = OrderedJobQueue::new(2);
        assert!(queue.try_push_or_replace(reactive_job(1, "old")).is_ok());
        assert!(queue.try_push_or_replace(reactive_job(2, "new")).is_ok());
        assert!(queue
            .try_push_or_replace(QueuedJob {
                key: RequestKey::PageInit(RequestId(3), "plugin".into(), "page".into(), TabId(7)),
                request_id: RequestId(3),
                request: PluginUiRequest::PageInit {
                    plugin_id: "plugin".into(),
                    page_id: "page".into(),
                    origin_tab: TabId(7),
                },
                origin_context: None,
            })
            .is_ok());
        assert!(matches!(
            queue.try_push_or_replace(QueuedJob {
                key: RequestKey::PageInit(RequestId(4), "plugin".into(), "other".into(), TabId(7)),
                request_id: RequestId(4),
                request: PluginUiRequest::PageInit {
                    plugin_id: "plugin".into(),
                    page_id: "other".into(),
                    origin_tab: TabId(7),
                },
                origin_context: None,
            }),
            Err(OrderedQueueError::Full(_))
        ));

        let newest = queue.recv().expect("reactive event queued");
        assert_eq!(newest.request_id, RequestId(2));
        assert!(matches!(
            newest.request,
            PluginUiRequest::ReactiveUiEvent { value: Some(value), .. } if value == "new"
        ));
        assert!(matches!(
            queue.recv().expect("direct event queued").request,
            PluginUiRequest::PageInit { .. }
        ));
    }

    #[test]
    fn ordered_queue_preserves_direct_actions_in_admission_order() {
        let queue = OrderedJobQueue::new(2);
        assert!(queue.try_push_or_replace(direct_job(1)).is_ok());
        assert!(queue.try_push_or_replace(direct_job(2)).is_ok());

        assert_eq!(queue.recv().unwrap().request_id, RequestId(1));
        assert_eq!(queue.recv().unwrap().request_id, RequestId(2));
    }

    #[test]
    fn older_reactive_admission_cannot_replace_a_newer_queued_value() {
        let queue = OrderedJobQueue::new(1);
        assert!(queue.try_push_or_replace(reactive_job(2, "new")).is_ok());
        let dropped = match queue.try_push_or_replace(reactive_job(1, "old")) {
            Ok(Some(job)) => job,
            _ => panic!("reactive event should be coalesced"),
        };

        assert_eq!(dropped.request_id, RequestId(1));
        assert_eq!(queue.recv().unwrap().request_id, RequestId(2));
    }

    #[test]
    fn oversized_ui_event_is_rejected_before_origin_context_is_snapshotted() {
        let provider_calls = Arc::new(AtomicU64::new(0));
        let calls = provider_calls.clone();
        let jobs = test_jobs().with_origin_context_provider(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        });

        jobs.request(PluginUiRequest::UiEvent {
            plugin_id: "plugin".to_string(),
            event_id: "x".repeat(MAX_UI_EVENT_ID_BYTES + 1),
            value: None,
            origin_tab: TabId(5),
        });

        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed { error, .. }] if error.contains("field limit")
        ));
    }

    #[test]
    fn oversized_install_path_is_rejected_without_retaining_the_path() {
        let jobs = test_jobs();

        jobs.request(PluginUiRequest::Install {
            wasm_path: PathBuf::from("x".repeat(MAX_UI_INSTALL_PATH_BYTES + 1)),
        });

        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed {
                context: PluginUiFailureContext::Install { wasm_path },
                error,
                ..
            }] if wasm_path == std::path::Path::new("<oversized>")
                && error.contains("field limit")
        ));
    }

    #[test]
    fn full_completion_queue_releases_the_dropped_pending_request() {
        let completed_queue = CompletedQueue::new(1);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let outstanding = Arc::new(AtomicUsize::new(2));
        let epoch = Signal::new(0);
        let first_key = RequestKey::ChromeSnapshot;
        let second_key = RequestKey::NetworkLog;
        pending.lock().insert(first_key.clone(), RequestId(1));
        pending.lock().insert(second_key.clone(), RequestId(2));

        publish_completed(
            &completed_queue,
            &pending,
            &outstanding,
            &epoch,
            Completed {
                key: first_key.clone(),
                result: PluginUiResult::ChromeSnapshotLoaded {
                    request_id: RequestId(1),
                    summary: PluginStatusSummary::default(),
                    top_tabs: Vec::new(),
                },
                tracked: true,
            },
        );
        publish_completed(
            &completed_queue,
            &pending,
            &outstanding,
            &epoch,
            Completed {
                key: second_key.clone(),
                result: PluginUiResult::NetworkLogLoaded {
                    request_id: RequestId(2),
                    entries: Vec::new(),
                },
                tracked: true,
            },
        );

        assert!(pending.lock().contains_key(&first_key));
        assert!(!pending.lock().contains_key(&second_key));
        assert_eq!(outstanding.load(AtomicOrdering::Acquire), 1);
        assert_eq!(epoch.get(), 1);
    }

    #[test]
    fn replacing_reactive_job_releases_old_origin_context() {
        let queue = OrderedJobQueue::new(1);
        let old_entries = Arc::new(Vec::new());
        let new_entries = Arc::new(Vec::new());
        let context = |entries: Arc<Vec<arclain_core::ArchiveEntry>>| EventContext {
            archive_path: "archive.zip".to_string(),
            password: None,
            entries,
            archive_session_id: 0,
        };
        let mut old = reactive_job(1, "old");
        old.origin_context = Some(context(old_entries.clone()));
        assert!(queue.try_push_or_replace(old).is_ok());
        assert_eq!(Arc::strong_count(&old_entries), 2);

        let mut new = reactive_job(2, "new");
        new.origin_context = Some(context(new_entries.clone()));
        assert!(queue.try_push_or_replace(new).is_ok());

        assert_eq!(Arc::strong_count(&old_entries), 1);
        assert_eq!(Arc::strong_count(&new_entries), 2);
    }

    #[test]
    fn rejecting_a_job_releases_its_sensitive_origin_snapshot() {
        let entries = Arc::new(Vec::new());
        let mut job = direct_job(1);
        job.origin_context = Some(origin_context(entries.clone()));
        assert_eq!(Arc::strong_count(&entries), 2);

        let _failure = rejected_completed(job, "queue full".to_string());

        assert_eq!(Arc::strong_count(&entries), 1);
    }

    #[test]
    fn finishing_a_job_releases_its_sensitive_origin_snapshot() {
        let entries = Arc::new(Vec::new());
        let mut job = direct_job(1);
        job.origin_context = Some(origin_context(entries.clone()));
        assert_eq!(Arc::strong_count(&entries), 2);

        let _completed = execute_job(None, job);

        assert_eq!(Arc::strong_count(&entries), 1);
    }

    #[test]
    fn pending_capacity_rejects_without_snapshotting_origin_context() {
        let provider_calls = Arc::new(AtomicU64::new(0));
        let calls = provider_calls.clone();
        let jobs = test_jobs().with_origin_context_provider(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        });
        {
            let mut pending = jobs.pending.lock();
            for index in 0..MAX_PENDING_JOBS {
                pending.insert(
                    RequestKey::Rejected(RequestId(index as u64)),
                    RequestId(index as u64),
                );
            }
        }

        jobs.request(PluginUiRequest::UiEvent {
            plugin_id: "plugin".to_string(),
            event_id: "clicked".to_string(),
            value: None,
            origin_tab: TabId(9),
        });

        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed { error, .. }] if error.contains("pending-job capacity")
        ));
    }

    #[test]
    fn outstanding_capacity_remains_bounded_after_pending_keys_are_invalidated() {
        let provider_calls = Arc::new(AtomicU64::new(0));
        let calls = provider_calls.clone();
        let jobs = test_jobs().with_origin_context_provider(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        });
        jobs.outstanding
            .store(MAX_PENDING_JOBS, AtomicOrdering::Release);

        jobs.request(PluginUiRequest::UiEvent {
            plugin_id: "plugin".to_string(),
            event_id: "clicked".to_string(),
            value: None,
            origin_tab: TabId(9),
        });

        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            jobs.outstanding.load(AtomicOrdering::Acquire),
            MAX_PENDING_JOBS
        );
        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed { error, .. }] if error.contains("pending-job capacity")
        ));
    }

    #[test]
    fn oversized_origin_context_is_not_retained_by_ordered_queue() {
        let jobs = test_jobs().with_origin_context_provider(|_| {
            Some(EventContext {
                archive_path: "archive.zip".to_string(),
                password: Some("p".repeat(MAX_ORIGIN_PASSWORD_BYTES + 1)),
                entries: Arc::new(Vec::new()),
                archive_session_id: 0,
            })
        });

        jobs.request(PluginUiRequest::UiEvent {
            plugin_id: "plugin".to_string(),
            event_id: "clicked".to_string(),
            value: None,
            origin_tab: TabId(9),
        });

        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed { error, .. }] if error.contains("origin context")
        ));
    }

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
        let jobs = test_jobs();
        jobs.ordered_queue.close();
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
