use archust_plugin_sdk::info;

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn init() {
        info("UI Demo Plugin initialized via Component Model!");
    }

    fn get_default_rules() -> Vec<archust_plugin_sdk::arclain::plugin::rules::PluginRuleDefinition>
    {
        vec![]
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
                UiElement::Separator,
                UiElement::Label(LabelConfig {
                    text: "New Elements".to_string(),
                    bold: true,
                    size: None,
                }),
                UiElement::RadioGroup(RadioGroupConfig {
                    id: "theme_radio".to_string(),
                    label: "Theme".to_string(),
                    options: vec!["Light".to_string(), "Dark".to_string()],
                    selected: "Light".to_string(),
                }),
                UiElement::Slider(SliderConfig {
                    id: "opacity_slider".to_string(),
                    label: "Opacity".to_string(),
                    value: 0.5,
                    min: 0.0,
                    max: 1.0,
                    step: Some(0.1),
                }),
                UiElement::Dropdown(DropdownConfig {
                    id: "mode_dropdown".to_string(),
                    label: "Mode".to_string(),
                    options: vec!["Simple".to_string(), "Advanced".to_string()],
                    selected: "Simple".to_string(),
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
