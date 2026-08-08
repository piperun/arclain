use archust_plugin_sdk::info;

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn get_metadata() -> archust_plugin_sdk::wirt::plugin::meta::PluginMetadata {
        // Mirrors gstreamer-preview.toml.
        archust_plugin_sdk::wirt::plugin::meta::PluginMetadata {
            id: "gstreamer-preview".to_string(),
            name: "GStreamer Media Preview".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Archust Team".to_string(),
            description: "Provides media file previews and thumbnail generation using GStreamer \
                 (hybrid WASM/native approach)"
                .to_string(),
        }
    }

    fn init() {
        info("GStreamer Preview Plugin initialized via Component Model");
    }

    fn get_default_rules() -> Vec<archust_plugin_sdk::wirt::plugin::rules::PluginRuleDefinition> {
        vec![]
    }

    fn get_ui_layout(
        extension_point: String,
    ) -> archust_plugin_sdk::wirt::plugin::ui::PluginLayout {
        use archust_plugin_sdk::wirt::plugin::ui::*;

        match extension_point.as_str() {
            "MainPage" => PluginLayout::Single(vec![
                UiElement::Checkbox(CheckboxConfig {
                    id: "enable_hw_accel".to_string(),
                    label: "Enable Hardware Acceleration".to_string(),
                    checked: true,
                }),
                UiElement::TextInput(TextInputConfig {
                    id: "preview_quality".to_string(),
                    label: "Preview Quality (1-10)".to_string(),
                    value: "8".to_string(),
                    placeholder: None,
                }),
            ]),
            "Sidebar" => PluginLayout::Single(vec![
                UiElement::Label(LabelConfig {
                    text: "Video Preview".to_string(),
                    bold: true,
                    size: Some(18.0),
                }),
                UiElement::Label(LabelConfig {
                    text: "No file selected".to_string(),
                    bold: false,
                    size: None,
                }),
                UiElement::Button(ButtonConfig {
                    id: "generate_preview".to_string(),
                    label: "Generate Preview".to_string(),
                    action: None,
                }),
            ]),
            "PluginButton" => PluginLayout::Single(vec![UiElement::Button(ButtonConfig {
                id: "gstreamer_play".to_string(),
                label: "Play Preview".to_string(),
                action: None,
            })]),
            "Panel" => PluginLayout::Single(vec![UiElement::Label(LabelConfig {
                text: "Video Stats".to_string(),
                bold: true,
                size: None,
            })]),
            _ => PluginLayout::Single(vec![]),
        }
    }

    fn get_top_tabs() -> Vec<archust_plugin_sdk::wirt::plugin::ui::TopTabConfig> {
        // GStreamer doesn't register a top tab
        vec![]
    }

    fn on_ui_event(
        id: String,
        value: Option<String>,
    ) -> Vec<archust_plugin_sdk::wirt::plugin::ui::PluginAction> {
        match id.as_str() {
            "enable_hw_accel" => {
                if let Some(val) = value {
                    info(&format!("Hardware acceleration: {}", val));
                }
            }
            "preview_quality" => {
                if let Some(val) = value {
                    info(&format!("Preview quality: {}", val));
                }
            }
            "generate_preview" => {
                info("Generating video preview...");
            }
            _ => {}
        }
        vec![]
    }
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
