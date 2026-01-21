pub mod application;
pub mod domain;
pub mod presentation;

// Re-exports for backward compatibility and API surface
pub use application::operations_service::{
    open_file_from_archive, run_organization_plan, ArchiveOperations,
};
pub use domain::state::ArchiveOperationsState;

// Re-export presentation views if needed (though they should be used by controllers/app)
pub use presentation::controllers::operations_controller;
