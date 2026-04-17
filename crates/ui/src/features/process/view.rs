//! Process page view — stub for Task 5, full layout in Task 6.

use super::state::ProcessPageState;
use crate::shared::SharedState;
use eframe::egui;

pub fn render(ctx: &egui::Context, _shared: &SharedState, state: &mut ProcessPageState) {
    state.refresh_preview();

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Process");
        ui.label("Pipeline builder (in progress)");
        ui.add_space(8.0);
        ui.label(format!("Steps: {}", state.pipeline.steps.len()));
    });
}
