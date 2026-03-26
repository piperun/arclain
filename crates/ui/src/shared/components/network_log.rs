//! Network Activity Log Component
//!
//! A reusable component for displaying timestamped network activity logs
//! with severity coloring, filtering, and export.

use eframe::egui::{self, RichText, Ui};
use std::time::SystemTime;

/// Log entry severity, inferred from message content
#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Request,
    Success,
    Error,
    Info,
}

fn classify(msg: &str) -> Severity {
    let lower = msg.to_lowercase();
    if lower.contains("failed") || lower.contains("error") || lower.contains("invalidating") {
        Severity::Error
    } else if lower.starts_with("fetching")
        || lower.starts_with("get ")
        || lower.starts_with("searching")
        || lower.starts_with("trying")
    {
        Severity::Request
    } else if lower.contains("response")
        || lower.contains("parsed")
        || lower.contains("found")
        || lower.contains("extracted")
        || lower.contains("saved")
    {
        Severity::Success
    } else {
        Severity::Info
    }
}

/// Persistent state for the log page (lives in the content handler)
#[derive(Default)]
pub struct NetworkLogState {
    pub filter: String,
    pub show_requests: bool,
    pub show_success: bool,
    pub show_errors: bool,
    pub show_info: bool,
    pub auto_scroll: bool,
    initialized: bool,
}

impl NetworkLogState {
    pub fn new() -> Self {
        Self {
            show_requests: true,
            show_success: true,
            show_errors: true,
            show_info: true,
            auto_scroll: true,
            initialized: true,
            ..Default::default()
        }
    }

    fn ensure_init(&mut self) {
        if !self.initialized {
            *self = Self::new();
        }
    }
}

/// Renders a network activity log
pub struct NetworkLog;

