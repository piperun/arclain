use parking_lot::RwLock;
use std::sync::Arc;

use arclain_core::utilities::ChecksumService;
use arclain_core::{ConfigService, LibraryService, OrganizationService, UiService};
use arclain_data::{ContentCache, ResourceManager};
use arclain_http::features::whitelist::DomainWhitelist;
use arclain_http::AsyncHttpClient;
use arclain_plugins::PluginManager;
use parking_lot::Mutex;

#[allow(dead_code)] // Some fields are stored for future use in feature development
pub struct Services {
    pub tokio_runtime: tokio::runtime::Runtime,
    pub async_http_client: Arc<AsyncHttpClient>,
    pub domain_whitelist: Arc<RwLock<DomainWhitelist>>,

    // Services that might fail to init or depend on config
    pub plugin_manager: Option<Arc<Mutex<PluginManager>>>,
    pub plugin_event_sender: Option<std::sync::mpsc::Sender<arclain_plugins::PluginEvent>>,
    pub checksum_service: Option<Arc<ChecksumService>>,
    pub content_cache: Option<Arc<ContentCache>>,
    pub resource_manager: Option<Arc<ResourceManager>>,

    // Core domain services
    pub library_service: Option<Arc<LibraryService>>,
    pub organization_service: Option<Arc<OrganizationService>>,
    pub config_service: Option<Arc<ConfigService>>,
    pub ui_service: Option<Arc<UiService>>,
}

impl Services {
    #[allow(dead_code)] // Constructor kept for standalone testing/future use
    pub fn new(runtime: tokio::runtime::Runtime) -> Self {
        let domain_whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));

        // Initialize AsyncHttpClient
        let async_http_client = Arc::new(AsyncHttpClient::new(
            runtime.handle().clone(),
            domain_whitelist.clone(),
            None,
        ));

        Self {
            tokio_runtime: runtime,
            async_http_client,
            domain_whitelist,
            plugin_manager: None,
            plugin_event_sender: None,
            checksum_service: None,
            content_cache: None,
            resource_manager: None,
            library_service: None,
            organization_service: None,
            config_service: None,
            ui_service: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create test runtime")
    }

    #[test]
    fn test_services_new_creates_required_fields() {
        let runtime = create_test_runtime();
        let services = Services::new(runtime);

        // Required fields are always present
        // async_http_client is Arc - just verify it's accessible
        let _client_ref = &services.async_http_client;

        // domain_whitelist should be accessible
        let whitelist = services.domain_whitelist.read();
        assert!(whitelist.get_all_entries().is_empty());
    }

    #[test]
    fn test_services_optional_fields_default_to_none() {
        let runtime = create_test_runtime();
        let services = Services::new(runtime);

        // Optional fields default to None
        assert!(services.plugin_manager.is_none());
        assert!(services.plugin_event_sender.is_none());
        assert!(services.checksum_service.is_none());
        assert!(services.content_cache.is_none());
        assert!(services.resource_manager.is_none());
        assert!(services.library_service.is_none());
        assert!(services.organization_service.is_none());
        assert!(services.config_service.is_none());
        assert!(services.ui_service.is_none());
    }

    #[test]
    fn test_services_async_http_client_is_arc_shared() {
        let runtime = create_test_runtime();
        let services = Services::new(runtime);

        // Clone the Arc
        let client1 = services.async_http_client.clone();
        let client2 = services.async_http_client.clone();

        // Both should point to the same underlying client
        assert!(Arc::ptr_eq(&client1, &client2));
    }

    #[test]
    fn test_services_domain_whitelist_is_arc_rwlock_shared() {
        let runtime = create_test_runtime();
        let services = Services::new(runtime);

        // Clone the Arc
        let wl1 = services.domain_whitelist.clone();
        let wl2 = services.domain_whitelist.clone();

        // Both should point to the same underlying whitelist
        assert!(Arc::ptr_eq(&wl1, &wl2));

        // Modifications through one should be visible through the other
        wl1.write().add_pending("test_plugin", "example.com");
        let entries = wl2.read().get_all_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "example.com");
    }
}
