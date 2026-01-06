//! Service layer for business logic
//!
//! Services wrap database operations with connection pool management,
//! matching the pattern used by `ChecksumService`.

mod config_service;
mod library_service;
mod organization_service;
mod ui_service;

pub use config_service::ConfigService;
pub use library_service::LibraryService;
pub use organization_service::OrganizationService;
pub use ui_service::UiService;
