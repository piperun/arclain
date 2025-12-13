pub mod navigation;
pub mod state;
pub mod ui;

pub use state::ArchiveBrowserState;

use crate::shared::SharedState;

pub struct ArchiveBrowser {
    state: ArchiveBrowserState,
}

impl ArchiveBrowser {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            state: ArchiveBrowserState::default(),
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) -> ArchiveBrowserAction {
        ui::render_archive_browser(ctx, &mut self.state, shared)
    }

    pub fn state_mut(&mut self) -> &mut ArchiveBrowserState {
        &mut self.state
    }
}

pub enum ArchiveBrowserAction {
    None,
    NavigateToFolder(String),
    NavigateToPath(String),
    OpenFile(String),
    OpenArchiveInTab(String), // Double-click on nested archive
    EditFile(String),
    DeleteFile(String),
    Organize,
    Metadata(String), // Metadata JSON from plugin
}
