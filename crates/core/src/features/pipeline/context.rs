//! Shared context passed to the pipeline executor — carries the services
//! needed to resolve rules, metadata, and backends.

use crate::archive::ArchiveBackend;
use crate::services::{LibraryService, OrganizationService};
use anyhow::Result;
use arclain_db::SqliteDb;
use std::path::Path;
use std::sync::Arc;

/// Services + callbacks the executor needs to run a full pipeline.
#[derive(Clone)]
pub struct PipelineContext {
    pub organization_service: Option<Arc<OrganizationService>>,
    pub library_service: Option<Arc<LibraryService>>,
    pub backend_for: Arc<dyn Fn(&Path) -> Result<Arc<dyn ArchiveBackend>> + Send + Sync>,
    /// Config DB handle for recording pipeline runs (dedup + audit). `None`
    /// disables persistence — the executor runs without consulting or writing
    /// history. Tests default to `None`.
    pub config_db: Option<Arc<SqliteDb>>,
}

impl PipelineContext {
    /// Construct a minimal context with only a backend selector.
    /// Suitable for tests and lightweight use.
    pub fn minimal(
        backend_for: impl Fn(&Path) -> Result<Arc<dyn ArchiveBackend>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            organization_service: None,
            library_service: None,
            backend_for: Arc::new(backend_for),
            config_db: None,
        }
    }
}
