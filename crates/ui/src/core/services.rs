use arclain_core::services::Services as CoreServices;
use arclain_core::{ContentCache, ResourceManager};
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::ops::Deref;
use std::sync::Arc;

/// UI-layer Services container
/// Wraps CoreServices and adds UI-specific services (PluginManager).
/// Cache + ResourceManager live here too because they aren't part of
/// the headless `arclain_core::services::Services` bag.
///
/// At real startup, every field here is populated from
/// `arclain_app::ArclainApp::take_legacy_composition` (see
/// `crate::core::state::init::AppState::new`) rather than computed
/// inline -- `ArclainApp::bootstrap` now owns that composition. `new`
/// below stays a direct constructor purely for feature-level UI tests
/// that want a minimal `Services` without paying for a full bootstrap.
pub struct Services {
    pub core: CoreServices,

    pub plugin_manager: Option<Arc<Mutex<PluginManager>>>,
    pub content_cache: Option<Arc<ContentCache>>,
    pub resource_manager: Option<Arc<ResourceManager>>,
}

impl Default for Services {
    fn default() -> Self {
        panic!("Services cannot be default-constructed without runtime");
    }
}

impl Services {
    /// Create new UI services wrapper with clean core services
    /// Mostly used for testing
    pub fn new(runtime: tokio::runtime::Runtime) -> Self {
        Self {
            core: CoreServices::new(Arc::new(runtime)),
            plugin_manager: None,
            content_cache: None,
            resource_manager: None,
        }
    }
}

impl Deref for Services {
    type Target = CoreServices;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}
