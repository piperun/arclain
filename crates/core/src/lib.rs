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
pub use features::conversion::{
    CompressionLevel, ConvertFormat, ConvertOptions, ModWarning, WarningKind,
};
pub use features::organization::{
    GameMetadata, MoveAction, OrganizationRule, RuleActions, RuleTrigger,
};
// Named by `PipelineContext::fetch_download`, so anyone composing a
// pipeline run has to be able to name it too.
pub use features::organization::engine::PendingDownload;
pub use features::pipeline::{
    builtin_presets, default_presets_path, execute_pipeline, load_presets, preview_pipeline,
    preview_pipeline_with_metadata, save_presets, OutputArtifact, OutputCollisionPolicy,
    OutputIdentity, OutputKind, Pipeline, PipelineContext, PipelineInput, PipelineOutput,
    PipelinePreview, PipelineProgress, PipelineStep, PresetsFile, PreviewEntry, ProcessPreset,
    SavedPreset, COLLISION_POLICY_CONFIG_KEY,
};
pub use services::{CacheService, ConfigService, OrganizationService, UiService};
#[cfg(feature = "gameta")]
pub use services::{
    LibraryService, MetadataSummary, METADATA_SUMMARY_MAX_IDS, METADATA_SUMMARY_MAX_ID_BYTES,
    METADATA_SUMMARY_MAX_STORED_ID_BYTES, METADATA_SUMMARY_TITLE_CHARS,
};
pub use utilities::init_logging;

// Re-export UI/DB types so consumers don't need to import arclain_db directly
pub use arclain_db::{ActionType, CacheType, DisplayMode, UiItem, UiRegion, UserConfig};
pub use arclain_db::{CacheEntry, ProductContent};
#[cfg(feature = "gameta")]
pub use arclain_db::{CompletenessScore, MetadataSource, ProductMetadata};

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
// and `services::cache_service`). Consumers reach this surface through
// us — no direct `arclain_data` dep needed.
//
// `CacheLimits` used to be re-exported here as well, so `crates/ui`'s
// image tests could build a cache with the free-space floor zeroed. No
// frontend builds a cache any more (images resolve through
// `arclain_app`'s image surface), so the only remaining consumers —
// `arclain_data`'s and `arclain_plugins`' own tests — name it on
// `arclain_data` directly, which both crates already depend on.
#[cfg(feature = "gameta")]
pub use arclain_data::MetadataReader;
pub use arclain_data::{
    CacheIndex, ContentCache, DataRequest, DataService, DataSource, DataSourceResolver,
    ResolveError, ResourceConfig, ResourceManager,
};
