use super::*;

use arclain_data::CacheIndex;
use arclain_db::{CacheEntry, CacheType};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const EXTERNAL_LAUNCH_DENIED: &str = "external launch disabled: host UI authorization required";

fn host_functions(
    plugin_id: &str,
    capabilities: std::collections::HashSet<PluginCapability>,
    requests_per_minute: u32,
) -> HostFunctions {
    HostFunctions::new(
        plugin_id.to_string(),
        capabilities,
        requests_per_minute,
        HashMap::new(),
    )
    .unwrap()
}

fn async_client(runtime: &tokio::runtime::Runtime) -> Arc<arclain_network::AsyncHttpClient> {
    Arc::new(arclain_network::AsyncHttpClient::new(
        runtime.handle().clone(),
        Arc::new(parking_lot::RwLock::new(
            arclain_network::DomainWhitelist::default(),
        )),
        None,
    ))
}

fn data_request(
    key: &str,
    resource_type: wirt::bindings::wirt::plugin::host::ResourceType,
    sources: Vec<wirt::bindings::wirt::plugin::host::DataSource>,
) -> wirt::bindings::wirt::plugin::host::DataRequest {
    wirt::bindings::wirt::plugin::host::DataRequest {
        key: key.to_string(),
        url: Some("https://example.invalid/data".to_string()),
        resource_type,
        product_id: None,
        sources,
    }
}

struct RecordingResolver {
    body: Option<Vec<u8>>,
    resolve_calls: AtomicUsize,
    store_calls: AtomicUsize,
}

impl RecordingResolver {
    fn new(body: Option<Vec<u8>>) -> Self {
        Self {
            body,
            resolve_calls: AtomicUsize::new(0),
            store_calls: AtomicUsize::new(0),
        }
    }
}

impl arclain_data::DataSourceResolver for RecordingResolver {
    fn try_resolve(
        &self,
        _key: &str,
        _request: &arclain_data::DataRequest,
    ) -> Result<Vec<u8>, arclain_data::ResolveError> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        self.body
            .clone()
            .ok_or(arclain_data::ResolveError::NotFound)
    }

    fn try_store(
        &self,
        _key: &str,
        _data: &[u8],
        _request: &arclain_data::DataRequest,
    ) -> Result<(), arclain_data::ResolveError> {
        self.store_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn has(&self, _key: &str, _request: &arclain_data::DataRequest) -> bool {
        self.body.is_some()
    }
}

#[test]
fn async_client_observes_exact_manifest_network_policy() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = async_client(&runtime);
    let capabilities = [PluginCapability::Network].into_iter().collect();
    let mut host = host_functions("manifest-policy", capabilities, 7);

    host.set_async_http_client(client.clone());

    assert_eq!(
        client.plugin_network_policy("manifest-policy"),
        Some(arclain_network::PluginNetworkPolicy {
            network_enabled: true,
            requests_per_minute: 7,
        })
    );
}

#[test]
fn disabled_manifest_network_capability_is_registered_disabled() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = async_client(&runtime);
    let mut host = host_functions("disabled-policy", Default::default(), 19);

    host.set_async_http_client(client.clone());

    assert_eq!(
        client.plugin_network_policy("disabled-policy"),
        Some(arclain_network::PluginNetworkPolicy {
            network_enabled: false,
            requests_per_minute: 19,
        })
    );
    assert!(matches!(
        client.request_for_plugin(
            "disabled-policy",
            arclain_network::HttpRequest::get("https://example.com/"),
        ),
        Err(arclain_network::HttpError::PluginNetworkDisabled { .. })
    ));
}

#[test]
fn direct_gameta_metadata_route_requires_network_and_a_host_service_permit() {
    for (network, preconsume, should_call) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = async_client(&runtime);
        let mut capabilities = [PluginCapability::ArchiveMetadataRead]
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        if network {
            capabilities.insert(PluginCapability::Network);
        }
        let mut host = host_functions("direct-gameta-policy", capabilities, 1);
        host.set_async_http_client(client.clone());
        if preconsume {
            client
                .try_acquire_plugin_host_service("direct-gameta-policy", "gameta")
                .unwrap();
        }
        let called = AtomicBool::new(false);

        let result = host.with_authorized_gameta_request(|limit| {
            called.store(true, Ordering::SeqCst);
            limit
        });

        assert_eq!(called.load(Ordering::SeqCst), should_call);
        assert_eq!(result.is_some(), should_call);
    }
}

fn gameta_404_server() -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(750);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    observed.fetch_add(1, Ordering::SeqCst);
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
                        .unwrap();
                    let mut request = [0_u8; 2048];
                    let _ = stream.read(&mut request);
                    stream
                        .write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("local Gameta fixture failed: {error}"),
            }
        }
    });
    (format!("http://{address}"), requests, worker)
}

fn attach_gameta_fixture(
    host: &mut HostFunctions,
    runtime: &tokio::runtime::Runtime,
    server_url: String,
) -> Arc<arclain_network::AsyncHttpClient> {
    let policy_client = async_client(runtime);
    host.set_async_http_client(policy_client.clone());
    host.set_gameta_client(Arc::new(
        arclain_network::features::gameta_client::GametaClient::new(
            arclain_network::features::gameta_client::ServerConfig {
                url: server_url,
                api_key: None,
            },
        ),
    ));
    policy_client
}

#[cfg(feature = "gameta")]
#[test]
fn product_metadata_local_db_hit_precedes_cache_and_consumes_no_network_permit() {
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    let mut metadata =
        arclain_core::ProductMetadata::new(arclain_core::MetadataSource::DLSite, "RJ000001");
    metadata.title = Some("Local title".to_string());
    metadata.extras = serde_json::json!({"cover_image": "already-cached"});
    library.save_metadata(&metadata).unwrap();

    let cache = Arc::new(RecordingResolver::new(Some(b"should not be read".to_vec())));
    let capabilities = [
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::FileRead,
        PluginCapability::Network,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("local-first-db", capabilities, 1);
    host.set_library_service(library);
    host.data_service
        .register_resolver(arclain_data::DataSource::ContentCache, cache.clone());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server_url, server_requests, server) = gameta_404_server();
    let policy_client = attach_gameta_fixture(&mut host, &runtime, server_url);

    let result =
        Host::get_product_metadata(&mut host, "RJ000001".to_string(), "dlsite".to_string());
    server.join().unwrap();

    assert!(result.is_some());
    assert_eq!(cache.resolve_calls.load(Ordering::SeqCst), 0);
    assert_eq!(server_requests.load(Ordering::SeqCst), 0);
    policy_client
        .try_acquire_plugin_host_service("local-first-db", "gameta")
        .expect("local database hit must not consume a network permit");
}

#[cfg(feature = "gameta")]
#[test]
fn product_metadata_owned_cache_hit_precedes_gameta_and_consumes_no_network_permit() {
    let raw_json = br#"{"work_name":"Cached title","maker_name":"Cached circle"}"#.to_vec();
    let cache = Arc::new(RecordingResolver::new(Some(raw_json)));
    let capabilities = [
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::FileRead,
        PluginCapability::Network,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("local-first-cache", capabilities, 1);
    host.data_service
        .register_resolver(arclain_data::DataSource::ContentCache, cache.clone());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server_url, server_requests, server) = gameta_404_server();
    let policy_client = attach_gameta_fixture(&mut host, &runtime, server_url);

    let result =
        Host::get_product_metadata(&mut host, "RJ000001".to_string(), "dlsite".to_string());
    server.join().unwrap();

    assert!(result.is_some());
    assert_eq!(cache.resolve_calls.load(Ordering::SeqCst), 1);
    assert_eq!(server_requests.load(Ordering::SeqCst), 0);
    policy_client
        .try_acquire_plugin_host_service("local-first-cache", "gameta")
        .expect("plugin-owned cache hit must not consume a network permit");
}

