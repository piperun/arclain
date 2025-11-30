use archust_plugin_sdk::info;

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn init() {
        info("UI Demo Plugin initialized via Component Model!");
    }

    fn get_ui_layout(
        extension_point: String,
    ) -> Vec<archust_plugin_sdk::arclain::plugin::ui::UiElement> {
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match extension_point.as_str() {
            "Sidebar" => vec![
                UiElement::Label(LabelConfig {
                    text: "UI Demo Plugin".to_string(),
                    bold: true,
                    size: Some(16.0),
                }),
                UiElement::Button(ButtonConfig {
                    id: "demo_btn".to_string(),
                    label: "Click Me!".to_string(),
                }),
                UiElement::TextInput(TextInputConfig {
                    id: "demo_input".to_string(),
                    label: "Enter text".to_string(),
                    value: "".to_string(),
                }),
                UiElement::Checkbox(CheckboxConfig {
                    id: "demo_check".to_string(),
                    label: "Check me".to_string(),
                    checked: false,
                }),
            ],
            _ => vec![],
        }
    }

    fn on_ui_event(id: String, value: Option<String>) {
        info(&format!("UI Event: {} = {:?}", id, value));
    }
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
