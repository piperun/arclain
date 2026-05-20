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
pub use config::{Config, ConfigStore, DropBehavior, PassRule};
pub use features::conversion::{
    CompressionLevel, ConvertFormat, ConvertOptions, ModWarning, WarningKind,
};
pub use features::organization::{
    GameMetadata, MoveAction, OrganizationRule, RuleActions, RuleTrigger,
};
pub use features::pipeline::{
    builtin_presets, default_presets_path, execute_pipeline, load_presets, preview_pipeline,
    preview_pipeline_with_metadata, save_presets, OutputArtifact, OutputCollisionPolicy,
    OutputIdentity, OutputKind, Pipeline, PipelineContext, PipelineInput, PipelineOutput,
    PipelinePreview, PipelineProgress, PipelineStep, PresetsFile, PreviewEntry, ProcessPreset,
    SavedPreset, COLLISION_POLICY_CONFIG_KEY,
};
pub use services::{CacheService, ConfigService, LibraryService, OrganizationService, UiService};
pub use utilities::{init_logging, FileOpener, OpenStrategy};

// Re-export UI/DB types so consumers don't need to import arclain_db directly
pub use arclain_db::{ActionType, CacheType, DisplayMode, UiItem, UiRegion, UserConfig};
pub use arclain_db::{CacheEntry, CompletenessScore, MetadataSource, ProductContent, ProductMetadata};

// Additional db re-exports for the UI / state layer. These were
// previously imported directly from `arclain_db` across `crates/ui`
// (audit: crate-boundary smell). Surfacing them through `arclain_core`
// gives the binary one entry point for persistence types.
pub use arclain_db::{
    delete_profile, get_config, list_interrupted_since, list_profiles, save_profile, set_config,
    set_default_profile, DbConnection, DbPassRule, SqliteDb,
};

// Cache + resource surface lives in `arclain_data`. The data crate
// declares `MetadataReader`/`CacheIndex` traits and we implement them
// on `LibraryService`/`CacheService` (see `services::library_service`
// and `services::cache_service`). UI consumers reach this surface
// through us — no direct `arclain_data` dep needed.
pub use arclain_data::{
    CacheIndex, ContentCache, DataRequest, DataService, DataSource, DataSourceResolver,
    MetadataReader, ResolveError, ResourceConfig, ResourceManager,
};
