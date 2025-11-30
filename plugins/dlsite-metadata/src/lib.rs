use archust_plugin_sdk::info;

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn init() {
        info("DLSite Metadata Enricher initialized via Component Model");
    }

    fn get_ui_layout(
        extension_point: String,
    ) -> Vec<archust_plugin_sdk::arclain::plugin::ui::UiElement> {
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match extension_point.as_str() {
            "MainPage" => vec![
                UiElement::TextInput(TextInputConfig {
                    id: "request_timeout".to_string(),
                    label: "API Request Timeout (seconds)".to_string(),
                    value: "30".to_string(),
                }),
                UiElement::Checkbox(CheckboxConfig {
                    id: "enable_cache".to_string(),
                    label: "Enable Metadata Caching".to_string(),
                    checked: true,
                }),
            ],
            "Sidebar" => vec![
                UiElement::Label(LabelConfig {
                    text: "DLSite Metadata".to_string(),
                    bold: true,
                    size: Some(18.0),
                }),
                UiElement::Label(LabelConfig {
                    text: "Ready to scan".to_string(),
                    bold: false,
                    size: None,
                }),
                UiElement::Button(ButtonConfig {
                    id: "fetch_metadata".to_string(),
                    label: "Fetch Metadata".to_string(),
                }),
            ],
            _ => vec![],
        }
    }

    fn on_ui_event(id: String, value: Option<String>) {
        match id.as_str() {
            "request_timeout" => {
                if let Some(val) = value {
                    info(&format!("Request timeout changed to: {}", val));
                }
            }
            "enable_cache" => {
                if let Some(val) = value {
                    info(&format!("Cache enabled: {}", val));
                }
            }
            "fetch_metadata" => {
                info("Fetching metadata from DLSite...");
            }
            _ => {}
        }
    }
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
