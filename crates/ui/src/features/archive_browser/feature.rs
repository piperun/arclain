use super::domain::Action;
use super::presentation::BrowserController;
use crate::shared::SharedState;
use eframe::egui;

pub struct ArchiveBrowser {
    pub controller: BrowserController,
}

impl ArchiveBrowser {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            controller: BrowserController::new(),
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) -> Action {
        super::presentation::views::browser_page::render_archive_browser(ctx, shared)
    }
}
