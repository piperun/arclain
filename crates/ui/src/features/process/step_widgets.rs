//! Per-step config widgets.

use arclain_core::{CompressionLevel, ConvertFormat, PipelineStep};
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

pub fn render_convert_config(ui: &mut egui::Ui, step: &mut PipelineStep) -> bool {
    let mut changed = false;
    if let PipelineStep::Convert {
        format,
        compression,
        password,
    } = step
    {
        ui.horizontal(|ui| {
            ui.label("Format:");
            egui::ComboBox::from_id_salt("pipeline_convert_format")
                .selected_text(format!(".{}", format.extension()))
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
            ui.label("Compression:");
            egui::ComboBox::from_id_salt("pipeline_convert_compression")
                .selected_text(match compression {
                    CompressionLevel::Fast => "Fast",
                    CompressionLevel::Normal => "Normal",
                    CompressionLevel::Max => "Max",
                })
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
            ui.label("Password:");
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

pub fn render_organize_config(ui: &mut egui::Ui, step: &mut PipelineStep) -> bool {
    let mut changed = false;
    if let PipelineStep::Organize { rule_id } = step {
        ui.horizontal(|ui| {
            ui.label("Rule ID:");
            if ui.add(egui::DragValue::new(rule_id).speed(1)).changed() {
                changed = true;
            }
        });
        ui.label(
            egui::RichText::new(
                "Rule picker integration pending — enter ID manually for now.",
            )
            .size(10.0)
            .weak(),
        );
    }
    changed
}
