//! Per-step config widgets, over the application's own step vocabulary.

use crate::shared::SharedState;
use arclain_app::operations::pipeline::{CompressionLevelDto, PipelineStepDto};
use arclain_widgets::{Text, ThemedDropdown};
use eframe::egui;

/// The convert-format tokens the application accepts, paired with the
/// label the dropdown shows. `arclain_app::operations::pipeline`'s
/// `PipelineStepDto::Convert::format` is a string precisely so the
/// accepted vocabulary lives on one side of the boundary; these are the
/// two it recognizes.
const CONVERT_FORMATS: [(&str, &str); 2] = [("zip", ".zip"), ("7z", ".7z")];

pub fn render_flatten_config(ui: &mut egui::Ui, step: &mut PipelineStepDto) -> bool {
    let mut changed = false;
    if let PipelineStepDto::Flatten {
        strip_common_prefix,
        max_depth,
    } = step
    {
        if ui
            .checkbox(strip_common_prefix, "Strip common prefix")
            .changed()
        {
            changed = true;
        }

        // Simple two-state UI: recursive (0 = until stable) vs single-pass (1).
        // Presets can encode a specific cap via max_depth > 1; the UI preserves it
        // by only flipping between 0 and 1 based on the toggle.
        let mut recursive = *max_depth == 0;
        let tooltip = "Keep unpacking archives that appear after the first pass \
                       (e.g. outer .rar contains an inner .zip). Bounded by an \
                       internal safety cap.";
        if ui
            .checkbox(&mut recursive, "Recursive (unpack nested archives)")
            .on_hover_text(tooltip)
            .changed()
        {
            *max_depth = if recursive { 0 } else { 1 };
            changed = true;
        }
    }
    changed
}

/// Format + compression for a Convert step.
///
/// The pre-facade version of this widget also offered a "Password:"
/// field. It was inert in every configuration: `arclain_core`'s pipeline
/// executor binds that field `password: _` and never reads it, so the
/// typed secret was carried into a saved preset's JSON on disk and then
/// silently dropped at run time — a field that looked like it encrypted
/// the output and did not. The application's step DTO deliberately has
/// no counterpart (see `PipelineStepDto`'s own doc comment), so the
/// field is gone rather than kept as decoration.
pub fn render_convert_config(
    ui: &mut egui::Ui,
    shared: &SharedState,
    step: &mut PipelineStepDto,
) -> bool {
    let mut changed = false;
    if let PipelineStepDto::Convert {
        format,
        compression,
    } = step
    {
        ui.horizontal(|ui| {
            Text::new("Format:").strong().show(ui);
            let current = CONVERT_FORMATS
                .iter()
                .find(|(token, _)| token == format)
                .map(|(_, label)| *label)
                .unwrap_or(format.as_str());
            ThemedDropdown::new("pipeline_convert_format", current)
                .with_theme_colors(&shared.theme.colors)
                .show_ui(ui, |ui| {
                    for (token, label) in CONVERT_FORMATS {
                        if ui.selectable_label(format == token, label).clicked() {
                            *format = token.to_string();
                            changed = true;
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            Text::new("Compression:").strong().show(ui);
            let current = match compression {
                CompressionLevelDto::Fast => "Fast",
                CompressionLevelDto::Normal => "Normal",
                CompressionLevelDto::Max => "Max",
            };
            ThemedDropdown::new("pipeline_convert_compression", current)
                .with_theme_colors(&shared.theme.colors)
                .show_ui(ui, |ui| {
                    for (lvl, label) in [
                        (CompressionLevelDto::Fast, "Fast"),
                        (CompressionLevelDto::Normal, "Normal"),
                        (CompressionLevelDto::Max, "Max"),
                    ] {
                        if ui.selectable_value(compression, lvl, label).clicked() {
                            changed = true;
                        }
                    }
                });
        });
    }
    changed
}

pub fn render_organize_config(
    ui: &mut egui::Ui,
    step: &mut PipelineStepDto,
    rules: &[arclain_app::organization::OrganizationRuleSummary],
) -> bool {
    let mut changed = false;
    if let PipelineStepDto::Organize { rule_id } = step {
        ui.horizontal(|ui| {
            Text::new("Rule:").strong().show(ui);
            if super::rule_picker::render(ui, "pipeline_organize_rule", rules, rule_id) {
                changed = true;
            }
        });
    }
    changed
}
