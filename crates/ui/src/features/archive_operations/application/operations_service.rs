use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use crate::shared::SharedState;
use eframe::egui;

use super::conversion;
use super::drag_drop;
use super::extraction;

pub use super::file_opener::open_file_from_archive;

pub use super::organization::run_organization_plan;

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
        extraction::update_extraction_progress(&mut self.state, &self.shared, ctx);
    }

    /// Handle conversion progress updates
    pub fn update_conversion_progress(&mut self, ctx: &egui::Context) {
        conversion::update_conversion_progress(&mut self.state, &self.shared, ctx);
    }

    pub fn pause_extraction(&mut self) {
        extraction::pause_extraction(&mut self.state, &self.shared);
    }

    pub fn resume_extraction(&mut self) {
        extraction::resume_extraction(&mut self.state, &self.shared);
    }

    pub fn cancel_extraction(&mut self) {
        extraction::cancel_extraction(&mut self.state, &self.shared);
    }

    /// Handle drag progress updates
    pub fn update_drag_progress(&mut self, ctx: &egui::Context) {
        drag_drop::update_drag_progress(&mut self.state, &self.shared, ctx);
    }
}
