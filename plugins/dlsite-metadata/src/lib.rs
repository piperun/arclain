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
                // TODO: SDK needs to provide current_archive_info() to get filename
                // For now, emit mock metadata as proof-of-concept
                emit_mock_metadata();
            }
            _ => {}
        }
    }
}

/// Detect DLSite code from filename (e.g., "RJ123456" or "[RJ123456]")
/// Simple implementation without regex for WASM compatibility
fn detect_dlsite_code(filename: &str) -> Option<String> {
    let upper = filename.to_uppercase();
    
    // Find "RJ" followed by 6-8 digits
    if let Some(pos) = upper.find("RJ") {
        let after_rj = &upper[pos + 2..];
        let digits: String = after_rj.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        
        if digits.len() >= 6 && digits.len() <= 8 {
            return Some(format!("RJ{}", digits));
        }
    }
    
    None
}

/// Emit mock DLSite metadata (in real implementation, would fetch from DLSite API)
fn emit_mock_metadata() {
    use archust_plugin_sdk::emit_metadata;

    // Example layered metadata structure
    let metadata_json = serde_json::json!({
        "product_id": "RJ123456",
        "source": "dlsite",
        "common": {
            "title": "Sample Game Title",
            "description": "A sample RPG game with fantasy elements",
            "tags": ["RPG", "Fantasy", "Adventure"],
            "release_date": "2024-01-01",
            "creator": "Sample Circle"
        },
        "dlsite": {
            "code": "RJ123456",
            "circle": "サンプルサークル",
            "work_format": "ゲーム",
            "genre": ["RPG", "ファンタジー"],
            "price": "1000 JPY",
            "age_rating": "全年齢",
            "language": "日本語"
        },
        "screenshots": []
    });

    // TODO: SDK needs to provide emit_metadata() function to send this to host
    // For now, just log it
    info(&format!("Would emit metadata: {}", metadata_json));
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
