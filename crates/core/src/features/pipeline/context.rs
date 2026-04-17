//! Shared context passed to the pipeline executor — carries the services
//! needed to resolve rules, metadata, and backends.

use crate::archive::ArchiveBackend;
use crate::services::{LibraryService, OrganizationService};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// Services + callbacks the executor needs to run a full pipeline.
#[derive(Clone)]
pub struct PipelineContext {
    pub organization_service: Option<Arc<OrganizationService>>,
    pub library_service: Option<Arc<LibraryService>>,
    pub backend_for: Arc<dyn Fn(&Path) -> Result<Arc<dyn ArchiveBackend>> + Send + Sync>,
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
        }
    }
}
