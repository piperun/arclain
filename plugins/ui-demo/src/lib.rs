use wirt_sdk::info;

struct Component;

impl wirt_sdk::Guest for Component {
    fn get_metadata() -> wirt_sdk::wirt::plugin::meta::PluginMetadata {
        // Mirrors plugin.toml.
        wirt_sdk::wirt::plugin::meta::PluginMetadata {
            id: "ui-demo".to_string(),
            name: "UI Demo Plugin".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Arclain Team".to_string(),
            description: "Demonstrates UI capabilities in the sidebar and plugins page".to_string(),
        }
    }

    fn init() {
        info("UI Demo Plugin initialized via Component Model!");
    }

    fn get_default_rules() -> Vec<wirt_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        vec![]
    }

    fn get_ui_layout(extension_point: String) -> wirt_sdk::wirt::plugin::ui::PluginLayout {
        use wirt_sdk::wirt::plugin::ui::*;

        match extension_point.as_str() {
            "MainPage" => PluginLayout::Single(vec![
                UiElement::Label(LabelConfig {
                    text: "UI Demo Plugin".to_string(),
                    role: TextRole::Subtitle,
                }),
                UiElement::Button(ButtonConfig {
                    id: "demo_btn".to_string(),
                    label: "Click Me!".to_string(),
                    action: None,
                }),
                UiElement::TextInput(TextInputConfig {
                    id: "demo_input".to_string(),
                    label: "Enter text".to_string(),
                    value: "".to_string(),
                    placeholder: None,
                }),
                UiElement::Checkbox(CheckboxConfig {
                    id: "demo_check".to_string(),
                    label: "Check me".to_string(),
                    checked: false,
                }),
                UiElement::Separator,
                UiElement::Space(SpacingStep::Small),
                UiElement::Label(LabelConfig {
                    text: "New Elements".to_string(),
                    role: TextRole::Emphasis,
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
            ]),
            "PluginButton" => PluginLayout::Single(vec![UiElement::Button(ButtonConfig {
                id: "plugin_toolbar_btn".to_string(),
                label: "Plugin Action".to_string(),
                action: None,
            })]),
            "Panel" => PluginLayout::Single(vec![
                UiElement::Space(SpacingStep::Small),
                UiElement::Label(LabelConfig {
                    text: "Plugin Info".to_string(),
                    role: TextRole::Emphasis,
                }),
                UiElement::Label(LabelConfig {
                    text: "Status: Active".to_string(),
                    role: TextRole::Body,
                }),
            ]),
            _ => PluginLayout::Single(vec![]),
        }
    }

    fn get_top_tabs() -> Vec<wirt_sdk::wirt::plugin::ui::TopTabConfig> {
        // UI Demo doesn't register a top tab
        vec![]
    }

    fn on_ui_event(
        id: String,
        value: Option<String>,
    ) -> Vec<wirt_sdk::wirt::plugin::ui::PluginAction> {
        info(&format!("UI Event: {} = {:?}", id, value));
        vec![]
    }
}

wirt_sdk::export!(Component with_types_in wirt_sdk);
