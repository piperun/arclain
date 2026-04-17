//! Arclain Core - Archive management and organization
//!
//! This crate provides core functionality for archive operations, organized by feature:
//! - [`archive`]: Archive operations and management
//! - [`backends`]: Archive format implementations
//! - [`features::organization`]: Archive organization and restructuring
//! - [`config`]: Configuration and password management
//! - [`utilities`]: Shared utilities

pub mod archive;
pub mod backends;
pub mod config;
pub mod dirs;
pub mod features;
pub mod services;
pub mod utilities;

// Re-export commonly used types for convenience
pub use archive::{
    Archive, ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities,
    CancellationToken, EntryRef, ExtractionProgress, NavigationState, ProgressCallback,
};
pub use config::{open_databases, ConfigDb, ConfigDbs, DbPaths, SecretsDb, SecretsKey};
pub use config::{Config, ConfigStore, PassRule};
pub use features::conversion::{CompressionLevel, ConvertFormat, ConvertOptions};
pub use features::organization::{
    GameMetadata, MoveAction, OrganizationRule, RuleActions, RuleTrigger,
};
pub use features::pipeline::{
    execute_pipeline, preview_pipeline, Pipeline, PipelineContext, PipelineInput, PipelineOutput,
    PipelinePreview, PipelineProgress, PipelineStep, PreviewEntry, ProcessPreset,
};
pub use services::{CacheService, ConfigService, LibraryService, OrganizationService, UiService};
pub use utilities::{init_logging, FileOpener, OpenStrategy};

// Re-export UI/DB types so consumers don't need to import arclain_db directly
pub use arclain_db::{ActionType, CacheType, DisplayMode, UiItem, UiRegion, UserConfig};
pub use arclain_db::{CacheEntry, CompletenessScore, MetadataSource, ProductContent, ProductMetadata};
