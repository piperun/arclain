pub mod operations;
pub mod state;

pub use state::ArchiveOperationsState;

use crate::shared::SharedState;

pub struct ArchiveOperations {
    state: ArchiveOperationsState,
}

impl ArchiveOperations {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            state: ArchiveOperationsState::default(),
        }
    }

    pub fn state_mut(&mut self) -> &mut ArchiveOperationsState {
        &mut self.state
    }

    /// Handle extraction progress updates
    pub fn update_extraction_progress(&mut self, ctx: &egui::Context) {
        operations::update_extraction_progress(&mut self.state, ctx);
    }

    /// Handle conversion progress updates
    pub fn update_conversion_progress(&mut self, ctx: &egui::Context) {
        operations::update_conversion_progress(&mut self.state, ctx);
    }

    pub fn pause_extraction(&mut self) {
        operations::pause_extraction(&mut self.state);
    }

    pub fn resume_extraction(&mut self) {
        operations::resume_extraction(&mut self.state);
    }

    pub fn cancel_extraction(&mut self) {
        operations::cancel_extraction(&mut self.state);
    }
}
