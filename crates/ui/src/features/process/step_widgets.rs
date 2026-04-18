//! Per-step config widgets.

use crate::shared::SharedState;
use arclain_core::{CompressionLevel, ConvertFormat, PipelineStep};
use arclain_widgets::{Text, ThemedDropdown};
use eframe::egui;

pub fn render_flatten_config(ui: &mut egui::Ui, step: &mut PipelineStep) -> bool {
    let mut changed = false;
    if let PipelineStep::Flatten {
        strip_common_prefix,
    } = step
    {
        if ui
            .checkbox(strip_common_prefix, "Strip common prefix")
            .changed()
        {
            changed = true;
        }
    }
    changed
}

pub fn render_convert_config(
    ui: &mut egui::Ui,
    shared: &SharedState,
    step: &mut PipelineStep,
) -> bool {
    let mut changed = false;
    if let PipelineStep::Convert {
        format,
        compression,
        password,
    } = step
    {
        ui.horizontal(|ui| {
            Text::new("Format:").strong().show(ui);
            let current = format!(".{}", format.extension());
            ThemedDropdown::new("pipeline_convert_format", current)
                .with_theme_colors(&shared.theme.colors)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(format, ConvertFormat::Zip, ".zip")
                        .clicked()
                    {
                        changed = true;
                    }
                    if ui
                        .selectable_value(format, ConvertFormat::SevenZ, ".7z")
                        .clicked()
                    {
                        changed = true;
                    }
                });
        });
        ui.horizontal(|ui| {
            Text::new("Compression:").strong().show(ui);
            let current = match compression {
                CompressionLevel::Fast => "Fast",
                CompressionLevel::Normal => "Normal",
                CompressionLevel::Max => "Max",
            };
            ThemedDropdown::new("pipeline_convert_compression", current)
                .with_theme_colors(&shared.theme.colors)
                .show_ui(ui, |ui| {
                    for (lvl, label) in [
                        (CompressionLevel::Fast, "Fast"),
                        (CompressionLevel::Normal, "Normal"),
                        (CompressionLevel::Max, "Max"),
                    ] {
                        if ui.selectable_value(compression, lvl, label).clicked() {
                            changed = true;
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            Text::new("Password:").strong().show(ui);
            let mut pw = password.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut pw).password(true))
                .changed()
            {
                *password = if pw.is_empty() { None } else { Some(pw) };
                changed = true;
            }
        });
    }
    changed
}

pub fn render_organize_config(
    ui: &mut egui::Ui,
    step: &mut PipelineStep,
    rules: &[arclain_core::OrganizationRule],
) -> bool {
    let mut changed = false;
    if let PipelineStep::Organize { rule_id } = step {
        ui.horizontal(|ui| {
            Text::new("Rule:").strong().show(ui);
            if super::rule_picker::render(ui, "pipeline_organize_rule", rules, rule_id) {
                changed = true;
            }
        });
    }
    changed
}
