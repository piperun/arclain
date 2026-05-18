//! Modal shown when closing a tab with in-flight operations.

use super::helpers::{show_dimmed_modal, ModalParams};
use crate::core::tabs::TabId;
use crate::shared::theme::AppTheme;
use eframe::egui;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloseTabConfirmState {
    pub show: bool,
    pub tab_id: Option<TabId>,
    pub tab_title: String,
    pub in_flight_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTabConfirmResult {
    None,
    Cancelled,
    Confirmed(TabId),
}

pub fn render_close_tab_confirm(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &mut CloseTabConfirmState,
) -> CloseTabConfirmResult {
    if !state.show {
        return CloseTabConfirmResult::None;
    }
    let Some(tab_id) = state.tab_id else {
        state.show = false;
        return CloseTabConfirmResult::None;
    };

    let params = ModalParams {
        width_frac: 0.35,
        height_frac: 0.20,
        min: egui::vec2(380.0, 160.0),
        max: egui::vec2(520.0, 220.0),
        bottom_bar_height: 48.0,
        ..Default::default()
    };

    let mut result = CloseTabConfirmResult::None;
    show_dimmed_modal(
        ctx,
        theme,
        "close_tab_confirm",
        &params,
        |ui, _rect| {
            ui.label(
                egui::RichText::new("Close tab?")
                    .size(18.0)
                    .color(theme.colors.on_surface)
                    .strong(),
            );
            ui.add_space(8.0);
            let msg = format!(
                "Tab '{}' has {} operation(s) in progress. Closing will cancel them.",
                state.tab_title, state.in_flight_count
            );
            ui.label(egui::RichText::new(&msg).color(theme.colors.on_surface_variant));
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close_btn = egui::Button::new(egui::RichText::new("Close"))
                    .min_size(egui::vec2(80.0, 32.0));
                if ui.add(close_btn).clicked() {
                    result = CloseTabConfirmResult::Confirmed(tab_id);
                    state.show = false;
                }
                ui.add_space(8.0);
                let cancel_btn = egui::Button::new(egui::RichText::new("Cancel"))
                    .min_size(egui::vec2(80.0, 32.0));
                if ui.add(cancel_btn).clicked() {
                    result = CloseTabConfirmResult::Cancelled;
                    state.show = false;
                }
            });
        },
    );
    result
}
