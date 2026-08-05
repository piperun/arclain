//! Shared context passed to the pipeline executor — carries the services
//! needed to resolve rules, metadata, and backends.

use crate::archive::ArchiveBackend;
use crate::features::pipeline::types::OutputCollisionPolicy;
#[cfg(feature = "gameta")]
use crate::services::LibraryService;
use crate::services::OrganizationService;
use anyhow::Result;
use arclain_db::SqliteDb;
use std::path::Path;
use std::sync::Arc;

/// Services + callbacks the executor needs to run a full pipeline.
#[derive(Clone)]
pub struct PipelineContext {
    pub organization_service: Option<Arc<OrganizationService>>,
    #[cfg(feature = "gameta")]
    pub library_service: Option<Arc<LibraryService>>,
    pub backend_for: Arc<dyn Fn(&Path) -> Result<Arc<dyn ArchiveBackend>> + Send + Sync>,
    /// Config DB handle for recording pipeline runs (dedup + audit). `None`
    /// disables persistence — the executor runs without consulting or writing
    /// history. Tests default to `None`.
    pub config_db: Option<Arc<SqliteDb>>,
    /// App-wide default collision policy from settings. `None` falls back to
    /// `OutputCollisionPolicy::Smart`. Per-pipeline overrides (set on the
    /// `Pipeline` struct) still take precedence over this.
    pub default_collision_policy: Option<OutputCollisionPolicy>,
}

impl PipelineContext {
    /// Construct a minimal context with only a backend selector.
    /// Suitable for tests and lightweight use.
    pub fn minimal(
        backend_for: impl Fn(&Path) -> Result<Arc<dyn ArchiveBackend>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            organization_service: None,
            #[cfg(feature = "gameta")]
            library_service: None,
            backend_for: Arc::new(backend_for),
            config_db: None,
            default_collision_policy: None,
        }
    }
}
