use arclain_plugins::types::PluginUiElement;
use arclain_theme::ThemeColors;
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
            let mut rich_text = egui::RichText::new(text).color(colors.on_surface);
            if *bold {
                rich_text = rich_text.strong();
            }
            if let Some(s) = size {
                rich_text = rich_text.size(*s);
            }
            ui.label(rich_text);
        }
        PluginUiElement::Button { id, label } => {
            let btn = egui::Button::new(egui::RichText::new(label).color(colors.on_surface))
                .fill(colors.surface_variant);
            if ui.add(btn).clicked() {
                event_callback(id, None);
            }
        }
        PluginUiElement::TextInput { id, label, value } => {
            let mut text = value.clone();
            ui.label(egui::RichText::new(label).color(colors.on_surface_variant));
            if ui.text_edit_singleline(&mut text).changed() {
                event_callback(id, Some(text));
            }
        }
        PluginUiElement::Checkbox { id, label, checked } => {
            let mut is_checked = *checked;
            ui.horizontal(|ui| {
                if ui.checkbox(&mut is_checked, "").changed() {
                    event_callback(id, Some(is_checked.to_string()));
                }
                ui.label(egui::RichText::new(label).color(colors.on_surface));
            });
        }
        PluginUiElement::Separator => {
            ui.separator();
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
            ui.label(egui::RichText::new(label).color(colors.on_surface));
            let mut current_selected = selected.clone();
            let mut changed = false;

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

            if changed {
                event_callback(id, Some(current_selected));
            }
        }
        PluginUiElement::Slider {
            id,
            label,
            value,
            min,
            max,
            step,
        } => {
            let mut current_value = *value;
            ui.label(egui::RichText::new(label).color(colors.on_surface));

            let slider = egui::Slider::new(&mut current_value, *min..=*max);
            let slider = if let Some(s) = step {
                slider.step_by(*s as f64)
            } else {
                slider
            };

            if ui.add(slider).changed() {
                event_callback(id, Some(current_value.to_string()));
            }
        }
        PluginUiElement::Dropdown {
            id,
            label,
            options,
            selected,
        } => {
            let mut current_selected = selected.clone();
            ui.label(egui::RichText::new(label).color(colors.on_surface));

            egui::ComboBox::from_id_salt(id)
                .selected_text(egui::RichText::new(&current_selected).color(colors.on_surface))
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
