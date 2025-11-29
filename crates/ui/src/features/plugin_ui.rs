use arclain_plugins::types::PluginUiElement;
use eframe::egui;

/// Callback for when a UI event occurs
pub type UiEventCallback = Box<dyn FnMut(&str, Option<String>)>;

/// Render a plugin UI element and its children
pub fn render_ui_element(
    ui: &mut egui::Ui,
    element: &PluginUiElement,
    event_callback: &mut UiEventCallback,
) {
    match element {
        PluginUiElement::Column { children } => {
            ui.vertical(|ui| {
                for child in children {
                    render_ui_element(ui, child, event_callback);
                }
            });
        }
        PluginUiElement::Row { children } => {
            ui.horizontal(|ui| {
                for child in children {
                    render_ui_element(ui, child, event_callback);
                }
            });
        }
        PluginUiElement::Label { text, bold, size } => {
            let mut rich_text = egui::RichText::new(text);
            if *bold {
                rich_text = rich_text.strong();
            }
            if let Some(s) = size {
                rich_text = rich_text.size(*s);
            }
            ui.label(rich_text);
        }
        PluginUiElement::Button { id, label } => {
            if ui.button(label).clicked() {
                event_callback(id, None);
            }
        }
        PluginUiElement::TextInput { id, label, value } => {
            let mut text = value.clone();
            ui.label(label);
            if ui.text_edit_singleline(&mut text).changed() {
                event_callback(id, Some(text));
            }
        }
        PluginUiElement::Checkbox { id, label, checked } => {
            let mut is_checked = *checked;
            if ui.checkbox(&mut is_checked, label).changed() {
                event_callback(id, Some(is_checked.to_string()));
            }
        }
        PluginUiElement::Separator => {
            ui.separator();
        }
        PluginUiElement::Space { size } => {
            ui.add_space(*size);
        }
    }
}

/// Render a list of UI elements
pub fn render_ui_elements(
    ui: &mut egui::Ui,
    elements: &[PluginUiElement],
    event_callback: &mut UiEventCallback,
) {
    for element in elements {
        render_ui_element(ui, element, event_callback);
    }
}
