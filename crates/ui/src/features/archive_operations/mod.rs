pub mod actions;
pub mod operations;
pub mod state;

pub use operations::open_file_from_archive;
pub use operations::run_organization_plan;
pub use state::ArchiveOperationsState;

use crate::shared::SharedState;

pub struct ArchiveOperations {
    state: ArchiveOperationsState,
    shared: SharedState,
}

impl ArchiveOperations {
    pub fn new(shared: &SharedState) -> Self {
        Self {
            state: ArchiveOperationsState::default(),
            shared: shared.clone(),
        }
    }

    pub fn state_mut(&mut self) -> &mut ArchiveOperationsState {
        &mut self.state
    }

    /// Handle extraction progress updates
    pub fn update_extraction_progress(&mut self, ctx: &egui::Context) {
        operations::update_extraction_progress(&mut self.state, &self.shared, ctx);
    }

    /// Handle conversion progress updates
    pub fn update_conversion_progress(&mut self, ctx: &egui::Context) {
        operations::update_conversion_progress(&mut self.state, &self.shared, ctx);
    }

    pub fn pause_extraction(&mut self) {
        operations::pause_extraction(&mut self.state, &self.shared);
    }

    pub fn resume_extraction(&mut self) {
        operations::resume_extraction(&mut self.state, &self.shared);
    }

    pub fn cancel_extraction(&mut self) {
        operations::cancel_extraction(&mut self.state, &self.shared);
    }

    /// Handle drag progress updates
    pub fn update_drag_progress(&mut self, ctx: &egui::Context) {
        operations::update_drag_progress(&mut self.state, &self.shared, ctx);
    }
}
