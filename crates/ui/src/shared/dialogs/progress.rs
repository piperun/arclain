use super::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionProgressDialog {
    pub show: bool,
    pub title: String,
    pub file_action: String,
    pub percent: u8,
    pub processed_text: String,
    pub elapsed_text: String,
    pub time_left_text: String,
    pub status: ExtractionStatus,
    pub can_minimize: bool,
    pub can_pause: bool,
    pub can_cancel: bool,
    pub error: String,
    pub log_lines: Vec<String>,
    pub show_log: bool,
    /// Destination path for checksum verification
    pub dest_path: Option<std::path::PathBuf>,
    /// When this dialog's operation started -- the reference point
    /// `elapsed_text`/`time_left_text` are computed from (see
    /// `crate::core::operation_bridge::handle_extract_progress`). Set
    /// once, when the dialog is first shown for a fresh extraction; not
    /// itself rendered.
    pub started_at: Option<std::time::Instant>,
}

impl Default for ExtractionProgressDialog {
    fn default() -> Self {
        Self {
            show: false,
            title: "Extracting".to_string(),
            file_action: String::new(),
            percent: 0,
            processed_text: String::new(),
            elapsed_text: String::new(),
            time_left_text: String::new(),
            status: ExtractionStatus::Running,
            can_minimize: true,
            can_pause: true,
            can_cancel: true,
            error: String::new(),
            log_lines: Vec::new(),
            show_log: true,
            dest_path: None,
            started_at: None,
        }
    }
}

/// Single-signal container for all three progress-dialog flavours.
///
/// Pre-2026-05-20 each kind (`extraction`, `conversion`, `drag`) had
/// its own top-level `Signal<ExtractionProgressDialog>` in
/// `AppSignals`. Three signals → three independent subscriber notify
/// fanouts per progress tick, and the audit flagged it as copy-paste
/// duplication ("only one is ever visible at a time"). Collapse to a
/// single signal carrying all three slots: each kind keeps independent
/// state so a background drag can still update its dialog data while
/// an extraction dialog is showing — just consolidates the
/// subscription side.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProgressDialogs {
    pub extraction: ExtractionProgressDialog,
    pub conversion: ExtractionProgressDialog,
    pub drag: ExtractionProgressDialog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionDialogResult {
    None,
    Minimized,
    Paused,
    Resumed,
    Cancelled,
}

