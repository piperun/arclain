use crate::features::archive_operations::domain::state::ArchiveOperationsState;
use crate::shared::SharedState;
use eframe::egui;

use super::conversion;
use super::drag_drop;

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

    /// Handle conversion progress updates
    pub fn update_conversion_progress(&mut self, ctx: &egui::Context) {
        conversion::update_conversion_progress(&mut self.state, &self.shared, ctx);
    }

    /// Cancels the active tab's currently-running extraction, if any --
    /// the facade owns the CLI child process, so this asks
    /// `ArclainApp::cancel_operation` rather than killing a handle egui
    /// holds directly. See `crate::core::operations::extraction`.
    pub fn cancel_extraction(&mut self) {
        let tab = self.shared.signals().tabs.get().active().clone();
        crate::core::operations::extraction::cancel_extraction(&self.shared, &tab);
    }

    /// Handle drag progress updates
    pub fn update_drag_progress(&mut self, ctx: &egui::Context) {
        drag_drop::update_drag_progress(&mut self.state, &self.shared, ctx);
    }
}
