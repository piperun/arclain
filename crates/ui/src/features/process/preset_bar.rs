//! Preset dropdown + save/delete buttons at the top of the Process page.

use super::state::ProcessPageState;
use super::view::ProcessAction;
use crate::shared::SharedState;
use arclain_widgets::{ButtonSize, Text, TextButton, ThemedDropdown};
use eframe::egui;

pub fn render(
    ui: &mut egui::Ui,
    shared: &SharedState,
    state: &mut ProcessPageState,
) -> Option<ProcessAction> {
    let mut emitted: Option<ProcessAction> = None;

    ui.horizontal(|ui| {
        Text::new("Preset:").strong().show(ui);

        let selected_text = state
            .active_preset_name
            .clone()
            .unwrap_or_else(|| "— custom —".to_string());

        let presets_snapshot = state.presets().to_vec();

        ThemedDropdown::new("process_preset_dropdown", selected_text)
            .with_theme_colors(&shared.theme.colors)
            .width(200.0)
            .show_ui(ui, |ui| {
                for preset in &presets_snapshot {
                    if ui
                        .selectable_label(
                            state.active_preset_name.as_deref() == Some(&preset.name),
                            &preset.name,
                        )
                        .clicked()
                    {
                        // Applying takes the preset's steps and output
                        // settings and preserves the current input.
                        state.apply_preset(preset);
                    }
                }
            });

        if ui
            .add(
                TextButton::new(
                    format!("{} Save", egui_phosphor::regular::FLOPPY_DISK),
                    ButtonSize::Small,
                )
                .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            // Saving over the selected preset's own name is the point
            // of having it selected: the application upserts by name, so
            // this edits it in place instead of leaving a second entry
            // the dropdown renders identically. With nothing selected,
            // fall back to a timestamped name.
            let name = state.active_preset_name.clone().unwrap_or_else(|| {
                format!("Preset {}", chrono::Local::now().format("%Y-%m-%d %H:%M"))
            });
            emitted = Some(ProcessAction::SavePreset { name });
        }

        let active = state.active_preset_name.clone();
        if let Some(name) = active {
            if ui
                .add(
                    TextButton::new(
                        format!("{} Delete", egui_phosphor::regular::TRASH),
                        ButtonSize::Small,
                    )
                    .with_theme_colors(&shared.theme.colors),
                )
                .clicked()
            {
                emitted = Some(ProcessAction::DeletePreset { name });
            }
        }
    });

    emitted
}