impl NetworkLog {
    /// Render the full log page with toolbar and entries
    pub fn render_page(
        ui: &mut Ui,
        entries: &[(SystemTime, String)],
        state: &mut NetworkLogState,
        colors: &crate::shared::theme::ThemeColors,
    ) {
        state.ensure_init();

        // Toolbar
        ui.horizontal(|ui| {
            // Filter text
            ui.label(RichText::new(egui_phosphor::regular::FUNNEL).size(14.0));
            let filter_resp = ui.add(
                egui::TextEdit::singleline(&mut state.filter)
                    .hint_text("Filter logs...")
                    .desired_width(200.0),
            );
            if filter_resp.changed() {
                // Reset auto_scroll when filtering
                state.auto_scroll = false;
            }

            ui.separator();

            // Severity toggles
            Self::severity_toggle(ui, &mut state.show_requests, "REQ", colors.primary);
            Self::severity_toggle(ui, &mut state.show_success, "OK", egui::Color32::from_rgb(34, 197, 94));
            Self::severity_toggle(ui, &mut state.show_errors, "ERR", colors.error);
            Self::severity_toggle(ui, &mut state.show_info, "INFO", colors.on_surface_variant);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Export
                if ui
                    .button(
                        RichText::new(format!(
                            "{}  Export",
                            egui_phosphor::regular::CLIPBOARD_TEXT
                        ))
                        .size(12.0),
                    )
                    .clicked()
                {
                    let text = Self::format_for_export(entries);
                    ui.ctx().copy_text(text);
                }

                // Entry count
                let filtered = Self::filter_entries(entries, state);
                ui.label(
                    RichText::new(format!("{} / {} entries", filtered.len(), entries.len()))
                        .size(11.0)
                        .color(colors.on_surface_variant),
                );
            });
        });

        ui.add_space(4.0);

        // Entries
        let filtered = Self::filter_entries(entries, state);

        if filtered.is_empty() {
            if entries.is_empty() {
                Self::render_empty_state(ui);
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(RichText::new("No entries match the current filter").weak());
                });
            }
        } else {
            Self::render_entries(ui, &filtered, state, colors);
        }
    }

    /// Simple render (for embedding in tabs, no toolbar)
    pub fn render(ui: &mut Ui, entries: &[(SystemTime, String)]) {
        if entries.is_empty() {
            Self::render_empty_state(ui);
        } else {
            let default_colors = crate::shared::theme::ThemeColors::default();
            let mut state = NetworkLogState::new();
            let refs: Vec<&(SystemTime, String)> = entries.iter().collect();
            Self::render_entries(ui, &refs, &mut state, &default_colors);
        }
    }

    fn severity_toggle(ui: &mut Ui, enabled: &mut bool, label: &str, color: egui::Color32) {
        let text = if *enabled {
            RichText::new(label).size(10.0).strong().color(color)
        } else {
            RichText::new(label)
                .size(10.0)
                .color(egui::Color32::from_rgb(80, 80, 80))
        };
        if ui.add(egui::Button::new(text).frame(false)).clicked() {
            *enabled = !*enabled;
        }
    }

    fn filter_entries<'a>(
        entries: &'a [(SystemTime, String)],
        state: &NetworkLogState,
    ) -> Vec<&'a (SystemTime, String)> {
        let filter_lower = state.filter.to_lowercase();
        entries
            .iter()
            .filter(|(_, msg)| {
                // Severity filter
                let sev = classify(msg);
                let pass_severity = match sev {
                    Severity::Request => state.show_requests,
                    Severity::Success => state.show_success,
                    Severity::Error => state.show_errors,
                    Severity::Info => state.show_info,
                };
                if !pass_severity {
                    return false;
                }
                // Text filter
                if !filter_lower.is_empty() && !msg.to_lowercase().contains(&filter_lower) {
                    return false;
                }
                true
            })
            .collect()
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
                RichText::new("Fetch metadata to see activity logs here")
                    .size(11.0)
                    .weak(),
            );
        });
    }

    fn render_entries(
        ui: &mut Ui,
        entries: &[&(SystemTime, String)],
        state: &mut NetworkLogState,
        colors: &crate::shared::theme::ThemeColors,
    ) {
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("network_log_scroll")
            .auto_shrink([false, false]);

        if state.auto_scroll {
            scroll = scroll.stick_to_bottom(true);
        }

        scroll.show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);

            for (time, msg) in entries {
                let time_str = chrono::DateTime::<chrono::Local>::from(*time)
                    .format("%H:%M:%S")
                    .to_string();

                let severity = classify(msg);
                let (indicator_color, msg_color) = match severity {
                    Severity::Request => (colors.primary, colors.on_surface),
                    Severity::Success => (
                        egui::Color32::from_rgb(34, 197, 94),
                        colors.on_surface,
                    ),
                    Severity::Error => (colors.error, colors.error),
                    Severity::Info => (colors.on_surface_variant, colors.on_surface_variant),
                };

                egui::Frame::NONE
                    .fill(ui.style().visuals.faint_bg_color)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            // Color indicator dot
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(4.0, 14.0),
                                egui::Sense::hover(),
                            );
                            ui.painter()
                                .rect_filled(rect, 2.0, indicator_color);

                            ui.add_space(6.0);

                            // Timestamp
                            ui.label(
                                RichText::new(&time_str)
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(100, 116, 139)),
                            );

                            ui.add_space(8.0);

                            // Message (wrapping)
                            ui.add(
                                egui::Label::new(
                                    RichText::new(msg.as_str()).size(11.0).color(msg_color),
                                )
                                .wrap(),
                            );
                        });
                    });
            }
        });
    }

    fn format_for_export(entries: &[(SystemTime, String)]) -> String {
        let mut out = String::with_capacity(entries.len() * 80);
        for (time, msg) in entries {
            let time_str = chrono::DateTime::<chrono::Local>::from(*time)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            out.push_str(&time_str);
            out.push_str("  ");
            out.push_str(msg);
            out.push('\n');
        }
        out
    }
}
