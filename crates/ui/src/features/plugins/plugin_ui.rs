use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::ThemeColors;
use arclain_plugins::types::PluginUiElement;
use eframe::egui;

/// Callback for when a UI event occurs
pub type UiEventCallback<'a> = Box<dyn FnMut(&str, Option<String>) + 'a>;

/// Render a plugin UI element and its children
pub fn render_ui_element(
    ui: &mut egui::Ui,
    element: &PluginUiElement,
    event_callback: &mut UiEventCallback<'_>,
    colors: &ThemeColors,
) {
    match element {
        PluginUiElement::Column { children } => {
            ui.vertical(|ui| {
                for child in children {
                    render_ui_element(ui, child, event_callback, colors);
                }
            });
        }
        PluginUiElement::Row { children } => {
            ui.horizontal(|ui| {
                for child in children {
                    render_ui_element(ui, child, event_callback, colors);
                }
            });
        }
        PluginUiElement::Label { text, bold, size } => {
            // Use SectionHeader if bold and large-ish, otherwise plain label
            if *bold && size.unwrap_or(14.0) >= 14.0 {
                SectionHeader::new(text).show(ui, colors);
            } else {
                let mut rich_text = egui::RichText::new(text).color(colors.on_surface);
                if *bold {
                    rich_text = rich_text.strong();
                }
                if let Some(s) = size {
                    rich_text = rich_text.size(*s);
                }
                ui.label(rich_text);
            }
        }
        PluginUiElement::Button { id, label } => {
            // Buttons might be standalone actions
            if ui
                .add(arclain_widgets::TextButton::new(
                    label,
                    arclain_widgets::ButtonSize::Small,
                ))
                .clicked()
            {
                event_callback(id, None);
            }
        }
        PluginUiElement::TextInput { id, label, value } => {
            let temp_id = ui.make_persistent_id(&id);
            // Retrieve temp state or default to current value
            let mut text = ui
                .data(|data| data.get_temp::<String>(temp_id))
                .unwrap_or(value.clone());

            SettingsRow::new(label)
                .action(|ui| {
                    ui.horizontal(|ui| {
                        let response =
                            ui.add(egui::TextEdit::singleline(&mut text).desired_width(200.0));

                        // If changed, update temp state
                        if response.changed() {
                            ui.data_mut(|data| data.insert_temp(temp_id, text.clone()));
                        }

                        // Show Save button if text differs from stored value
                        let is_modified = text != *value;
                        if is_modified {
                            if ui.button("Save").clicked()
                                || (response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                event_callback(id, Some(text.clone()));
                                // Clear temp state to sync with new incoming value
                                ui.data_mut(|data| data.remove::<String>(temp_id));
                            }
                        } else if response.lost_focus() {
                            // If focus lost without changes (or reverted), assume sync
                            // Optional: clear temp logic if needed
                        }
                    });
                })
                .show(ui, colors);
        }
        PluginUiElement::Checkbox { id, label, checked } => {
            let temp_id = ui.make_persistent_id(id);
            let mut is_checked = *checked;

            // Check for optimistic state to handle thread latency
            if let Some(optimistic) = ui.data(|d| d.get_temp::<bool>(temp_id)) {
                if optimistic == *checked {
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
                        event_callback(id, Some(is_checked.to_string()));
                    }
                })
                .show(ui, colors);
        }
        PluginUiElement::Separator => {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
        }
        PluginUiElement::Space { size } => {
            ui.add_space(*size);
        }
        PluginUiElement::RadioGroup {
            id,
            label,
            options,
            selected,
        } => {
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current_selected = selected.clone();
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
                        event_callback(id, Some(current_selected));
                    }
                })
                .show(ui, colors);
        }
        PluginUiElement::Slider {
            id,
            label,
            value,
            min,
            max,
            step,
        } => {
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current_value = *value;
                    let slider = egui::Slider::new(&mut current_value, *min..=*max);
                    let slider = if let Some(s) = step {
                        slider.step_by(*s as f64)
                    } else {
                        slider
                    };

                    if ui.add(slider).changed() {
                        event_callback(id, Some(current_value.to_string()));
                    }
                })
                .show(ui, colors);
        }
        PluginUiElement::Dropdown {
            id,
            label,
            options,
            selected,
        } => {
            SettingsRow::new(label)
                .action(|ui| {
                    let mut current_selected = selected.clone();
                    egui::ComboBox::from_id_salt(id)
                        .selected_text(
                            egui::RichText::new(&current_selected).color(colors.on_surface),
                        )
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
                                    event_callback(id, Some(current_selected.clone()));
                                }
                            }
                        });
                })
                .show(ui, colors);
        }
    }
}

/// Render a list of UI elements
pub fn render_ui_elements(
    ui: &mut egui::Ui,
    elements: &[PluginUiElement],
    event_callback: &mut UiEventCallback<'_>,
    colors: &ThemeColors,
) {
    for element in elements {
        render_ui_element(ui, element, event_callback, colors);
    }
}