pub fn render_extraction_progress_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dlg: &mut ExtractionProgressDialog,
) -> Option<ExtractionDialogResult> {
    if !dlg.show {
        return None;
    }

    let params = ModalParams {
        width_frac: 0.5,
        height_frac: 0.45,
        min: egui::vec2(550.0, 350.0),
        max: egui::vec2(900.0, 600.0),
        padding: egui::vec2(18.0, 14.0),
        bottom_bar_height: 56.0,
        ..Default::default()
    };

    let mut result: ExtractionDialogResult = ExtractionDialogResult::None;

    show_dimmed_modal(
        ctx,
        theme,
        "extraction_progress",
        &params,
        |ui, _rect| {
            // Header
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&dlg.title)
                        .size(16.0)
                        .color(theme.colors.on_surface)
                        .strong(),
                );
            });
            ui.add_space(8.0);

            // Current file/action row
            if !dlg.file_action.is_empty() {
                ui.label(
                    egui::RichText::new(&dlg.file_action)
                        .size(14.0)
                        .color(theme.colors.on_surface_variant),
                );
            }

            ui.add_space(10.0);

            // Big progress bar like modern UX
            let pct = dlg.percent as f32 / 100.0;
            let pb = egui::ProgressBar::new(pct)
                .desired_width(ui.available_width())
                .text(format!("{}%", dlg.percent))
                .fill(theme.colors.primary)
                .animate(true);
            ui.add(pb);

            ui.add_space(6.0);

            // Details row
            ui.horizontal(|ui| {
                // `processed_text` is deliberately left blank for
                // facade-driven extraction (see `operation_bridge::
                // handle_extract_progress`'s own comment on why) -- a
                // blank *value* is fine, but the bare "Processed:"
                // label with nothing after it is not, so hide the
                // whole label rather than showing a dangling prefix.
                if !dlg.processed_text.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("Processed: {}", dlg.processed_text))
                            .color(theme.colors.on_surface_variant),
                    );
                    ui.add_space(12.0);
                }
                ui.label(
                    egui::RichText::new(format!("Elapsed: {}", dlg.elapsed_text))
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(12.0);
                if !dlg.time_left_text.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("Time left: {}", dlg.time_left_text))
                            .color(theme.colors.on_surface_variant),
                    );
                }
            });

            ui.add_space(8.0);
            let header = if dlg.show_log {
                "▼ Details"
            } else {
                "▶ Details"
            };
            if ui
                .button(
                    egui::RichText::new(header)
                        .strong()
                        .color(theme.colors.on_surface_variant),
                )
                .clicked()
            {
                dlg.show_log = !dlg.show_log;
            }
            ui.add_space(4.0);

            if dlg.show_log {
                let frame = egui::Frame::new()
                    .fill(theme.colors.surface_variant)
                    .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(8));
                frame.show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(200.0) // Limit height to prevent overflow
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width()); // Use full width
                            for line in &dlg.log_lines {
                                ui.label(
                                    egui::RichText::new(line)
                                        .color(theme.colors.on_surface_variant),
                                );
                            }
                        });
                });
            }

            if !dlg.error.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&dlg.error).color(egui::Color32::RED));
            }
        },
        |ui| {
            // Only show buttons if at least one action is available
            let any_button_enabled = dlg.can_cancel || dlg.can_pause || dlg.can_minimize;
            if !any_button_enabled {
                // Show a simple "Please wait..." message instead
                ui.with_layout(
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            egui::RichText::new("Please wait...")
                                .color(theme.colors.on_surface_variant),
                        );
                    },
                );
                return;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Cancel
                if dlg.can_cancel {
                    let cancel_enabled = matches!(
                        dlg.status,
                        ExtractionStatus::Running | ExtractionStatus::Paused
                    );
                    let cancel = egui::Button::new(egui::RichText::new("Cancel"))
                        .min_size(egui::vec2(100.0, 32.0));
                    if ui.add_enabled(cancel_enabled, cancel).clicked() {
                        result = ExtractionDialogResult::Cancelled;
                    }
                    ui.add_space(8.0);
                }

                // Pause/Resume
                if dlg.can_pause {
                    let pause_enabled = matches!(
                        dlg.status,
                        ExtractionStatus::Running | ExtractionStatus::Paused
                    );
                    let label = if dlg.status == ExtractionStatus::Paused {
                        "Resume"
                    } else {
                        "Pause"
                    };
                    let pause_btn = egui::Button::new(egui::RichText::new(label))
                        .min_size(egui::vec2(100.0, 32.0));
                    if ui.add_enabled(pause_enabled, pause_btn).clicked() {
                        result = if dlg.status == ExtractionStatus::Paused {
                            ExtractionDialogResult::Resumed
                        } else {
                            ExtractionDialogResult::Paused
                        };
                    }
                    ui.add_space(8.0);
                }

                // Minimize (background)
                if dlg.can_minimize {
                    let minimize_enabled = matches!(
                        dlg.status,
                        ExtractionStatus::Running | ExtractionStatus::Paused
                    );
                    let minimize_btn = egui::Button::new(egui::RichText::new("Minimize"))
                        .min_size(egui::vec2(112.0, 32.0));
                    if ui.add_enabled(minimize_enabled, minimize_btn).clicked() {
                        result = ExtractionDialogResult::Minimized;
                    }
                }
            });
        },
    );

    if result != ExtractionDialogResult::None {
        Some(result)
    } else {
        None
    }
}
