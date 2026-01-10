use super::{ArchiveBrowserAction, ArchiveBrowserState};
use crate::shared::SharedState;
use eframe::egui;

pub struct ArchiveBrowser {
    pub state: ArchiveBrowserState,
}

impl ArchiveBrowser {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            state: ArchiveBrowserState::default(),
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) -> ArchiveBrowserAction {
        crate::features::archive_browser::views::browser::render_archive_browser(
            ctx,
            &mut self.state,
            shared,
        )
    }

    pub fn state_mut(&mut self) -> &mut ArchiveBrowserState {
        &mut self.state
    }
}
