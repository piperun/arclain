//! Service layer for business logic
//!
//! Services wrap database operations with connection pool management,
//! matching the pattern used by `ChecksumService`.

mod cache_service;
mod config_service;
mod library_service;
mod manager;
mod merge_service;
mod network_proxy_persistence_service;
mod organization_service;
mod secrets_service;
mod ui_service;

pub use cache_service::CacheService;
pub use config_service::ConfigService;
pub use library_service::{
    LibraryService, MetadataSummary, METADATA_SUMMARY_MAX_IDS, METADATA_SUMMARY_MAX_ID_BYTES,
    METADATA_SUMMARY_MAX_STORED_ID_BYTES, METADATA_SUMMARY_TITLE_CHARS,
};
pub use manager::Services;
pub use merge_service::{
    CompressionLevel, MergeOptions, MergePhase, MergePreview, MergeProgress, MergeProgressCallback,
    MergeService, OutputFormat,
};
pub use network_proxy_persistence_service::{
    NetworkProxyPersistenceService, ProxyRecoveryOutcome, ProxySaveOutcome,
};
pub use organization_service::OrganizationService;
pub use secrets_service::SecretsService;
pub use ui_service::UiService;
