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
        info(&format!(
            "[DLSite Plugin] on_ui_event called: id={}, value={:?}",
            id, value
        ));

        match id.as_str() {
            "request_timeout" => {
                info("[DLSite Plugin] Handling request_timeout");
                if let Some(val) = value {
                    info(&format!("Request timeout changed to: {}", val));
                }
            }
            "enable_cache" => {
                info("[DLSite Plugin] Handling enable_cache");
                if let Some(val) = value {
                    info(&format!("Cache enabled: {}", val));
                }
            }
            "fetch_metadata" => {
                info("[DLSite Plugin] Handling fetch_metadata");
                info("Fetching metadata from DLSite...");
                emit_mock_metadata();
                info("[DLSite Plugin] emit_mock_metadata completed");
            }
            _ => {
                info(&format!("[DLSite Plugin] Unknown event: {}", id));
            }
        }

        info("[DLSite Plugin] on_ui_event finished");
    }
}

/// Detect DLSite code from filename (e.g., "RJ123456" or "[RJ123456]")
/// Simple implementation without regex for WASM compatibility
fn detect_dlsite_code(filename: &str) -> Option<String> {
    let upper = filename.to_uppercase();

    // Find "RJ" followed by 6-8 digits
    if let Some(pos) = upper.find("RJ") {
        let after_rj = &upper[pos + 2..];
        let digits: String = after_rj
            .chars()
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
    use archust_plugin_sdk::{current_archive_info, emit_metadata, info};

    info("[DLSite Plugin] emit_mock_metadata: Starting");

    // Get current archive info
    info("[DLSite Plugin] Calling current_archive_info()");
    let product_id = if let Some(info_data) = current_archive_info() {
        info(&format!(
            "[DLSite Plugin] Archive info: filename={}",
            info_data.filename
        ));
        // Try to detect DLSite code from filename
        let detected = detect_dlsite_code(&info_data.filename);
        info(&format!("[DLSite Plugin] Detected code: {:?}", detected));
        detected.unwrap_or_else(|| "RJ123456".to_string())
    } else {
        info("[DLSite Plugin] No archive info available, using default");
        "RJ123456".to_string()
    };

    info(&format!("[DLSite Plugin] Using product_id: {}", product_id));

    // Example layered metadata structure
    info("[DLSite Plugin] Building metadata JSON");
    let metadata_json = serde_json::json!({
        "product_id": product_id,
        "source": "dlsite",
        "common": {
            "title": "Sample Game Title",
            "description": "A sample RPG game with fantasy elements",
            "tags": ["RPG", "Fantasy", "Adventure"],
            "release_date": "2024-01-01",
            "creator": "Sample Circle"
        },
        "dlsite": {
            "code": product_id,
            "circle": "サンプルサークル",
            "work_format": "ゲーム",
            "genre": ["RPG", "ファンタジー"],
            "price": "1000 JPY",
            "age_rating": "全年齢",
            "language": "日本語"
        },
        "screenshots": []
    })
    .to_string();

    info(&format!(
        "[DLSite Plugin] Metadata JSON length: {} bytes",
        metadata_json.len()
    ));

    // Emit metadata to host
    info("[DLSite Plugin] Calling emit_metadata()");
    emit_metadata(&metadata_json);
    info("[DLSite Plugin] emit_metadata() returned successfully");
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);
