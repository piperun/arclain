//! Preset dropdown + save/delete buttons at the top of the Process page.

use super::state::ProcessPageState;
use crate::shared::SharedState;
use arclain_core::SavedPreset;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

pub fn render(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
    ui.horizontal(|ui| {
        ui.label("Preset:");

        let selected_text = state
            .active_preset_name
            .clone()
            .unwrap_or_else(|| "— custom —".to_string());

        let presets_snapshot = state.presets.clone();

        egui::ComboBox::from_id_salt("process_preset_dropdown")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for preset in &presets_snapshot {
                    if ui
                        .selectable_label(
                            state.active_preset_name.as_deref() == Some(&preset.name),
                            &preset.name,
                        )
                        .clicked()
                    {
                        // Apply preset: preserve current input, take steps + output
                        let current_input = state.pipeline.input.clone();
                        state.pipeline = preset.pipeline.clone();
                        state.pipeline.input = current_input;
                        state.active_preset_name = Some(preset.name.clone());
                        state.mark_dirty();
                    }
                }
            });

        if ui
            .add(
                TextButton::new("Save as...", ButtonSize::Small)
                    .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            let name = format!(
                "Preset {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            );
            state.presets.push(SavedPreset {
                name: name.clone(),
                pipeline: state.pipeline.clone(),
            });
            state.active_preset_name = Some(name);
            state.save_presets();
        }

        let active = state.active_preset_name.clone();
        if let Some(name) = active {
            if ui
                .add(
                    TextButton::new("Delete", ButtonSize::Small)
                        .with_theme_colors(&shared.theme.colors),
                )
                .clicked()
            {
                state.presets.retain(|p| p.name != name);
                state.active_preset_name = None;
                state.save_presets();
            }
        }
    });
}
