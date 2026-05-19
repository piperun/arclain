//! Modal shown when the user has `drop_behavior = AskEachTime` and a
//! drop event fired without aiming at an overlay zone. Asks "open in
//! a new tab" or "replace the current tab" for the dropped files.
//!
//! Triggered by the drop-overlay routing in `dialog_handler.rs` when
//! the effective drop behavior resolves to `AskEachTime`. Until the
//! user picks, the dropped paths are held in this dialog's state and
//! NOT yet routed to tabs. On choice, the routing logic (the same
//! used by overlay zones) consumes the held paths.

use super::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use eframe::egui;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AskEachTimeDropState {
    pub show: bool,
    /// Paths waiting for a routing decision. Cleared on
    /// NewTab/Replace/Cancel.
    pub pending_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskEachTimeDropResult {
    None,
    NewTab,
    Replace,
    Cancel,
}

pub fn render_ask_each_time_drop_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &mut AskEachTimeDropState,
) -> AskEachTimeDropResult {
    if !state.show {
        return AskEachTimeDropResult::None;
    }
    if state.pending_paths.is_empty() {
        // Nothing to route — close the dialog.
        state.show = false;
        return AskEachTimeDropResult::None;
    }

    let params = ModalParams {
        width_frac: 0.40,
        height_frac: 0.25,
        min: egui::vec2(420.0, 200.0),
        max: egui::vec2(560.0, 280.0),
        bottom_bar_height: 48.0,
        ..Default::default()
    };

    let mut result = AskEachTimeDropResult::None;
    show_dimmed_modal(
        ctx,
        theme,
        "ask_each_time_drop",
        &params,
        |ui, _rect| {
            ui.label(
                egui::RichText::new("Open dropped file")
                    .size(18.0)
                    .color(theme.colors.on_surface)
                    .strong(),
            );
            ui.add_space(8.0);
            let primary_name = state.pending_paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unnamed>");
            let body = if state.pending_paths.len() == 1 {
                format!("Where should '{}' open?", primary_name)
            } else {
                format!(
                    "Where should '{}' (and {} more file{}) open?\n\n\
                     Subsequent files always open as new tabs regardless of this choice.",
                    primary_name,
                    state.pending_paths.len() - 1,
                    if state.pending_paths.len() == 2 { "" } else { "s" },
                )
            };
            ui.label(
                egui::RichText::new(&body)
                    .color(theme.colors.on_surface_variant),
            );
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let new_tab_btn = egui::Button::new(egui::RichText::new("New tab"))
                    .min_size(egui::vec2(110.0, 32.0));
                if ui.add(new_tab_btn).clicked() {
                    result = AskEachTimeDropResult::NewTab;
                    state.show = false;
                }
                ui.add_space(8.0);
                let replace_btn = egui::Button::new(egui::RichText::new("Replace current"))
                    .min_size(egui::vec2(140.0, 32.0));
                if ui.add(replace_btn).clicked() {
                    result = AskEachTimeDropResult::Replace;
                    state.show = false;
                }
                ui.add_space(8.0);
                let cancel_btn = egui::Button::new(egui::RichText::new("Cancel"))
                    .min_size(egui::vec2(80.0, 32.0));
                if ui.add(cancel_btn).clicked() {
                    result = AskEachTimeDropResult::Cancel;
                    state.show = false;
                    // pending_paths cleanup is the caller's responsibility
                    // (they mem::take after the renderer returns).
                }
            });
        },
    );
    result
}