#[cfg(feature = "gameta")]
#[test]
fn cached_metadata_migration_reads_only_the_calling_plugin_owner() {
    let (_root, cache, manager) = owner_cache_fixture();
    let host_owner = arclain_data::CacheOwner::host();
    let plugin_a = arclain_data::CacheOwner::plugin("plugin-a");
    let plugin_b = arclain_data::CacheOwner::plugin("plugin-b");
    let key = "dlsite:json:RJ000001";
    for (owner, title) in [
        (&host_owner, "Host title"),
        (&plugin_a, "Plugin A title"),
        (&plugin_b, "Plugin B title"),
    ] {
        let body = format!(r#"{{"work_name":"{title}","maker_name":"Circle"}}"#);
        cache
            .put_for_owner(owner, key, body.as_bytes(), CacheType::Metadata, None, None)
            .unwrap();
    }
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    let capabilities = [
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::ArchiveMetadataWrite,
        PluginCapability::FileRead,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("plugin-a", capabilities, 0);
    host.set_content_cache(cache);
    host.set_resource_manager(manager);
    host.set_library_service(library.clone());

    let returned =
        Host::get_product_metadata(&mut host, "RJ000001".to_string(), "dlsite".to_string())
            .expect("plugin-owned JSON should migrate");
    let returned: serde_json::Value = serde_json::from_str(&returned).unwrap();
    let persisted = library
        .get_metadata("dlsite:RJ000001")
        .unwrap()
        .expect("migrated metadata should persist");

    assert_eq!(returned["title"], "Plugin A title");
    assert_eq!(persisted.title.as_deref(), Some("Plugin A title"));
}

fn assert_product_metadata_input_rejected_before_network(
    plugin_id: &str,
    product_id: String,
    source: String,
) {
    let capabilities = [
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::FileRead,
        PluginCapability::Network,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions(plugin_id, capabilities, 1);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server_url, server_requests, server) = gameta_404_server();
    let policy_client = attach_gameta_fixture(&mut host, &runtime, server_url);

    let result = Host::get_product_metadata(&mut host, product_id, source);
    server.join().unwrap();

    assert!(result.is_none());
    assert_eq!(server_requests.load(Ordering::SeqCst), 0);
    policy_client
        .try_acquire_plugin_host_service(plugin_id, "gameta")
        .expect("rejected input must not consume a network permit");
}

#[test]
fn product_metadata_rejects_oversized_product_id_before_source_lookup() {
    assert_product_metadata_input_rejected_before_network(
        "bounded-product-id",
        "x".repeat(257),
        "dlsite".to_string(),
    );
}

#[test]
fn product_metadata_rejects_oversized_source_before_source_lookup() {
    assert_product_metadata_input_rejected_before_network(
        "bounded-product-source",
        "RJ000001".to_string(),
        "x".repeat(257),
    );
}

#[derive(Default)]
struct RecordingCacheIndex {
    deleted: AtomicBool,
}

impl CacheIndex for RecordingCacheIndex {
    fn upsert(
        &self,
        _key: &str,
        _product_id: Option<&str>,
        _content_hash: &str,
        _source_url: Option<&str>,
        _cache_type: CacheType,
        _size_bytes: Option<i64>,
    ) -> anyhow::Result<i64> {
        Ok(1)
    }

    fn get(&self, _key: &str) -> anyhow::Result<Option<CacheEntry>> {
        Ok(None)
    }

    fn has(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn delete(&self, _key: &str) -> anyhow::Result<bool> {
        self.deleted.store(true, Ordering::SeqCst);
        Ok(true)
    }

    fn delete_by_pattern(&self, _pattern: &str) -> anyhow::Result<usize> {
        self.deleted.store(true, Ordering::SeqCst);
        Ok(1)
    }

    fn update_last_accessed(&self, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct OwnerMapCacheIndex {
    entries: parking_lot::Mutex<HashMap<String, CacheEntry>>,
}

impl CacheIndex for OwnerMapCacheIndex {
    fn upsert(
        &self,
        key: &str,
        product_id: Option<&str>,
        content_hash: &str,
        source_url: Option<&str>,
        cache_type: CacheType,
        size_bytes: Option<i64>,
    ) -> anyhow::Result<i64> {
        let mut entries = self.entries.lock();
        let id = entries.len() as i64 + 1;
        entries.insert(
            key.to_string(),
            CacheEntry {
                id,
                key: key.to_string(),
                product_id: product_id.map(str::to_string),
                content_hash: content_hash.to_string(),
                source_url: source_url.map(str::to_string),
                cache_type,
                created_at: format!("{id:020}"),
                last_accessed: None,
                size_bytes,
            },
        );
        Ok(id)
    }

    fn get(&self, key: &str) -> anyhow::Result<Option<CacheEntry>> {
        Ok(self.entries.lock().get(key).cloned())
    }

    fn has(&self, key: &str) -> anyhow::Result<bool> {
        Ok(self.entries.lock().contains_key(key))
    }

    fn delete(&self, key: &str) -> anyhow::Result<bool> {
        Ok(self.entries.lock().remove(key).is_some())
    }

    fn delete_by_pattern(&self, pattern: &str) -> anyhow::Result<usize> {
        let prefix = pattern.strip_suffix('*').unwrap_or(pattern);
        let mut entries = self.entries.lock();
        let before = entries.len();
        entries.retain(|key, _| !key.starts_with(prefix));
        Ok(before - entries.len())
    }

    fn update_last_accessed(&self, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn entries_lru(&self) -> anyhow::Result<Vec<CacheEntry>> {
        let mut entries: Vec<_> = self.entries.lock().values().cloned().collect();
        entries.sort_by_key(|entry| entry.id);
        Ok(entries)
    }

    fn count_keys_with_prefix(
        &self,
        scoped_prefix: &str,
        cache_type: CacheType,
    ) -> anyhow::Result<u64> {
        Ok(self
            .entries
            .lock()
            .values()
            .filter(|entry| entry.key.starts_with(scoped_prefix) && entry.cache_type == cache_type)
            .count() as u64)
    }

    fn list_keys_with_prefix_page(
        &self,
        scoped_prefix: &str,
        cache_type: CacheType,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<String>> {
        let mut keys: Vec<_> = self
            .entries
            .lock()
            .values()
            .filter(|entry| entry.key.starts_with(scoped_prefix) && entry.cache_type == cache_type)
            .map(|entry| entry.key.clone())
            .collect();
        keys.sort();
        Ok(keys.into_iter().skip(offset).take(limit).collect())
    }
}

#[derive(Default)]
struct OversizedPageCacheIndex;

impl CacheIndex for OversizedPageCacheIndex {
    fn upsert(
        &self,
        _key: &str,
        _product_id: Option<&str>,
        _content_hash: &str,
        _source_url: Option<&str>,
        _cache_type: CacheType,
        _size_bytes: Option<i64>,
    ) -> anyhow::Result<i64> {
        Ok(1)
    }

    fn get(&self, _key: &str) -> anyhow::Result<Option<CacheEntry>> {
        Ok(None)
    }

    fn has(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn delete(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn delete_by_pattern(&self, _pattern: &str) -> anyhow::Result<usize> {
        Ok(0)
    }

    fn update_last_accessed(&self, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn list_keys_with_prefix_page(
        &self,
        scoped_prefix: &str,
        _cache_type: CacheType,
        _offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<String>> {
        Ok((limit > 0)
            .then(|| format!("{scoped_prefix}{}", "x".repeat(1024 * 1024 + 1)))
            .into_iter()
            .collect())
    }
}

#[derive(Default)]
struct OverdeliveringPageCacheIndex;

impl CacheIndex for OverdeliveringPageCacheIndex {
    fn upsert(
        &self,
        _key: &str,
        _product_id: Option<&str>,
        _content_hash: &str,
        _source_url: Option<&str>,
        _cache_type: CacheType,
        _size_bytes: Option<i64>,
    ) -> anyhow::Result<i64> {
        Ok(1)
    }

    fn get(&self, _key: &str) -> anyhow::Result<Option<CacheEntry>> {
        Ok(None)
    }

    fn has(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn delete(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn delete_by_pattern(&self, _pattern: &str) -> anyhow::Result<usize> {
        Ok(0)
    }

    fn update_last_accessed(&self, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn list_keys_with_prefix_page(
        &self,
        scoped_prefix: &str,
        _cache_type: CacheType,
        _offset: usize,
        _limit: usize,
    ) -> anyhow::Result<Vec<String>> {
        Ok((0..257)
            .map(|index| format!("{scoped_prefix}key-{index:03}"))
            .collect())
    }
}

// Tempdirs may sit on a small ramdisk; tests must not depend on the
// machine's free-space headroom.
fn test_cache_limits() -> arclain_data::CacheLimits {
    arclain_data::CacheLimits {
        min_free_space_bytes: 0,
        ..Default::default()
    }
}

fn owner_cache_fixture() -> (
    tempfile::TempDir,
    Arc<arclain_data::ContentCache>,
    Arc<arclain_data::ResourceManager>,
) {
    let root = tempfile::tempdir().unwrap();
    let index = Arc::new(OwnerMapCacheIndex::default());
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            root.path().join("cache"),
            index,
            test_cache_limits(),
        )
        .unwrap(),
    );
    let manager = Arc::new(arclain_data::ResourceManager::new(
        cache.clone(),
        arclain_data::ResourceConfig::default(),
    ));
    (root, cache, manager)
}

#[test]
fn host_cache_calls_are_confined_to_the_calling_plugin_owner() {
    let (_root, cache, manager) = owner_cache_fixture();
    let host_owner = arclain_data::CacheOwner::host();
    let plugin_a = arclain_data::CacheOwner::plugin("plugin-a");
    let plugin_b = arclain_data::CacheOwner::plugin("plugin-b");
    for (owner, body) in [
        (&host_owner, b"host".as_slice()),
        (&plugin_a, b"plugin-a".as_slice()),
        (&plugin_b, b"plugin-b".as_slice()),
    ] {
        cache
            .put_for_owner(owner, "shared:key", body, CacheType::Other, None, None)
            .unwrap();
        cache
            .put_for_owner(owner, "group:item", body, CacheType::Other, None, None)
            .unwrap();
    }

    let capabilities = [
        PluginCapability::FileRead,
        PluginCapability::FileWrite,
        PluginCapability::ArchiveMetadataWrite,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("plugin-a", capabilities, 0);
    host.set_content_cache(cache.clone());
    host.set_resource_manager(manager);

    assert!(Host::has_data(&mut host, "shared:key".to_string()));
    assert_eq!(
        Host::get_data(&mut host, "shared:key".to_string()).as_deref(),
        Some(b"plugin-a".as_slice())
    );

    assert!(Host::invalidate_cache(&mut host, "shared:key".to_string()));
    assert!(!cache.has_for_owner(&plugin_a, "shared:key").unwrap());
    assert!(cache.has_for_owner(&host_owner, "shared:key").unwrap());
    assert!(cache.has_for_owner(&plugin_b, "shared:key").unwrap());

    assert!(Host::invalidate_cache(&mut host, "group:*".to_string()));
    assert!(!cache.has_for_owner(&plugin_a, "group:item").unwrap());
    assert!(cache.has_for_owner(&host_owner, "group:item").unwrap());
    assert!(cache.has_for_owner(&plugin_b, "group:item").unwrap());
}

#[test]
fn host_cache_calls_require_file_write_for_persistent_data() {
    let mut host = host_functions("persistent-data-denied", Default::default(), 0);

    assert!(Host::put_data(&mut host, "state".to_string(), b"value".to_vec()).is_err());
    assert!(Host::data_key_count(&mut host, String::new()).is_err());
    assert!(Host::list_data_keys_page(&mut host, String::new(), 0, 1).is_err());
}

#[test]
fn host_cache_calls_write_and_page_only_the_calling_plugin_owner() {
    let (_root, cache, manager) = owner_cache_fixture();
    let host_owner = arclain_data::CacheOwner::host();
    let plugin_a = arclain_data::CacheOwner::plugin("plugin-a");
    let plugin_b = arclain_data::CacheOwner::plugin("plugin-b");
    cache
        .put_for_owner(
            &host_owner,
            "state:host",
            b"host",
            CacheType::Other,
            None,
            None,
        )
        .unwrap();
    cache
        .put_for_owner(
            &plugin_a,
            "dlsite:json:RJ000001",
            b"reserved",
            CacheType::Metadata,
            None,
            None,
        )
        .unwrap();
    cache
        .put_for_owner(
            &plugin_b,
            "state:other",
            b"other",
            CacheType::Other,
            None,
            None,
        )
        .unwrap();

    let capabilities = [PluginCapability::FileWrite, PluginCapability::FileRead]
        .into_iter()
        .collect();
    let mut host = host_functions("plugin-a", capabilities, 0);
    host.set_content_cache(cache);
    host.set_resource_manager(manager);

    Host::put_data(&mut host, "state:two".to_string(), b"two".to_vec()).unwrap();
    Host::put_data(&mut host, "state:one".to_string(), b"one".to_vec()).unwrap();
    Host::put_data(&mut host, "other:key".to_string(), b"other".to_vec()).unwrap();

    assert_eq!(Host::data_key_count(&mut host, String::new()), Ok(3));
    assert_eq!(
        Host::list_data_keys_page(&mut host, String::new(), 0, 256),
        Ok(vec![
            "other:key".to_string(),
            "state:one".to_string(),
            "state:two".to_string(),
        ])
    );
    assert_eq!(Host::data_key_count(&mut host, "state:".to_string()), Ok(2));
    assert_eq!(
        Host::list_data_keys_page(&mut host, "state:".to_string(), 0, 1),
        Ok(vec!["state:one".to_string()])
    );
    assert_eq!(
        Host::list_data_keys_page(&mut host, "state:".to_string(), 1, 1),
        Ok(vec!["state:two".to_string()])
    );
    assert_eq!(
        Host::get_data(&mut host, "state:one".to_string()),
        Some(b"one".to_vec())
    );
}

#[test]
fn host_cache_calls_reject_invalid_inputs_before_persistent_data_access() {
    let (_root, cache, manager) = owner_cache_fixture();
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("bounded-data", capabilities, 0);
    host.set_content_cache(cache);
    host.set_resource_manager(manager);

    for key in [
        "",
        ".",
        "..",
        "../outside",
        "directory/entry",
        "directory\\entry",
        "wild*card",
        "dlsite:json:RJ000001",
        "dlsite:HTML:RJ000001",
        "dlsite:metadata:RJ000001",
    ] {
        assert!(
            Host::put_data(&mut host, key.to_string(), b"value".to_vec()).is_err(),
            "key={key:?}"
        );
    }
    assert!(Host::put_data(&mut host, "k".repeat(513), b"value".to_vec()).is_err());
    assert!(Host::put_data(
        &mut host,
        "oversized-value".to_string(),
        vec![0; 4 * 1024 * 1024 + 1],
    )
    .is_err());

    for prefix in [
        "../outside",
        "directory/",
        "directory\\",
        "wild*",
        "dlsite:json:",
        "dlsite:HTML:",
        "dlsite:metadata:",
    ] {
        assert!(
            Host::data_key_count(&mut host, prefix.to_string()).is_err(),
            "prefix={prefix:?}"
        );
        assert!(
            Host::list_data_keys_page(&mut host, prefix.to_string(), 0, 1).is_err(),
            "prefix={prefix:?}"
        );
    }
    assert!(Host::data_key_count(&mut host, "p".repeat(513)).is_err());
    assert!(Host::list_data_keys_page(&mut host, "p".repeat(513), 0, 1).is_err());
    assert!(Host::list_data_keys_page(&mut host, String::new(), 0, 257).is_err());
}

#[test]
fn host_cache_calls_accept_exact_persistent_data_bounds_and_enforce_owner_quota() {
    let (_root, cache, manager) = owner_cache_fixture();
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("exact-data-bounds", capabilities, 0);
    host.set_content_cache(cache);
    host.set_resource_manager(manager);

    let boundary_key = "k".repeat(512);
    Host::put_data(&mut host, boundary_key.clone(), vec![0; 4 * 1024 * 1024]).unwrap();
    assert_eq!(Host::data_key_count(&mut host, String::new()), Ok(1));
    assert_eq!(
        Host::list_data_keys_page(&mut host, String::new(), 0, 256),
        Ok(vec![boundary_key])
    );

    let quota_root = tempfile::tempdir().unwrap();
    let quota_cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            quota_root.path().join("cache"),
            Arc::new(OwnerMapCacheIndex::default()),
            arclain_data::CacheLimits {
                max_owner_committed_bytes: 3,
                min_free_space_bytes: 0,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let mut quota_host = host_functions(
        "quota-data",
        [PluginCapability::FileWrite].into_iter().collect(),
        0,
    );
    quota_host.set_content_cache(quota_cache);
    assert!(Host::put_data(&mut quota_host, "state".to_string(), b"four".to_vec()).is_err());
}

#[test]
fn host_cache_calls_reject_oversized_returned_key_text() {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            root.path().join("cache"),
            Arc::new(OversizedPageCacheIndex),
            test_cache_limits(),
        )
        .unwrap(),
    );
    let mut host = host_functions(
        "bounded-page-text",
        [PluginCapability::FileWrite].into_iter().collect(),
        0,
    );
    host.set_content_cache(cache);

    assert!(Host::list_data_keys_page(&mut host, String::new(), 0, 1).is_err());
}

#[test]
fn host_cache_calls_reject_an_overdelivering_page_backend() {
    let root = tempfile::tempdir().unwrap();
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            root.path().join("cache"),
            Arc::new(OverdeliveringPageCacheIndex),
            test_cache_limits(),
        )
        .unwrap(),
    );
    let mut host = host_functions(
        "bounded-page-count",
        [PluginCapability::FileWrite].into_iter().collect(),
        0,
    );
    host.set_content_cache(cache);

    assert!(Host::list_data_keys_page(&mut host, String::new(), 0, 256).is_err());
}

#[test]
fn wildcard_invalidation_requires_metadata_write_and_remains_owner_scoped() {
    let (_root, cache, manager) = owner_cache_fixture();
    let host_owner = arclain_data::CacheOwner::host();
    let plugin_a = arclain_data::CacheOwner::plugin("plugin-a");
    let plugin_b = arclain_data::CacheOwner::plugin("plugin-b");
    let keys = [
        "group:ordinary:item",
        "group:json:item",
        "group:HTML:item",
        "group:metadata:item",
    ];
    for owner in [&host_owner, &plugin_a, &plugin_b] {
        for key in keys {
            cache
                .put_for_owner(owner, key, b"body", CacheType::Other, None, None)
                .unwrap();
        }
    }

    let mut file_write_only = host_functions(
        "plugin-a",
        [PluginCapability::FileWrite].into_iter().collect(),
        0,
    );
    file_write_only.set_content_cache(cache.clone());
    file_write_only.set_resource_manager(manager.clone());

    assert!(!Host::invalidate_cache(
        &mut file_write_only,
        "group:*".to_string()
    ));
    for key in keys {
        assert!(cache.has_for_owner(&plugin_a, key).unwrap(), "key={key}");
    }

    let mut metadata_writer = host_functions(
        "plugin-a",
        [
            PluginCapability::FileWrite,
            PluginCapability::ArchiveMetadataWrite,
        ]
        .into_iter()
        .collect(),
        0,
    );
    metadata_writer.set_content_cache(cache.clone());
    metadata_writer.set_resource_manager(manager);

    assert!(Host::invalidate_cache(
        &mut metadata_writer,
        "group:*".to_string()
    ));
    for key in keys {
        assert!(!cache.has_for_owner(&plugin_a, key).unwrap(), "key={key}");
        assert!(cache.has_for_owner(&host_owner, key).unwrap(), "key={key}");
        assert!(cache.has_for_owner(&plugin_b, key).unwrap(), "key={key}");
    }
}

#[test]
fn raw_metadata_classifier_is_ascii_case_consistent_without_percent_decoding() {
    for key in [
        "dlsite:json:RJ000001",
        "dlsite:JSON:RJ000001",
        "dlsite:HtMl:RJ000001",
        "dlsite:METADATA:RJ000001",
    ] {
        assert!(is_raw_metadata_cache_key(key), "key={key}");
    }
    for key in [
        "dlsite:%6ason:RJ000001",
        "dlsite:json",
        "json:RJ000001",
        "dlsite::json:RJ000001",
    ] {
        assert!(!is_raw_metadata_cache_key(key), "key={key}");
    }
}

#[test]
fn mixed_case_raw_metadata_reads_require_archive_metadata_read() {
    let resolver = Arc::new(RecordingResolver::new(Some(b"private metadata".to_vec())));
    let capabilities = [PluginCapability::FileRead].into_iter().collect();
    let mut host = host_functions("raw-case-read-policy", capabilities, 0);
    host.data_service
        .register_resolver(arclain_data::DataSource::ContentCache, resolver);

    for key in ["dlsite:JSON:RJ000001", "dlsite:HtMl:RJ000001"] {
        assert!(!Host::has_data(&mut host, key.to_string()), "key={key}");
        assert_eq!(
            Host::get_data(&mut host, key.to_string()),
            None,
            "key={key}"
        );

        let request = host
            .build_data_request(data_request(
                key,
                wirt::bindings::wirt::plugin::host::ResourceType::Binary,
                vec![wirt::bindings::wirt::plugin::host::DataSource::ContentCache],
            ))
            .expect_err("raw metadata source must be denied without metadata read");
        assert!(request.contains("no requested data source"));
    }
}

#[test]
fn mixed_case_raw_metadata_writeback_requires_archive_metadata_write() {
    use wirt::bindings::wirt::plugin::host::{DataSource, ResourceType};

    let capabilities = [
        PluginCapability::Network,
        PluginCapability::FileRead,
        PluginCapability::FileWrite,
        PluginCapability::ArchiveMetadataRead,
    ]
    .into_iter()
    .collect();
    let cache = Arc::new(RecordingResolver::new(None));
    let network = Arc::new(RecordingResolver::new(Some(b"raw metadata".to_vec())));
    let mut host = host_functions("raw-case-write-policy", capabilities, 0);
    host.data_service
        .register_resolver(arclain_data::DataSource::ContentCache, cache.clone());
    host.data_service
        .register_resolver(arclain_data::DataSource::Network, network);

    let request_id = Host::request_data(
        &mut host,
        data_request(
            "dlsite:JSON:RJ000001",
            ResourceType::Binary,
            vec![DataSource::ContentCache, DataSource::Network],
        ),
    );
    let _ = Host::poll_data(&mut host, request_id);

    assert_eq!(cache.store_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn mixed_case_raw_metadata_stream_and_delete_require_metadata_write() {
    use wirt::bindings::wirt::plugin::host::{DataSource, ResourceType};

    let cache_root = tempfile::tempdir().unwrap();
    let index = Arc::new(RecordingCacheIndex::default());
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            cache_root.path().join("cache"),
            index.clone(),
            test_cache_limits(),
        )
        .unwrap(),
    );
    let network = Arc::new(RecordingResolver::new(Some(b"raw metadata".to_vec())));
    let capabilities = [
        PluginCapability::Network,
        PluginCapability::FileWrite,
        PluginCapability::FileRead,
        PluginCapability::ArchiveMetadataRead,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("raw-case-stream-delete", capabilities, 0);
    host.set_content_cache(cache);
    host.data_service
        .register_resolver(arclain_data::DataSource::Network, network.clone());

    assert!(!Host::fetch_to_cache(
        &mut host,
        data_request(
            "dlsite:JSON:RJ000001",
            ResourceType::Binary,
            vec![DataSource::Network],
        )
    ));
    assert_eq!(network.resolve_calls.load(Ordering::SeqCst), 0);

    assert!(!Host::invalidate_cache(
        &mut host,
        "dlsite:JSON:*".to_string()
    ));
    assert!(!index.deleted.load(Ordering::SeqCst));
}

#[tracing_test::traced_test]
#[test]
fn cache_invalidation_global_trace_redacts_guest_key() {
    let marker = "guest-cache-key-must-not-reach-global-tracing";
    let cache_root = tempfile::tempdir().unwrap();
    let index = Arc::new(RecordingCacheIndex::default());
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            cache_root.path().join("cache"),
            index,
            test_cache_limits(),
        )
        .unwrap(),
    );
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("trace-redaction", capabilities, 0);
    host.set_content_cache(cache);

    let _ = Host::invalidate_cache(&mut host, marker.to_string());

    assert!(!logs_contain(marker));
}

#[tracing_test::traced_test]
#[test]
fn poll_data_global_trace_redacts_guest_request_id() {
    let marker = "guest-request-id-must-not-reach-global-tracing";
    let mut host = host_functions("poll-trace-redaction", Default::default(), 0);

    let _ = Host::poll_data(&mut host, marker.to_string());

    assert!(!logs_contain(marker));
}

#[test]
fn normal_plugins_cannot_request_background_external_launches() {
    let capabilities = [PluginCapability::FileRead].into_iter().collect();
    let mut host = host_functions("external-launch-denial", capabilities, 0);

    for extension in ["exe", "bat", "mp4"] {
        let error = Host::play_cached_blob(
            &mut host,
            "attacker-controlled-cache-key".to_string(),
            extension.to_string(),
        )
        .expect_err("plugin background launch must fail closed");
        assert_eq!(error, EXTERNAL_LAUNCH_DENIED, "extension={extension}");
    }
}

#[test]
fn play_cached_blob_requires_file_read_capability_before_stable_denial() {
    let mut host = host_functions("external-launch-capability-denial", Default::default(), 0);

    let error = Host::play_cached_blob(&mut host, "key".to_string(), "mp4".to_string())
        .expect_err("FileRead is required even though background launch is disabled");

    assert!(error.contains("FileRead capability not granted"));
}

#[test]
fn archive_path_requires_archive_metadata_read_capability() {
    let mut host = host_functions("archive-path-denial", Default::default(), 0);
    host.set_event_context(Some(EventContext {
        archive_path: "C:/private/library/secret.zip".to_string(),
        password: None,
        entries: Arc::new(Vec::new()),
        archive_session_id: 0,
    }));

    assert!(Host::current_archive_info(&mut host).is_none());
    assert!(Host::list_archive_files(&mut host).is_err());
}

#[cfg(feature = "gameta")]
#[test]
fn populated_library_queries_require_archive_metadata_read_capability() {
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    let mut metadata =
        arclain_core::ProductMetadata::new(arclain_core::MetadataSource::DLSite, "RJ000001");
    metadata.title = Some("Private metadata".to_string());
    library.save_metadata(&metadata).unwrap();

    let mut host = host_functions("populated-library-denial", Default::default(), 0);
    host.set_library_service(library);

    assert!(Host::list_cached_entries(&mut host).is_empty());
    assert!(Host::get_metadata_summaries(&mut host, vec!["RJ000001".to_string()]).is_empty());
    assert!(
        Host::get_product_metadata(&mut host, "RJ000001".to_string(), "dlsite".to_string())
            .is_none()
    );
}

#[test]
fn data_request_rejects_an_explicit_metadata_source_without_read_capability() {
    use wirt::bindings::wirt::plugin::host::{DataSource, DataStatus, ResourceType};

    let resolver = Arc::new(RecordingResolver::new(Some(b"private metadata".to_vec())));
    let mut host = host_functions("data-metadata-read-denial", Default::default(), 0);
    host.data_service
        .register_resolver(arclain_data::DataSource::MetadataStore, resolver.clone());

    let request_id = Host::request_data(
        &mut host,
        data_request(
            "dlsite:RJ000001",
            ResourceType::Json,
            vec![DataSource::MetadataCache],
        ),
    );
    let result = Host::poll_data(&mut host, request_id);

    assert_eq!(result.status, DataStatus::Failed);
    assert!(result.data.is_none());
    assert_eq!(resolver.resolve_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn data_cache_reads_require_file_read_and_hide_raw_metadata_keys() {
    let resolver = Arc::new(RecordingResolver::new(Some(b"cached bytes".to_vec())));
    let file_read = [PluginCapability::FileRead].into_iter().collect();
    let mut host = host_functions("data-content-read-policy", file_read, 0);
    host.data_service
        .register_resolver(arclain_data::DataSource::ContentCache, resolver);

    assert!(Host::has_data(
        &mut host,
        "ordinary:image:cover".to_string()
    ));
    assert_eq!(
        Host::get_data(&mut host, "ordinary:image:cover".to_string()),
        Some(b"cached bytes".to_vec())
    );
    for raw_metadata_key in ["dlsite:json:RJ000001", "dlsite:html:RJ000001"] {
        assert!(!Host::has_data(&mut host, raw_metadata_key.to_string()));
        assert_eq!(
            Host::get_data(&mut host, raw_metadata_key.to_string()),
            None
        );
    }

    let capabilities = [
        PluginCapability::FileRead,
        PluginCapability::ArchiveMetadataRead,
    ]
    .into_iter()
    .collect();
    let resolver = Arc::new(RecordingResolver::new(Some(b"allowed metadata".to_vec())));
    let mut allowed = host_functions("data-content-metadata-allow", capabilities, 0);
    allowed
        .data_service
        .register_resolver(arclain_data::DataSource::ContentCache, resolver);
    assert_eq!(
        Host::get_data(&mut allowed, "dlsite:json:RJ000001".to_string()),
        Some(b"allowed metadata".to_vec())
    );
}

#[test]
fn data_network_writeback_respects_metadata_and_file_write_capabilities() {
    use wirt::bindings::wirt::plugin::host::{DataSource, ResourceType};

    for (write_capability, cache_source, wit_source) in [
        (
            PluginCapability::ArchiveMetadataWrite,
            arclain_data::DataSource::MetadataStore,
            DataSource::MetadataCache,
        ),
        (
            PluginCapability::FileWrite,
            arclain_data::DataSource::ContentCache,
            DataSource::ContentCache,
        ),
    ] {
        for allow_write in [false, true] {
            let mut capabilities = [PluginCapability::Network]
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            if allow_write {
                capabilities.insert(write_capability);
            }
            let cache = Arc::new(RecordingResolver::new(None));
            let network = Arc::new(RecordingResolver::new(Some(b"network result".to_vec())));
            let mut host = host_functions("data-writeback-policy", capabilities, 0);
            host.data_service
                .register_resolver(cache_source, cache.clone());
            host.data_service
                .register_resolver(arclain_data::DataSource::Network, network);

            let request_id = Host::request_data(
                &mut host,
                data_request(
                    "policy-key",
                    if cache_source == arclain_data::DataSource::MetadataStore {
                        ResourceType::Json
                    } else {
                        ResourceType::Binary
                    },
                    vec![wit_source, DataSource::Network],
                ),
            );
            let _ = Host::poll_data(&mut host, request_id);

            assert_eq!(
                cache.store_calls.load(Ordering::SeqCst),
                usize::from(allow_write),
                "write capability {write_capability:?}, allow={allow_write}"
            );
        }
    }
}

#[test]
fn raw_metadata_content_cache_writes_require_archive_metadata_write() {
    use wirt::bindings::wirt::plugin::host::{DataSource, ResourceType};

    for allow_metadata_write in [false, true] {
        let mut capabilities = [
            PluginCapability::Network,
            PluginCapability::FileRead,
            PluginCapability::FileWrite,
            PluginCapability::ArchiveMetadataRead,
        ]
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
        if allow_metadata_write {
            capabilities.insert(PluginCapability::ArchiveMetadataWrite);
        }
        let cache = Arc::new(RecordingResolver::new(None));
        let network = Arc::new(RecordingResolver::new(Some(b"raw metadata".to_vec())));
        let mut host = host_functions("raw-metadata-write-policy", capabilities, 0);
        host.data_service
            .register_resolver(arclain_data::DataSource::ContentCache, cache.clone());
        host.data_service
            .register_resolver(arclain_data::DataSource::Network, network);

        let request_id = Host::request_data(
            &mut host,
            data_request(
                "dlsite:json:RJ000001",
                ResourceType::Json,
                vec![DataSource::ContentCache, DataSource::Network],
            ),
        );
        let _ = Host::poll_data(&mut host, request_id);

        assert_eq!(
            cache.store_calls.load(Ordering::SeqCst),
            usize::from(allow_metadata_write)
        );
    }
}

#[test]
fn fetch_to_cache_rejects_metadata_poison_before_resolver_side_effects() {
    use wirt::bindings::wirt::plugin::host::{DataSource, ResourceType};

    let capabilities = [PluginCapability::Network, PluginCapability::FileWrite]
        .into_iter()
        .collect();
    let network = Arc::new(RecordingResolver::new(Some(b"raw metadata".to_vec())));
    let mut host = host_functions("fetch-metadata-poison-denial", capabilities, 0);
    host.data_service
        .register_resolver(arclain_data::DataSource::Network, network.clone());

    assert!(!Host::fetch_to_cache(
        &mut host,
        data_request(
            "dlsite:json:RJ000001",
            ResourceType::Json,
            vec![DataSource::Network],
        )
    ));
    assert_eq!(network.resolve_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn data_api_guest_returns_are_bounded_below_hostcall_fuel() {
    use wirt::bindings::wirt::plugin::host::{DataSource, DataStatus, ResourceType};

    let oversized = vec![0xA5; crate::types::MAX_PLUGIN_GUEST_DATA_BYTES + 1];
    let capabilities = [PluginCapability::FileRead, PluginCapability::Network]
        .into_iter()
        .collect();
    let mut host = host_functions("guest-data-return-bound", capabilities, 0);
    host.data_service.register_resolver(
        arclain_data::DataSource::ContentCache,
        Arc::new(RecordingResolver::new(Some(oversized.clone()))),
    );
    host.data_service.register_resolver(
        arclain_data::DataSource::Network,
        Arc::new(RecordingResolver::new(Some(oversized))),
    );

    assert_eq!(Host::get_data(&mut host, "large-binary".to_string()), None);
    let request_id = Host::request_data(
        &mut host,
        data_request(
            "large-network",
            ResourceType::Binary,
            vec![DataSource::Network],
        ),
    );
    let result = Host::poll_data(&mut host, request_id);
    assert_eq!(result.status, DataStatus::Failed);
    assert!(result.data.is_none());
}

#[test]
fn default_metadata_request_omits_unauthorized_local_sources_but_keeps_network() {
    use wirt::bindings::wirt::plugin::host::ResourceType;

    let capabilities = [PluginCapability::Network].into_iter().collect();
    let host = host_functions("metadata-default-source-filter", capabilities, 0);

    let request = host
        .build_data_request(data_request("metadata-key", ResourceType::Json, Vec::new()))
        .expect("Network remains an authorized metadata source");

    assert_eq!(
        request.sources.iter().copied().collect::<Vec<_>>(),
        vec![arclain_data::DataSource::Network]
    );
    assert!(!request.allows_store_to(arclain_data::DataSource::MetadataStore));
    assert!(!request.allows_store_to(arclain_data::DataSource::ContentCache));
}

#[cfg(feature = "gameta")]
#[test]
fn cached_metadata_migration_persists_only_with_metadata_write_capability() {
    let raw_json = br#"{"work_name":"Cached title","maker_name":"Cached circle"}"#.to_vec();

    for allow_write in [false, true] {
        let library_root = tempfile::tempdir().unwrap();
        let library = Arc::new(
            arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite"))
                .unwrap(),
        );
        let mut capabilities = [
            PluginCapability::ArchiveMetadataRead,
            PluginCapability::FileRead,
        ]
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
        if allow_write {
            capabilities.insert(PluginCapability::ArchiveMetadataWrite);
        }
        let mut host = host_functions("metadata-migration-policy", capabilities, 0);
        host.set_library_service(library.clone());
        host.data_service.register_resolver(
            arclain_data::DataSource::ContentCache,
            Arc::new(RecordingResolver::new(Some(raw_json.clone()))),
        );

        let returned =
            Host::get_product_metadata(&mut host, "RJ000001".to_string(), "dlsite".to_string());

        assert!(
            returned.is_some(),
            "cached metadata read should still succeed"
        );
        assert_eq!(
            library.get_metadata("dlsite:RJ000001").unwrap().is_some(),
            allow_write,
            "read path persisted without ArchiveMetadataWrite={allow_write}"
        );
    }
}

#[test]
fn emit_metadata_requires_archive_metadata_write_capability() {
    let mut host = host_functions("metadata-write-denial", Default::default(), 0);
    let bridge = Arc::new(TestActiveTabBridge::default());
    host.set_active_tab_bridge(bridge.clone());
    host.set_event_context(Some(EventContext {
        archive_path: "test.zip".to_string(),
        password: None,
        entries: Arc::new(Vec::new()),
        archive_session_id: 1,
    }));

    Host::emit_metadata(
        &mut host,
        r#"{"product_id":"RJ000001","title":"unauthorized"}"#.to_string(),
    );

    assert!(
        bridge.metadata().is_none(),
        "unauthorized metadata write reached the host signal"
    );
}

#[cfg(feature = "gameta")]
#[test]
fn emit_metadata_rejects_oversized_json_and_product_ids_without_side_effects() {
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    let capabilities = [PluginCapability::ArchiveMetadataWrite]
        .into_iter()
        .collect();
    let mut host = host_functions("bounded-metadata-emission", capabilities, 0);
    host.set_library_service(library.clone());
    let bridge = Arc::new(TestActiveTabBridge::default());
    host.set_active_tab_bridge(bridge.clone());
    host.set_event_context(Some(EventContext {
        archive_path: "test.zip".to_string(),
        password: None,
        entries: Arc::new(Vec::new()),
        archive_session_id: 1,
    }));

    let oversized_json = serde_json::json!({
        "product_id": "RJ000001",
        "description": "x".repeat(4 * 1024 * 1024),
    })
    .to_string();
    Host::emit_metadata(&mut host, oversized_json);
    Host::emit_metadata(
        &mut host,
        serde_json::json!({"product_id": "R".repeat(257), "title": "oversized id"}).to_string(),
    );

    assert!(bridge.metadata().is_none());
    assert!(library
        .list_by_source(arclain_core::MetadataSource::DLSite)
        .unwrap()
        .is_empty());
}

#[test]
fn cache_invalidation_requires_file_write_capability() {
    let cache_root = tempfile::tempdir().unwrap();
    let index = Arc::new(RecordingCacheIndex::default());
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            cache_root.path().join("cache"),
            index.clone(),
            test_cache_limits(),
        )
        .unwrap(),
    );
    let mut host = host_functions("cache-delete-denial", Default::default(), 0);
    host.set_content_cache(cache);

    assert!(!Host::invalidate_cache(
        &mut host,
        "another-plugin:*".to_string()
    ));
    assert!(
        !index.deleted.load(Ordering::SeqCst),
        "unauthorized plugin reached the cache delete backend"
    );
}

#[test]
fn create_file_requires_file_write_capability() {
    let filename = format!(
        "arclain-unauthorized-create-{}-sentinel.txt",
        std::process::id()
    );
    let legacy_path = std::env::temp_dir().join(&filename);
    let result = {
        let mut host = host_functions("file-create-denial", Default::default(), 0);
        Host::create_file(&mut host, filename, b"unauthorized".to_vec())
    };
    let _ = std::fs::remove_file(&legacy_path);

    assert!(result.is_err(), "plugin without FileWrite created a file");
}

#[test]
fn temp_storage_is_lazy_and_denied_create_file_does_not_initialize_it() {
    let file_write = [PluginCapability::FileWrite].into_iter().collect();
    let write_capable_host = host_functions("lazy-temp-storage", file_write, 0);
    assert!(
        write_capable_host.temp_storage.is_none(),
        "loading an idle FileWrite plugin must not touch temporary storage"
    );

    let mut denied_host = host_functions("denied-lazy-temp-storage", Default::default(), 0);
    assert!(Host::create_file(
        &mut denied_host,
        "denied.txt".to_string(),
        b"denied".to_vec(),
    )
    .is_err());
    assert!(
        denied_host.temp_storage.is_none(),
        "a denied create_file must not initialize temporary storage"
    );
}

#[test]
fn create_file_never_reuses_a_plugin_supplied_name() {
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("file-collision-safety", capabilities, 0);

    let first =
        Host::create_file(&mut host, "same-name.txt".to_string(), b"first".to_vec()).unwrap();
    let second =
        Host::create_file(&mut host, "same-name.txt".to_string(), b"second".to_vec()).unwrap();
    let first_path = std::path::PathBuf::from(&first);
    let second_path = std::path::PathBuf::from(&second);
    let first_bytes = std::fs::read(&first_path).unwrap();
    let second_bytes = std::fs::read(&second_path).unwrap();
    let _ = std::fs::remove_file(&first_path);
    if second_path != first_path {
        let _ = std::fs::remove_file(&second_path);
    }

    assert_ne!(first_path, second_path, "second call reused the first path");
    assert_eq!(
        first_bytes, b"first",
        "second call overwrote the first file"
    );
    assert_eq!(second_bytes, b"second");
}

#[cfg(unix)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn create_file_ignores_a_predictable_shared_temp_collision() {
    let fixture = tempfile::tempdir().unwrap();
    let outside = fixture.path().join("outside.txt");
    std::fs::write(&outside, b"known-good").unwrap();
    let filename = format!("arclain-plugin-symlink-{}.txt", std::process::id());
    let predictable = std::env::temp_dir().join(&filename);
    assert!(
        !predictable.exists(),
        "predictable test path already exists"
    );
    std::fs::write(&predictable, b"shared-temp-collision").unwrap();

    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let result = {
        let mut host = host_functions("file-symlink-safety", capabilities, 0);
        Host::create_file(&mut host, filename, b"attacker".to_vec())
    };
    let _ = std::fs::remove_file(&predictable);

    assert!(
        result.is_ok(),
        "owned storage should avoid the external collision"
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"known-good");
}

#[cfg(unix)]
#[test]
fn create_file_skips_a_symlink_at_the_next_owned_name() {
    let fixture = tempfile::tempdir().unwrap();
    let outside = fixture.path().join("outside-owned.txt");
    std::fs::write(&outside, b"known-good").unwrap();
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("owned-name-symlink-safety", capabilities, 0);
    let first = std::path::PathBuf::from(
        Host::create_file(&mut host, "seed.txt".to_string(), b"seed".to_vec()).unwrap(),
    );
    let owned_root = first.parent().unwrap();
    let predicted = owned_root.join("0000000000000001-collision.txt");
    create_file_symlink(&outside, &predicted);

    let created = std::path::PathBuf::from(
        Host::create_file(
            &mut host,
            "collision.txt".to_string(),
            b"plugin-data".to_vec(),
        )
        .expect("create_new collision should advance to a fresh owned name"),
    );

    assert_ne!(created, predicted);
    assert_eq!(std::fs::read(&outside).unwrap(), b"known-good");
    assert_eq!(std::fs::read(created).unwrap(), b"plugin-data");
}

#[test]
fn create_file_skips_a_preexisting_owned_name_collision() {
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("owned-name-collision-safety", capabilities, 0);
    let first = std::path::PathBuf::from(
        Host::create_file(&mut host, "seed.txt".to_string(), b"seed".to_vec()).unwrap(),
    );
    let predicted = first
        .parent()
        .unwrap()
        .join("0000000000000001-collision.txt");
    std::fs::write(&predicted, b"pre-existing").unwrap();

    let created = std::path::PathBuf::from(
        Host::create_file(
            &mut host,
            "collision.txt".to_string(),
            b"plugin-data".to_vec(),
        )
        .expect("create_new collision should advance to a fresh owned name"),
    );

    assert_ne!(created, predicted);
    assert_eq!(std::fs::read(predicted).unwrap(), b"pre-existing");
    assert_eq!(std::fs::read(created).unwrap(), b"plugin-data");
}

#[test]
fn create_file_enforces_the_per_instance_file_count_quota() {
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("file-count-quota", capabilities, 0);
    let mut paths = Vec::new();

    for index in 0..128 {
        paths.push(
            Host::create_file(&mut host, format!("quota-{index}.txt"), vec![b'x'])
                .expect("file within count quota"),
        );
    }
    let over_limit = Host::create_file(&mut host, "quota-over.txt".to_string(), vec![b'x']);
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(path) = &over_limit {
        let _ = std::fs::remove_file(path);
    }

    assert!(
        over_limit.is_err(),
        "129th file exceeded the approved quota"
    );
}

#[test]
fn create_file_enforces_the_per_instance_byte_quota() {
    const MAX_PLUGIN_TEMP_BYTES: usize = 64 * 1024 * 1024;
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("file-byte-quota", capabilities, 0);

    let result = Host::create_file(
        &mut host,
        "oversized.bin".to_string(),
        vec![0; MAX_PLUGIN_TEMP_BYTES + 1],
    );
    if let Ok(path) = &result {
        let _ = std::fs::remove_file(path);
    }

    assert!(result.is_err(), "file exceeded the approved byte quota");
}

#[test]
fn plugin_owned_temp_directory_is_removed_when_host_state_drops() {
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let (file_path, owned_dir) = {
        let mut host = host_functions("file-drop-cleanup", capabilities, 0);
        let file_path = std::path::PathBuf::from(
            Host::create_file(&mut host, "cleanup.txt".to_string(), b"owned".to_vec()).unwrap(),
        );
        let owned_dir = file_path.parent().unwrap().to_path_buf();
        assert!(file_path.exists());
        (file_path, owned_dir)
    };

    if owned_dir == std::env::temp_dir() {
        let _ = std::fs::remove_file(&file_path);
    }
    assert_ne!(
        owned_dir,
        std::env::temp_dir(),
        "plugin file was written directly into the shared temp directory"
    );
    assert!(
        !owned_dir.exists(),
        "plugin-owned temp directory survived host unload/drop"
    );
}

#[cfg(feature = "gameta")]
#[test]
fn granted_archive_metadata_capabilities_reach_read_and_write_hostcalls() {
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    let mut metadata =
        arclain_core::ProductMetadata::new(arclain_core::MetadataSource::DLSite, "RJ000001");
    metadata.title = Some("Allowed metadata".to_string());
    library.save_metadata(&metadata).unwrap();

    let capabilities = [
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::ArchiveMetadataWrite,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("metadata-allow-path", capabilities, 0);
    host.set_library_service(library);
    let bridge = Arc::new(TestActiveTabBridge::default());
    host.set_active_tab_bridge(bridge.clone());
    host.set_event_context(Some(EventContext {
        archive_path: "C:/library/allowed.zip".to_string(),
        password: None,
        entries: Arc::new(Vec::new()),
        archive_session_id: 1,
    }));

    assert_eq!(
        Host::current_archive_info(&mut host)
            .expect("read capability should expose archive context")
            .path,
        "C:/library/allowed.zip"
    );
    assert!(Host::list_archive_files(&mut host).is_ok());
    assert_eq!(Host::list_cached_entries(&mut host), vec!["RJ000001"]);
    let summaries = Host::get_metadata_summaries(&mut host, vec!["RJ000001".to_string()]);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].title.as_deref(), Some("Allowed metadata"));
    assert!(
        Host::get_product_metadata(&mut host, "RJ000001".to_string(), "dlsite".to_string())
            .is_some()
    );

    Host::emit_metadata(
        &mut host,
        r#"{"product_id":"RJ000002","title":"Allowed write"}"#.to_string(),
    );
    assert_eq!(
        bridge
            .metadata()
            .and_then(|value| value["product_id"].as_str().map(str::to_owned))
            .as_deref(),
        Some("RJ000002")
    );
}

/// Regression coverage for the review finding that the panel-driven
/// (non-event-context) `emit_metadata` path silently dropped the write
/// whenever the active tab had no archive open, where the pre-decoupling
/// behavior wrote to the active tab's signal unconditionally. This test
/// covers the *first* branch of the fix: when the active tab *does* have
/// a session (`active_archive_session_id` is `Some`), the write must go
/// through `set_session_metadata` with that exact session id -- not
/// `set_active_tab_metadata`.
#[cfg(feature = "gameta")]
#[test]
fn panel_driven_emit_with_an_active_session_writes_via_set_session_metadata() {
    let capabilities = [PluginCapability::ArchiveMetadataWrite]
        .into_iter()
        .collect();
    let mut host = host_functions("metadata-panel-with-session", capabilities, 0);
    let bridge = Arc::new(TestActiveTabBridge::default());
    bridge.set_active_session_id(Some(77));
    host.set_active_tab_bridge(bridge.clone());
    // No event context installed: this is a panel/manual-emit call, not
    // one dispatched from inside a queued `PluginEvent::OnArchiveOpen`
    // handler.

    Host::emit_metadata(
        &mut host,
        r#"{"product_id":"RJ000003","title":"Panel write with session"}"#.to_string(),
    );

    let session_calls = bridge.session_metadata_calls();
    assert_eq!(session_calls.len(), 1);
    assert_eq!(session_calls[0].0, 77);
    assert_eq!(
        session_calls[0]
            .1
            .as_ref()
            .and_then(|value| value["product_id"].as_str()),
        Some("RJ000003")
    );
    assert!(
        bridge.active_tab_metadata_calls().is_empty(),
        "an active session must resolve via set_session_metadata, not the active-tab fallback"
    );
}

/// Second branch of the same fix: when the active tab has *no* archive
/// open (`active_archive_session_id` is `None`, the default), the write
/// must fall back to `set_active_tab_metadata` -- restoring the
/// pre-decoupling behavior -- rather than being silently dropped.
#[cfg(feature = "gameta")]
#[test]
fn panel_driven_emit_with_no_active_session_falls_back_to_set_active_tab_metadata() {
    let capabilities = [PluginCapability::ArchiveMetadataWrite]
        .into_iter()
        .collect();
    let mut host = host_functions("metadata-panel-without-session", capabilities, 0);
    let bridge = Arc::new(TestActiveTabBridge::default());
    host.set_active_tab_bridge(bridge.clone());
    // No event context, and no active session id -- exactly the case
    // that used to silently drop the write.

    Host::emit_metadata(
        &mut host,
        r#"{"product_id":"RJ000004","title":"Panel write without session"}"#.to_string(),
    );

    let fallback_calls = bridge.active_tab_metadata_calls();
    assert_eq!(fallback_calls.len(), 1);
    assert_eq!(
        fallback_calls[0]
            .as_ref()
            .and_then(|value| value["product_id"].as_str()),
        Some("RJ000004"),
        "the write must not be silently dropped when no archive session is active"
    );
    assert!(
        bridge.session_metadata_calls().is_empty(),
        "no active session must never resolve via set_session_metadata"
    );
}

#[cfg(feature = "gameta")]
#[test]
fn metadata_summary_accepts_max_external_id_after_dlsite_prefixing() {
    let external_id = "X".repeat(256);
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    let mut metadata =
        arclain_core::ProductMetadata::new(arclain_core::MetadataSource::DLSite, &external_id);
    metadata.title = Some("Maximum external id".to_string());
    library.save_metadata(&metadata).unwrap();

    let capabilities = [PluginCapability::ArchiveMetadataRead]
        .into_iter()
        .collect();
    let mut host = host_functions("metadata-summary-max-id", capabilities, 0);
    host.set_library_service(library);

    let summaries = Host::get_metadata_summaries(&mut host, vec![external_id.clone()]);

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, external_id);
    assert_eq!(summaries[0].title.as_deref(), Some("Maximum external id"));
}

#[cfg(feature = "gameta")]
#[test]
fn source_explicit_metadata_apis_page_count_summarize_and_record_provenance() {
    let library_root = tempfile::tempdir().unwrap();
    let library = Arc::new(
        arclain_core::LibraryService::new(&library_root.path().join("metadata.sqlite")).unwrap(),
    );
    for external_id in ["40", "41", "42"] {
        let mut metadata =
            arclain_core::ProductMetadata::new(arclain_core::MetadataSource::Steam, external_id);
        metadata.title = Some(format!("Steam {external_id}"));
        library.save_metadata(&metadata).unwrap();
    }

    let capabilities = [
        PluginCapability::ArchiveMetadataRead,
        PluginCapability::ArchiveMetadataWrite,
    ]
    .into_iter()
    .collect();
    let mut host = host_functions("steam-metadata", capabilities, 0);
    host.set_library_service(library.clone());

    assert_eq!(
        Host::cached_metadata_count(&mut host, "steam".to_string()).unwrap(),
        3
    );
    assert_eq!(
        Host::list_cached_metadata(&mut host, "steam".to_string(), 1, 2).unwrap(),
        vec!["41", "42"]
    );
    let summaries = Host::get_metadata_summaries_for_source(
        &mut host,
        "steam".to_string(),
        vec!["42".to_string()],
    )
    .unwrap();
    assert_eq!(summaries[0].title.as_deref(), Some("Steam 42"));

    assert!(Host::emit_metadata_for_source(
        &mut host,
        "steam".to_string(),
        r#"{"product_id":"99","source":"steam","title":"Emitted"}"#.to_string(),
    ));
    let emitted = library
        .get_metadata("steam:99")
        .unwrap()
        .expect("explicit-source emission persisted");
    assert_eq!(emitted.source, arclain_core::MetadataSource::Steam);
    assert_eq!(
        emitted.extras["_arclain"]["emitted_by_plugin"].as_str(),
        Some("steam-metadata")
    );
}

#[test]
fn source_explicit_metadata_write_rejects_a_spoofed_payload_source() {
    let capabilities = [PluginCapability::ArchiveMetadataWrite]
        .into_iter()
        .collect();
    let mut host = host_functions("source-spoof", capabilities, 0);

    assert!(!Host::emit_metadata_for_source(
        &mut host,
        "steam".to_string(),
        r#"{"product_id":"99","source":"dlsite","title":"Spoofed"}"#.to_string(),
    ));
}

#[derive(Default)]
struct TestActiveTabBridge {
    archive_path: parking_lot::Mutex<Option<String>>,
    metadata: parking_lot::Mutex<Option<serde_json::Value>>,
    /// Configurable return value for `active_archive_session_id` --
    /// `None` by default (every pre-existing test that never calls
    /// `set_active_session_id` keeps observing the old hardcoded-`None`
    /// behavior unchanged).
    active_session_id: parking_lot::Mutex<Option<u64>>,
    /// Every `(archive_session_id, metadata)` pair passed to
    /// `set_session_metadata`, in call order.
    session_metadata_calls: parking_lot::Mutex<Vec<(u64, Option<serde_json::Value>)>>,
    /// Every `metadata` value passed to `set_active_tab_metadata`, in
    /// call order.
    active_tab_metadata_calls: parking_lot::Mutex<Vec<Option<serde_json::Value>>>,
}

impl TestActiveTabBridge {
    fn metadata(&self) -> Option<serde_json::Value> {
        self.metadata.lock().clone()
    }

    #[cfg(feature = "gameta")]
    fn set_active_session_id(&self, id: Option<u64>) {
        *self.active_session_id.lock() = id;
    }

    #[cfg(feature = "gameta")]
    fn session_metadata_calls(&self) -> Vec<(u64, Option<serde_json::Value>)> {
        self.session_metadata_calls.lock().clone()
    }

    #[cfg(feature = "gameta")]
    fn active_tab_metadata_calls(&self) -> Vec<Option<serde_json::Value>> {
        self.active_tab_metadata_calls.lock().clone()
    }
}

impl ActiveTabBridge for TestActiveTabBridge {
    fn archive_path(&self) -> Option<String> {
        self.archive_path.lock().clone()
    }

    fn current_password(&self) -> Option<String> {
        None
    }

    fn archive_entries(&self) -> Vec<String> {
        Vec::new()
    }

    fn active_archive_session_id(&self) -> Option<u64> {
        *self.active_session_id.lock()
    }

    fn set_session_metadata(&self, archive_session_id: u64, metadata: Option<serde_json::Value>) {
        self.session_metadata_calls
            .lock()
            .push((archive_session_id, metadata.clone()));
        *self.metadata.lock() = metadata;
    }

    fn set_active_tab_metadata(&self, metadata: Option<serde_json::Value>) {
        self.active_tab_metadata_calls.lock().push(metadata.clone());
        *self.metadata.lock() = metadata;
    }

    fn set_archive_path(&self, path: Option<String>) {
        *self.archive_path.lock() = path;
    }
}

#[test]
fn granted_archive_modify_capability_reaches_rename_hostcall() {
    let fixture = tempfile::tempdir().unwrap();
    let original = fixture.path().join("original.zip");
    std::fs::write(&original, b"archive").unwrap();
    let bridge = Arc::new(TestActiveTabBridge::default());
    bridge.set_archive_path(Some(original.to_string_lossy().into_owned()));
    let capabilities = [PluginCapability::ArchiveModify].into_iter().collect();
    let mut host = host_functions("archive-modify-allow-path", capabilities, 0);
    host.set_active_tab_bridge(bridge.clone());

    let renamed = Host::rename_archive(&mut host, "renamed.zip".to_string()).unwrap();

    assert!(std::path::Path::new(&renamed).exists());
    assert_eq!(bridge.archive_path().as_deref(), Some(renamed.as_str()));
}

#[tracing_test::traced_test]
#[test]
fn archive_rename_global_trace_redacts_source_and_destination_paths() {
    let source_marker = "source-archive-path-must-not-reach-global-tracing";
    let destination_marker = "destination-archive-path-must-not-reach-global-tracing";
    let fixture = tempfile::tempdir().unwrap();
    let original = fixture.path().join(format!("{source_marker}.zip"));
    std::fs::write(&original, b"archive").unwrap();
    let bridge = Arc::new(TestActiveTabBridge::default());
    bridge.set_archive_path(Some(original.to_string_lossy().into_owned()));
    let capabilities = [PluginCapability::ArchiveModify].into_iter().collect();
    let mut host = host_functions("archive-trace-redaction", capabilities, 0);
    host.set_active_tab_bridge(bridge);

    Host::rename_archive(&mut host, format!("{destination_marker}.zip")).unwrap();

    assert!(!logs_contain(source_marker));
    assert!(!logs_contain(destination_marker));
}

#[test]
fn granted_file_write_capability_reaches_cache_and_file_hostcalls() {
    let cache_root = tempfile::tempdir().unwrap();
    let index = Arc::new(RecordingCacheIndex::default());
    let cache = Arc::new(
        arclain_data::ContentCache::new_with_limits(
            cache_root.path().join("cache"),
            index.clone(),
            test_cache_limits(),
        )
        .unwrap(),
    );
    let capabilities = [PluginCapability::FileWrite].into_iter().collect();
    let mut host = host_functions("file-write-allow-path", capabilities, 0);
    host.set_content_cache(cache);

    assert!(Host::invalidate_cache(
        &mut host,
        "owned-prefix:item".to_string()
    ));
    assert!(index.deleted.load(Ordering::SeqCst));
    let created =
        Host::create_file(&mut host, "allowed.txt".to_string(), b"allowed".to_vec()).unwrap();
    assert_eq!(std::fs::read(created).unwrap(), b"allowed");
}

#[test]
fn status_message_abi_does_not_retain_guest_memory() {
    let mut host = host_functions("bounded-status-message", Default::default(), 0);
    let mut message = String::with_capacity(1024 * 1024);
    message.push_str(&"🙂".repeat(2048));

    Host::set_status_message(&mut host, message);

    // No host state retains the guest-supplied message.
}
