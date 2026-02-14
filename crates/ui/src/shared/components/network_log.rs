//! Network Activity Log Component
//!
//! A reusable component for displaying timestamped network activity logs.
//! Used in the organize panel and potentially other places where network
//! activity needs to be shown.

use eframe::egui::{self, RichText, Ui};
use std::time::SystemTime;

/// A single log entry with timestamp and message
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct NetworkLogEntry {
    pub time: SystemTime,
    pub message: String,
}

/// Renders a network activity log
pub struct NetworkLog;

#[allow(dead_code)]
impl NetworkLog {
    /// Render the network log as a full panel/section
    /// Takes a slice of log entries and renders them in a scrollable area
    pub fn render(ui: &mut Ui, entries: &[(SystemTime, String)]) {
        if entries.is_empty() {
            Self::render_empty_state(ui);
        } else {
            Self::render_entries(ui, entries);
        }
    }

    fn render_empty_state(ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new(egui_phosphor::regular::GLOBE)
                    .size(40.0)
                    .color(egui::Color32::from_rgb(100, 116, 139)),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("No network activity yet").size(14.0).weak());
            ui.label(
                RichText::new("Metadata fetching logs will appear here")
                    .size(11.0)
                    .weak(),
            );
        });
    }

    fn render_entries(ui: &mut Ui, entries: &[(SystemTime, String)]) {
        egui::ScrollArea::vertical()
            .id_salt("network_log_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);

                for (time, msg) in entries {
                    let time_str = chrono::DateTime::<chrono::Local>::from(*time)
                        .format("%H:%M:%S")
                        .to_string();

                    egui::Frame::NONE
                        .fill(ui.style().visuals.faint_bg_color)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .corner_radius(3.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(&time_str)
                                        .monospace()
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(100, 116, 139)),
                                );
                                // Use Label with wrap for full text
                                ui.add(egui::Label::new(RichText::new(msg).size(12.0)).wrap());
                            });
                        });
                }
            });
    }
}
