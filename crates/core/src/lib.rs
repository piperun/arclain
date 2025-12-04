//! Arclain Core - Archive management and organization
//!
//! This crate provides core functionality for archive operations, organized by feature:
//! - [`archive`]: Archive operations and management
//! - [`backends`]: Archive format implementations
//! - [`organization`]: Archive organization and restructuring
//! - [`config`]: Configuration and password management
//! - [`utilities`]: Shared utilities

pub mod archive;
pub mod backends;
pub mod config;
pub mod organization;
pub mod utilities;

// Re-export commonly used types for convenience
pub use archive::{
    Archive, ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities,
    NavigationState,
};
pub use config::{Config, ConfigStore, PassRule};
pub use config::{open_databases, ConfigDb, ConfigDbs, DbPaths, SecretsDb, SecretsKey};
pub use organization::{
    ExtractedMetadata, MoveFileRule, MoveRule, OrganizationRule, RuleActions, RuleTrigger,
};
pub use utilities::{init_logging, FileOpener, OpenStrategy};
