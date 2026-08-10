//! Asynchronous, cached reads of the application facade's plugin surfaces.
//!
//! This coordinator is intentionally a frontend cache, not a second plugin
//! runtime. It owns request coalescing, stale-result rejection, and the
//! repaint epoch; `ArclainApp` owns every plugin call and all WASM execution.

use crate::features::plugins::domain::types::{PluginInfo, PluginStatus, RequestId};
use arclain_app::error::ApplicationErrorKind;
use arclain_app::plugins::{
    DomainWhitelistEntryDto, PluginChromeSnapshot, PluginInstallPreviewDto,
    PluginNetworkLogEntryDto, PluginSummary,
};
use arclain_app::{ArclainApp, Signal};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const NETWORK_LOG_TTL: Duration = Duration::from_secs(1);
const DOMAIN_WHITELIST_TTL: Duration = Duration::from_secs(1);
const MAX_PENDING_JOBS: usize = 96;
const COMPLETED_JOB_CAPACITY: usize = MAX_PENDING_JOBS + 8;

#[derive(Clone, Debug)]
pub enum PluginUiRequest {
    Snapshot,
    ChromeSnapshot,
    NetworkLog,
    DomainWhitelist {
        plugin_id: String,
    },
    SetDomainApproved {
        plugin_id: String,
        domain: String,
        approved: bool,
    },
    InspectPackage {
        package_path: PathBuf,
    },
    InstallPackage {
        package_path: PathBuf,
        expected_fingerprint: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginUiFailureContext {
    Snapshot,
    ChromeSnapshot,
    NetworkLog,
    DomainWhitelist {
        plugin_id: String,
    },
    SetDomainApproved {
        plugin_id: String,
        domain: String,
        approved: bool,
    },
    InspectPackage {
        package_path: PathBuf,
    },
    InstallPackage {
        package_path: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub enum PluginUiResult {
    SnapshotLoaded {
        request_id: RequestId,
        plugins: Vec<PluginInfo>,
    },
    ChromeSnapshotLoaded {
        request_id: RequestId,
        snapshot: PluginChromeSnapshot,
    },
    NetworkLogLoaded {
        request_id: RequestId,
        entries: Vec<(SystemTime, String)>,
    },
    DomainWhitelistLoaded {
        request_id: RequestId,
        plugin_id: String,
        entries: Vec<DomainWhitelistEntryDto>,
    },
    DomainApprovalFinished {
        request_id: RequestId,
        plugin_id: String,
        result: Result<(), String>,
    },
    PackageInspected {
        request_id: RequestId,
        preview: PluginInstallPreviewDto,
    },
    PackageInstalled {
        request_id: RequestId,
        plugin_id: String,
    },
    Failed {
        request_id: RequestId,
        context: PluginUiFailureContext,
        error_kind: Option<ApplicationErrorKind>,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RequestKey {
    Snapshot,
    ChromeSnapshot,
    NetworkLog,
    DomainWhitelist(String),
    DomainApproval {
        plugin_id: String,
        domain: String,
        approved: bool,
    },
    InspectPackage(RequestId),
    InstallPackage(RequestId),
}

impl PluginUiRequest {
    fn key(&self, request_id: RequestId) -> RequestKey {
        match self {
            Self::Snapshot => RequestKey::Snapshot,
            Self::ChromeSnapshot => RequestKey::ChromeSnapshot,
            Self::NetworkLog => RequestKey::NetworkLog,
            Self::DomainWhitelist { plugin_id } => RequestKey::DomainWhitelist(plugin_id.clone()),
            Self::SetDomainApproved {
                plugin_id,
                domain,
                approved,
            } => RequestKey::DomainApproval {
                plugin_id: plugin_id.clone(),
                domain: domain.clone(),
                approved: *approved,
            },
            Self::InspectPackage { .. } => RequestKey::InspectPackage(request_id),
            Self::InstallPackage { .. } => RequestKey::InstallPackage(request_id),
        }
    }
}

impl RequestKey {
    fn failure_context(&self, request: &PluginUiRequest) -> PluginUiFailureContext {
        match (self, request) {
            (Self::Snapshot, _) => PluginUiFailureContext::Snapshot,
            (Self::ChromeSnapshot, _) => PluginUiFailureContext::ChromeSnapshot,
            (Self::NetworkLog, _) => PluginUiFailureContext::NetworkLog,
            (Self::DomainWhitelist(plugin_id), _) => PluginUiFailureContext::DomainWhitelist {
                plugin_id: plugin_id.clone(),
            },
            (
                Self::DomainApproval {
                    plugin_id,
                    domain,
                    approved,
                },
                _,
            ) => PluginUiFailureContext::SetDomainApproved {
                plugin_id: plugin_id.clone(),
                domain: domain.clone(),
                approved: *approved,
            },
            (Self::InspectPackage(_), PluginUiRequest::InspectPackage { package_path }) => {
                PluginUiFailureContext::InspectPackage {
                    package_path: package_path.clone(),
                }
            }
            (Self::InstallPackage(_), PluginUiRequest::InstallPackage { package_path, .. }) => {
                PluginUiFailureContext::InstallPackage {
                    package_path: package_path.clone(),
                }
            }
            (Self::InspectPackage(_), _) => {
                unreachable!("an inspection key always belongs to an inspection")
            }
            (Self::InstallPackage(_), _) => {
                unreachable!("an install key always belongs to an install")
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Completed {
    key: RequestKey,
    result: PluginUiResult,
    tracked: bool,
}

#[derive(Default)]
struct PluginUiCache {
    snapshot: Option<Result<Arc<Vec<PluginInfo>>, Arc<str>>>,
    chrome: Option<Result<Arc<PluginChromeSnapshot>, Arc<str>>>,
    network_log: Option<(Instant, Arc<Vec<(SystemTime, String)>>)>,
    network_log_failure: Option<Arc<str>>,
    domain_whitelists: HashMap<String, (Instant, Arc<Vec<DomainWhitelistEntryDto>>)>,
    domain_whitelist_failures: HashMap<String, Arc<str>>,
}

/// Cloneable facade-query coordinator shared by render and update paths.
#[derive(Clone)]
pub struct PluginUiJobs {
    facade: Option<ArclainApp>,
    runtime: tokio::runtime::Handle,
    pending: Arc<Mutex<HashMap<RequestKey, RequestId>>>,
    outstanding: Arc<AtomicUsize>,
    completed: Arc<Mutex<VecDeque<Completed>>>,
    cache: Arc<Mutex<PluginUiCache>>,
    completion_epoch: Signal<u64>,
}

impl PluginUiJobs {
    pub fn new(facade: Option<ArclainApp>, runtime: tokio::runtime::Handle) -> Self {
        Self {
            facade,
            runtime,
            pending: Arc::new(Mutex::new(HashMap::new())),
            outstanding: Arc::new(AtomicUsize::new(0)),
            completed: Arc::new(Mutex::new(VecDeque::new())),
            cache: Arc::new(Mutex::new(PluginUiCache::default())),
            completion_epoch: Signal::new(0).with_name("plugin_ui_completion_epoch"),
        }
    }

    pub fn request(&self, request: PluginUiRequest) -> RequestId {
        let request_id = RequestId::next();
        let key = request.key(request_id);
        {
            let mut pending = self.pending.lock();
            if !matches!(
                key,
                RequestKey::InspectPackage(_) | RequestKey::InstallPackage(_)
            ) {
                if let Some(existing) = pending.get(&key) {
                    return *existing;
                }
            }
            if !try_reserve_outstanding(&self.outstanding) {
                drop(pending);
                self.publish(Completed {
                    key: key.clone(),
                    result: PluginUiResult::Failed {
                        request_id,
                        context: key.failure_context(&request),
                        error_kind: None,
                        error: "plugin UI pending-job capacity reached".to_string(),
                    },
                    tracked: false,
                });
                return request_id;
            }
            pending.insert(key.clone(), request_id);
        }

        let facade = self.facade.clone();
        let pending = self.pending.clone();
        let outstanding = self.outstanding.clone();
        let completed = self.completed.clone();
        let completion_epoch = self.completion_epoch.clone();
        self.runtime.spawn(async move {
            let result = execute(facade, request_id, request).await;
            let item = Completed {
                key: key.clone(),
                result,
                tracked: true,
            };
            if !publish_completed(&completed, &completion_epoch, item) {
                remove_pending_if_current(&pending, &key, request_id);
                release_outstanding(&outstanding);
            }
        });
        request_id
    }

    pub fn completion_signal(&self) -> &Signal<u64> {
        &self.completion_epoch
    }

    pub fn drain(&self) -> Vec<PluginUiResult> {
        let mut results = Vec::new();
        for completed in self.completed.lock().drain(..) {
            if completed.tracked {
                release_outstanding(&self.outstanding);
            }
            let request_id = result_request_id(&completed.result);
            let current = {
                let mut pending = self.pending.lock();
                let current = pending.get(&completed.key) == Some(&request_id);
                if current {
                    pending.remove(&completed.key);
                }
                current
            };
            if current || !completed.tracked {
                self.cache_completed(&completed);
                results.push(completed.result);
            }
        }
        results
    }

    pub fn plugin_snapshot(&self) -> Option<Result<Arc<Vec<PluginInfo>>, Arc<str>>> {
        let snapshot = self.cache.lock().snapshot.clone();
        if snapshot.is_none() {
            self.request(PluginUiRequest::Snapshot);
        }
        snapshot
    }

    pub fn invalidate_plugin_snapshots(&self) {
        self.cache.lock().snapshot = None;
        self.pending.lock().remove(&RequestKey::Snapshot);
    }

    pub fn chrome_snapshot(&self) -> Option<Result<Arc<PluginChromeSnapshot>, Arc<str>>> {
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

    pub fn network_log(&self) -> Option<Result<Arc<Vec<(SystemTime, String)>>, Arc<str>>> {
        self.network_log_at(Instant::now())
    }

    fn network_log_at(
        &self,
        now: Instant,
    ) -> Option<Result<Arc<Vec<(SystemTime, String)>>, Arc<str>>> {
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

    pub fn domain_whitelist(
        &self,
        plugin_id: &str,
    ) -> Option<Result<Arc<Vec<DomainWhitelistEntryDto>>, Arc<str>>> {
        self.domain_whitelist_at(plugin_id, Instant::now())
    }

    fn domain_whitelist_at(
        &self,
        plugin_id: &str,
        now: Instant,
    ) -> Option<Result<Arc<Vec<DomainWhitelistEntryDto>>, Arc<str>>> {
        let cache = self.cache.lock();
        if let Some(error) = cache.domain_whitelist_failures.get(plugin_id).cloned() {
            return Some(Err(error));
        }
        let cached = cache.domain_whitelists.get(plugin_id).cloned();
        drop(cache);
        let mutation_pending = self.pending.lock().keys().any(|key| {
            matches!(
                key,
                RequestKey::DomainApproval {
                    plugin_id: pending_plugin,
                    ..
                } if pending_plugin == plugin_id
            )
        });

        if let Some((fetched_at, entries)) = cached {
            if mutation_pending || now.saturating_duration_since(fetched_at) < DOMAIN_WHITELIST_TTL
            {
                return Some(Ok(entries));
            }
            self.invalidate_domain_whitelist(plugin_id);
        }
        if mutation_pending {
            return None;
        }

        self.request(PluginUiRequest::DomainWhitelist {
            plugin_id: plugin_id.to_string(),
        });
        None
    }

    pub fn domain_approval_pending(&self, plugin_id: &str, domain: &str) -> bool {
        self.pending.lock().keys().any(|key| {
            matches!(
                key,
                RequestKey::DomainApproval {
                    plugin_id: pending_plugin,
                    domain: pending_domain,
                    ..
                } if pending_plugin == plugin_id && pending_domain == domain
            )
        })
    }

    pub fn invalidate_domain_whitelist(&self, plugin_id: &str) {
        let mut cache = self.cache.lock();
        cache.domain_whitelists.remove(plugin_id);
        cache.domain_whitelist_failures.remove(plugin_id);
        drop(cache);
        self.pending
            .lock()
            .remove(&RequestKey::DomainWhitelist(plugin_id.to_string()));
    }

    fn publish(&self, completed: Completed) {
        let _ = publish_completed(&self.completed, &self.completion_epoch, completed);
    }

    fn cache_completed(&self, completed: &Completed) {
        let mut cache = self.cache.lock();
        match (&completed.key, &completed.result) {
            (RequestKey::Snapshot, PluginUiResult::SnapshotLoaded { plugins, .. }) => {
                cache.snapshot = Some(Ok(Arc::new(plugins.clone())));
            }
            (RequestKey::ChromeSnapshot, PluginUiResult::ChromeSnapshotLoaded { snapshot, .. }) => {
                cache.chrome = Some(Ok(Arc::new(snapshot.clone())))
            }
            (RequestKey::NetworkLog, PluginUiResult::NetworkLogLoaded { entries, .. }) => {
                cache.network_log = Some((Instant::now(), Arc::new(entries.clone())));
                cache.network_log_failure = None;
            }
            (
                RequestKey::DomainWhitelist(plugin_id),
                PluginUiResult::DomainWhitelistLoaded { entries, .. },
            ) => {
                cache.domain_whitelists.insert(
                    plugin_id.clone(),
                    (Instant::now(), Arc::new(entries.clone())),
                );
                cache.domain_whitelist_failures.remove(plugin_id);
            }
            (
                RequestKey::DomainApproval { plugin_id, .. },
                PluginUiResult::DomainApprovalFinished { result: Ok(()), .. },
            ) => {
                cache.domain_whitelists.remove(plugin_id);
                cache.domain_whitelist_failures.remove(plugin_id);
            }
            (_, PluginUiResult::Failed { context, error, .. }) => {
                let error = Arc::<str>::from(error.as_str());
                match context {
                    PluginUiFailureContext::Snapshot => cache.snapshot = Some(Err(error)),
                    PluginUiFailureContext::ChromeSnapshot => cache.chrome = Some(Err(error)),
                    PluginUiFailureContext::NetworkLog => {
                        cache.network_log = None;
                        cache.network_log_failure = Some(error);
                    }
                    PluginUiFailureContext::DomainWhitelist { plugin_id } => {
                        cache.domain_whitelists.remove(plugin_id);
                        cache
                            .domain_whitelist_failures
                            .insert(plugin_id.clone(), error);
                    }
                    PluginUiFailureContext::SetDomainApproved { .. }
                    | PluginUiFailureContext::InspectPackage { .. }
                    | PluginUiFailureContext::InstallPackage { .. } => {}
                }
            }
            _ => {}
        }
    }
}

fn publish_completed(
    completed: &Mutex<VecDeque<Completed>>,
    completion_epoch: &Signal<u64>,
    item: Completed,
) -> bool {
    let mut queue = completed.lock();
    if queue.len() == COMPLETED_JOB_CAPACITY {
        if item.tracked {
            if let Some(index) = queue.iter().position(|queued| !queued.tracked) {
                queue.remove(index);
            } else {
                tracing::error!(
                    "[plugin-facade-queries] completion queue reached its tracked-result bound"
                );
                return false;
            }
        } else if let Some(existing) = queue
            .iter_mut()
            .find(|queued| !queued.tracked && same_failure_kind(&queued.result, &item.result))
        {
            *existing = item;
            drop(queue);
            completion_epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
            return true;
        } else {
            return false;
        }
    }
    queue.push_back(item);
    drop(queue);
    completion_epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
    true
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
    pending: &Mutex<HashMap<RequestKey, RequestId>>,
    key: &RequestKey,
    request_id: RequestId,
) {
    let mut pending = pending.lock();
    if pending.get(key) == Some(&request_id) {
        pending.remove(key);
    }
}

fn same_failure_kind(left: &PluginUiResult, right: &PluginUiResult) -> bool {
    let (
        PluginUiResult::Failed { context: left, .. },
        PluginUiResult::Failed { context: right, .. },
    ) = (left, right)
    else {
        return false;
    };
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

async fn execute(
    facade: Option<ArclainApp>,
    request_id: RequestId,
    request: PluginUiRequest,
) -> PluginUiResult {
    let Some(facade) = facade else {
        return PluginUiResult::Failed {
            request_id,
            context: request.key(request_id).failure_context(&request),
            error_kind: None,
            error: "application facade is unavailable".to_string(),
        };
    };

    match request {
        PluginUiRequest::Snapshot => match facade.plugins().await {
            Ok(plugins) => PluginUiResult::SnapshotLoaded {
                request_id,
                plugins: project_plugins(plugins),
            },
            Err(error) => PluginUiResult::Failed {
                request_id,
                context: PluginUiFailureContext::Snapshot,
                error_kind: Some(error.kind),
                error: error.summary,
            },
        },
        PluginUiRequest::ChromeSnapshot => match facade.plugin_chrome().await {
            Ok(snapshot) => PluginUiResult::ChromeSnapshotLoaded {
                request_id,
                snapshot,
            },
            Err(error) => PluginUiResult::Failed {
                request_id,
                context: PluginUiFailureContext::ChromeSnapshot,
                error_kind: Some(error.kind),
                error: error.summary,
            },
        },
        PluginUiRequest::NetworkLog => match facade.plugin_network_log().await {
            Ok(entries) => PluginUiResult::NetworkLogLoaded {
                request_id,
                entries: entries.into_iter().map(project_network_log).collect(),
            },
            Err(error) => PluginUiResult::Failed {
                request_id,
                context: PluginUiFailureContext::NetworkLog,
                error_kind: Some(error.kind),
                error: error.summary,
            },
        },
        PluginUiRequest::DomainWhitelist { plugin_id } => {
            match facade.plugin_domain_whitelist(plugin_id.clone()).await {
                Ok(entries) => PluginUiResult::DomainWhitelistLoaded {
                    request_id,
                    plugin_id,
                    entries,
                },
                Err(error) => PluginUiResult::Failed {
                    request_id,
                    context: PluginUiFailureContext::DomainWhitelist { plugin_id },
                    error_kind: Some(error.kind),
                    error: error.summary,
                },
            }
        }
        PluginUiRequest::SetDomainApproved {
            plugin_id,
            domain,
            approved,
        } => {
            let result = facade
                .set_plugin_domain_approved(plugin_id.clone(), domain, approved)
                .await
                .map_err(|error| error.summary);
            PluginUiResult::DomainApprovalFinished {
                request_id,
                plugin_id,
                result,
            }
        }
        PluginUiRequest::InspectPackage { package_path } => {
            match facade.inspect_plugin_package(package_path.clone()).await {
                Ok(preview) => PluginUiResult::PackageInspected {
                    request_id,
                    preview,
                },
                Err(error) => PluginUiResult::Failed {
                    request_id,
                    context: PluginUiFailureContext::InspectPackage { package_path },
                    error_kind: Some(error.kind),
                    error: error.summary,
                },
            }
        }
        PluginUiRequest::InstallPackage {
            package_path,
            expected_fingerprint,
        } => match facade
            .install_plugin_package(package_path.clone(), expected_fingerprint)
            .await
        {
            Ok(plugin_id) => PluginUiResult::PackageInstalled {
                request_id,
                plugin_id,
            },
            Err(error) => PluginUiResult::Failed {
                request_id,
                context: PluginUiFailureContext::InstallPackage { package_path },
                error_kind: Some(error.kind),
                error: error.summary,
            },
        },
    }
}

fn project_plugins(plugins: Vec<PluginSummary>) -> Vec<PluginInfo> {
    let mut projected = plugins
        .into_iter()
        .map(|plugin| {
            let loaded = plugin.load_error.is_none();
            PluginInfo {
                visibility: plugin.visibility.into_iter().collect(),
                id: plugin.id,
                name: plugin.name,
                version: plugin.version,
                author: Some(plugin.author),
                description: Some(plugin.description),
                capabilities: plugin
                    .capabilities
                    .into_iter()
                    .map(|capability| capability.label().to_string())
                    .collect(),
                enabled: plugin.enabled,
                loaded,
                status: if loaded {
                    PluginStatus::Ready
                } else {
                    PluginStatus::Error
                },
                error: plugin.load_error,
            }
        })
        .collect::<Vec<_>>();
    projected.sort_by_cached_key(|plugin| plugin.name.to_lowercase());
    projected
}

fn project_network_log(entry: PluginNetworkLogEntryDto) -> (SystemTime, String) {
    let magnitude = entry.logged_at_unix_ms.unsigned_abs();
    let offset = Duration::from_millis(magnitude);
    let time = if entry.logged_at_unix_ms >= 0 {
        SystemTime::UNIX_EPOCH
            .checked_add(offset)
            .unwrap_or(SystemTime::UNIX_EPOCH)
    } else {
        SystemTime::UNIX_EPOCH
            .checked_sub(offset)
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    (time, entry.message)
}

fn result_request_id(result: &PluginUiResult) -> RequestId {
    match result {
        PluginUiResult::SnapshotLoaded { request_id, .. }
        | PluginUiResult::ChromeSnapshotLoaded { request_id, .. }
        | PluginUiResult::NetworkLogLoaded { request_id, .. }
        | PluginUiResult::DomainWhitelistLoaded { request_id, .. }
        | PluginUiResult::DomainApprovalFinished { request_id, .. }
        | PluginUiResult::PackageInspected { request_id, .. }
        | PluginUiResult::PackageInstalled { request_id, .. }
        | PluginUiResult::Failed { request_id, .. } => *request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::plugins::PluginCapabilityDto;

    struct TestJobs {
        jobs: PluginUiJobs,
        // Declared last so every Handle held by `jobs` is released before
        // the owning runtime is dropped.
        _runtime: tokio::runtime::Runtime,
    }

    impl std::ops::Deref for TestJobs {
        type Target = PluginUiJobs;

        fn deref(&self) -> &Self::Target {
            &self.jobs
        }
    }

    fn test_jobs() -> TestJobs {
        let runtime = tokio::runtime::Runtime::new().expect("create runtime");
        let jobs = PluginUiJobs::new(None, runtime.handle().clone());
        TestJobs {
            jobs,
            _runtime: runtime,
        }
    }

    #[test]
    fn plugin_projection_preserves_visibility_and_facade_capability_labels() {
        let mut visibility = std::collections::BTreeMap::new();
        visibility.insert("toolbar".to_string(), true);
        let plugins = project_plugins(vec![PluginSummary {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            version: "1.0.0".to_string(),
            author: "Arclain".to_string(),
            description: "fixture".to_string(),
            capabilities: vec![PluginCapabilityDto::Network],
            visibility,
            enabled: true,
            load_error: None,
        }]);

        assert_eq!(plugins[0].capabilities, vec!["Network"]);
        assert_eq!(plugins[0].visibility.get("toolbar"), Some(&true));
        assert_eq!(plugins[0].status, PluginStatus::Ready);
    }

    #[test]
    fn package_inspection_failure_keeps_its_request_identity_and_context() {
        let temp = tempfile::tempdir().expect("temporary profile");
        let facade = crate::test_support::bootstrap_test_facade(&temp);
        let runtime = tokio::runtime::Runtime::new().expect("create runtime");
        let package_path = temp.path().join("missing.wirt");
        let result = runtime.block_on(execute(
            Some(facade),
            RequestId(40),
            PluginUiRequest::InspectPackage {
                package_path: package_path.clone(),
            },
        ));

        assert!(matches!(
            result,
            PluginUiResult::Failed {
                request_id: RequestId(40),
                context: PluginUiFailureContext::InspectPackage { package_path: failed_path },
                error_kind: Some(ApplicationErrorKind::Backend),
                error,
            } if failed_path == package_path && !error.is_empty()
        ));
    }

    #[test]
    fn negative_network_log_time_projects_before_the_epoch() {
        let (time, message) = project_network_log(PluginNetworkLogEntryDto {
            logged_at_unix_ms: -25,
            message: "before".to_string(),
        });

        assert_eq!(
            SystemTime::UNIX_EPOCH.duration_since(time).unwrap(),
            Duration::from_millis(25)
        );
        assert_eq!(message, "before");
    }

    #[test]
    fn invalidation_rejects_a_late_snapshot_completion() {
        let jobs = test_jobs();
        let key = RequestKey::Snapshot;
        let request_id = RequestId(41);
        jobs.pending.lock().insert(key.clone(), request_id);
        jobs.outstanding.store(1, AtomicOrdering::Release);
        jobs.invalidate_plugin_snapshots();
        jobs.publish(Completed {
            key,
            result: PluginUiResult::SnapshotLoaded {
                request_id,
                plugins: Vec::new(),
            },
            tracked: true,
        });

        assert!(
            jobs.drain().is_empty(),
            "an invalidated result must not escape to the UI"
        );
        assert!(
            jobs.cache.lock().snapshot.is_none(),
            "a late result must not repopulate the invalidated cache"
        );
        assert_eq!(jobs.outstanding.load(AtomicOrdering::Acquire), 0);
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
    fn domain_whitelist_cache_expires_and_refetches() {
        let jobs = test_jobs();
        let fetched_at = Instant::now();
        let entries = Arc::new(vec![DomainWhitelistEntryDto {
            plugin_id: "demo".to_string(),
            domain: "example.test".to_string(),
            approved: false,
        }]);
        jobs.cache
            .lock()
            .domain_whitelists
            .insert("demo".to_string(), (fetched_at, entries.clone()));

        let cached = jobs
            .domain_whitelist_at(
                "demo",
                fetched_at + DOMAIN_WHITELIST_TTL - Duration::from_millis(1),
            )
            .expect("fresh whitelist must remain cached")
            .expect("fresh whitelist must be successful");
        assert!(Arc::ptr_eq(&cached, &entries));

        assert!(
            jobs.domain_whitelist_at("demo", fetched_at + DOMAIN_WHITELIST_TTL,)
                .is_none(),
            "an expired whitelist must be refreshed off the render thread",
        );
        assert!(jobs
            .pending
            .lock()
            .contains_key(&RequestKey::DomainWhitelist("demo".to_string())));
    }

    #[test]
    fn pending_domain_mutation_keeps_the_last_whitelist_visible() {
        let jobs = test_jobs();
        let fetched_at = Instant::now();
        let entries = Arc::new(vec![DomainWhitelistEntryDto {
            plugin_id: "demo".to_string(),
            domain: "example.test".to_string(),
            approved: false,
        }]);
        jobs.cache
            .lock()
            .domain_whitelists
            .insert("demo".to_string(), (fetched_at, entries.clone()));
        jobs.pending.lock().insert(
            RequestKey::DomainApproval {
                plugin_id: "demo".to_string(),
                domain: "example.test".to_string(),
                approved: true,
            },
            RequestId(42),
        );

        let cached = jobs
            .domain_whitelist_at("demo", fetched_at + DOMAIN_WHITELIST_TTL)
            .expect("a mutation must keep the previous row visible")
            .expect("the previous row remains successful");

        assert!(Arc::ptr_eq(&cached, &entries));
        assert!(jobs.domain_approval_pending("demo", "example.test"));
        assert!(!jobs
            .pending
            .lock()
            .contains_key(&RequestKey::DomainWhitelist("demo".to_string())));
    }

    #[test]
    fn successful_domain_mutation_invalidates_the_whitelist_cache() {
        let jobs = test_jobs();
        jobs.cache
            .lock()
            .domain_whitelists
            .insert("demo".to_string(), (Instant::now(), Arc::new(Vec::new())));
        let completed = Completed {
            key: RequestKey::DomainApproval {
                plugin_id: "demo".to_string(),
                domain: "example.test".to_string(),
                approved: true,
            },
            result: PluginUiResult::DomainApprovalFinished {
                request_id: RequestId(43),
                plugin_id: "demo".to_string(),
                result: Ok(()),
            },
            tracked: false,
        };

        jobs.cache_completed(&completed);

        assert!(!jobs.cache.lock().domain_whitelists.contains_key("demo"));
    }

    #[test]
    fn pending_capacity_rejects_without_starting_more_work() {
        let jobs = test_jobs();
        jobs.outstanding
            .store(MAX_PENDING_JOBS, AtomicOrdering::Release);
        {
            let mut pending = jobs.pending.lock();
            for index in 0..MAX_PENDING_JOBS {
                let request_id = RequestId(index as u64);
                pending.insert(RequestKey::InstallPackage(request_id), request_id);
            }
        }

        jobs.request(PluginUiRequest::Snapshot);

        assert_eq!(jobs.pending.lock().len(), MAX_PENDING_JOBS);
        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed { error, .. }]
                if error.contains("pending-job capacity")
        ));
    }

    #[test]
    fn invalidated_keys_do_not_bypass_outstanding_capacity() {
        let jobs = test_jobs();
        jobs.outstanding
            .store(MAX_PENDING_JOBS, std::sync::atomic::Ordering::Release);

        jobs.invalidate_plugin_snapshots();
        jobs.request(PluginUiRequest::Snapshot);

        assert_eq!(
            jobs.outstanding.load(std::sync::atomic::Ordering::Acquire),
            MAX_PENDING_JOBS
        );
        assert!(matches!(
            jobs.drain().as_slice(),
            [PluginUiResult::Failed { error, .. }]
                if error.contains("pending-job capacity")
        ));
    }
}
