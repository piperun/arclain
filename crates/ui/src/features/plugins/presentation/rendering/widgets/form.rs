//! Interactive input widgets — user changes a value, the plugin gets
//! an event back through `ctx.event_callback`.

use super::super::context::{RenderContext, UiEventHandler};
use crate::shared::components::settings_form::SettingsRow;
use arclain_plugins::types::ButtonAction;
use arclain_widgets::{TextInput, ThemedDropdown, ThemedSlider};
use eframe::egui;

pub fn render_button(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    action: &Option<ButtonAction>,
) {
    let colors = ctx.colors;
    if ui
        .add(
            arclain_widgets::TextButton::new(label, arclain_widgets::ButtonSize::Small)
                .with_theme_colors(colors),
        )
        .clicked()
    {
        match action.as_ref().unwrap_or(&ButtonAction::None) {
            ButtonAction::ShowDialog { id: dialog_id } => {
                // Use special prefix to signal dialog open intent
                (ctx.event_callback)(&format!("__dialog_open:{}", dialog_id), None);
            }
            ButtonAction::CloseDialog => {
                (ctx.event_callback)("__dialog_close", None);
            }
            ButtonAction::OpenPage { id: page_id } => {
                (ctx.event_callback)(&format!("__page_open:{}", page_id), None);
            }
            ButtonAction::ClosePage => {
                (ctx.event_callback)("__page_close", None);
            }
            ButtonAction::Custom(custom_id) => {
                (ctx.event_callback)(custom_id, None);
            }
            ButtonAction::None => {
                // Normal button click - send to plugin
                (ctx.event_callback)(id, None);
            }
        }
    }
}

pub fn render_text_input(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    value: &str,
    placeholder: &Option<String>,
) {
    let colors = ctx.colors;
    let temp_id = ui.make_persistent_id(id);
    // Retrieve temp state or default to current value
    let mut text = ui
        .data(|data| data.get_temp::<String>(temp_id))
        .unwrap_or(value.to_string());

    // If placeholder is set, render as simple search-style input (no label title)
    if let Some(hint) = placeholder {
        let response = TextInput::new(&mut text)
            .hint(hint)
            .width(ui.available_width())
            .with_theme_colors(colors)
            .show(ui);

        if response.changed() {
            ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
            // Auto-submit on change for filter inputs
            (ctx.event_callback)(id, Some(text.clone()));
        }
    } else {
        // Original behavior with SettingsRow wrapper
        SettingsRow::new(label)
            .action(|ui| {
                ui.horizontal(|ui| {
                    let response = TextInput::new(&mut text)
                        .width(200.0)
                        .with_theme_colors(colors)
                        .show(ui);

                    // If changed, update temp state
                    if response.changed() {
                        ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
                    }

                    // Show Save button if text differs from stored value
                    let is_modified = text != *value;
                    if is_modified {
                        if ui
                            .add(
                                arclain_widgets::TextButton::new(
                                    "Save",
                                    arclain_widgets::ButtonSize::Small,
                                )
                                .with_theme_colors(colors),
                            )
                            .clicked()
                            || (response.response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            (ctx.event_callback)(id, Some(text.clone()));
                            // Clear temp state to sync with new incoming value
                            ui.data_mut(|data| data.remove::<String>(temp_id));
                        }
                    } else if response.response.lost_focus() {
                        // If focus lost without changes (or reverted), assume sync
                        // Optional: clear temp logic if needed
                    }
                });
            })
            .show(ui, colors);
    }
}

pub fn render_checkbox(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    checked: bool,
) {
    let colors = ctx.colors;
    let temp_id = ui.make_persistent_id(id);
    let mut is_checked = checked;

    // Check for optimistic state to handle thread latency
    if let Some(optimistic) = ui.data(|d| d.get_temp::<bool>(temp_id)) {
        if optimistic == checked {
            // Backend has caught up, clear optimistic state
            ui.data_mut(|d| d.remove::<bool>(temp_id));
        } else {
            // Backend stale, use optimistic value
            is_checked = optimistic;
        }
    }

    SettingsRow::new(label)
        .action(|ui| {
            if ui
                .add(arclain_widgets::ToggleSwitch::new(&mut is_checked))
                .changed()
            {
                // Set optimistic state immediately
                ui.data_mut(|d| d.insert_temp(temp_id, is_checked));
                (ctx.event_callback)(id, Some(is_checked.to_string()));
            }
        })
        .show(ui, colors);
}

pub fn render_radio_group(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    options: &[String],
    selected: &str,
) {
    let colors = ctx.colors;
    SettingsRow::new(label)
        .action(|ui| {
            let mut current_selected = selected.to_string();
            let mut changed = false;
            ui.horizontal(|ui| {
                for option in options {
                    if ui
                        .radio_value(
                            &mut current_selected,
                            option.clone(),
                            egui::RichText::new(option).color(colors.on_surface),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
            if changed {
                (ctx.event_callback)(id, Some(current_selected));
            }
        })
        .show(ui, colors);
}

pub fn render_slider(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    value: f64,
    min: f64,
    max: f64,
    _step: Option<f64>,
) {
    // ThemedSlider works in f32. Plugin sliders are integer-or-low-
    // precision (volumes, percentages, counts), so the cast is fine.
    // _step is currently ignored; ThemedSlider does continuous values
    // — wire it up if a plugin requests stepped behaviour.
    let colors = ctx.colors;
    SettingsRow::new(label)
        .action(|ui| {
            let mut current_value = value as f32;
            let response = ui.add(
                ThemedSlider::new(&mut current_value, (min as f32)..=(max as f32))
                    .with_theme_colors(colors),
            );
            if response.changed() {
                (ctx.event_callback)(id, Some((current_value as f64).to_string()));
            }
        })
        .show(ui, colors);
}

pub fn render_dropdown(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    id: &str,
    label: &str,
    options: &[String],
    selected: &str,
) {
    let colors = ctx.colors;
    SettingsRow::new(label)
        .action(|ui| {
            let mut current_selected = selected.to_string();
            ThemedDropdown::new(id, &current_selected)
                .with_theme_colors(colors)
                .show_ui(ui, |ui| {
                    for option in options {
                        if ui
                            .selectable_value(
                                &mut current_selected,
                                option.clone(),
                                egui::RichText::new(option).color(colors.on_surface),
                            )
                            .changed()
                        {
                            (ctx.event_callback)(id, Some(current_selected.clone()));
                        }
                    }
                });
        })
        .show(ui, colors);
}
