//! Tab bar for the OrganizePanel: Preview / Variables / Network.
//!
//! Extracted from `mod.rs::render` so the top-level fn can stay
//! focused on validation + content dispatch.

use super::OrganizeTab;
use eframe::egui;

/// Render the tab bar and return `Some(tab)` if the user clicked a
/// non-active tab. The caller is responsible for committing the
/// switch (`ui_state.active_tab = tab`).
///
/// `network_log_count` controls the badge on the Network tab; pass
/// 0 to suppress the count.
pub fn render_tab_bar(
    ui: &mut egui::Ui,
    active: OrganizeTab,
    network_log_count: usize,
) -> Option<OrganizeTab> {
    let mut clicked = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

        if tab_button(ui, &preview_label(), OrganizeTab::Preview, active) {
            clicked = Some(OrganizeTab::Preview);
        }
        ui.add_space(16.0);

        if tab_button(ui, &variables_label(), OrganizeTab::Variables, active) {
            clicked = Some(OrganizeTab::Variables);
        }
        ui.add_space(16.0);

        if tab_button(
            ui,
            &network_label(network_log_count),
            OrganizeTab::NetworkActivity,
            active,
        ) {
            clicked = Some(OrganizeTab::NetworkActivity);
        }
    });

    clicked
}

fn preview_label() -> String {
    format!("{} Preview", egui_phosphor::regular::EYE)
}

fn variables_label() -> String {
    format!("{} Variables", egui_phosphor::regular::CODE)
}

fn network_label(count: usize) -> String {
    if count > 0 {
        format!("{} Network ({})", egui_phosphor::regular::GLOBE, count)
    } else {
        format!("{} Network", egui_phosphor::regular::GLOBE)
    }
}

fn tab_button(ui: &mut egui::Ui, label: &str, tab: OrganizeTab, active: OrganizeTab) -> bool {
    let is_active = tab == active;
    let text = egui::RichText::new(label).size(13.0).color(if is_active {
        ui.visuals().text_color()
    } else {
        ui.visuals().text_color().gamma_multiply(0.6)
    });
    ui.add(egui::Button::new(text).frame(false)).clicked()
}
