use archust_plugin_sdk::info;
use std::cell::RefCell;
use std::sync::Mutex;

// Plugin state to store found metadata
struct PluginState {
    found_metadata: Option<(String, serde_json::Value)>, // (product_id, json)
    last_status: String,
}

// Global state (thread-local for WASM component)
thread_local! {
    static STATE: RefCell<PluginState> = RefCell::new(PluginState {
        found_metadata: None,
        last_status: "Ready to scan".to_string(),
    });
}

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
            "Sidebar" => {
                let mut elements = vec![UiElement::Label(LabelConfig {
                    text: "DLSite Metadata".to_string(),
                    bold: true,
                    size: Some(18.0),
                })];

                STATE.with(|state| {
                    let state = state.borrow();

                    elements.push(UiElement::Label(LabelConfig {
                        text: state.last_status.clone(),
                        bold: false,
                        size: None,
                    }));

                    if let Some((id, data)) = &state.found_metadata {
                        let title = data["work_name"].as_str().unwrap_or("Unknown Title");
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "show_details".to_string(),
                            label: format!("Found: {} ({})", title, id),
                        }));
                    } else {
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "fetch_metadata".to_string(),
                            label: "Fetch Metadata".to_string(),
                        }));
                    }
                });

                elements
            }
            _ => vec![],
        }
    }

    fn on_ui_event(id: String, value: Option<String>) {
        info(&format!(
            "[DLSite Plugin] on_ui_event called: id={}, value={:?}",
            id, value
        ));

        match id.as_str() {
            "fetch_metadata" => {
                info("[DLSite Plugin] Handling fetch_metadata");
                STATE.with(|state| {
                    state.borrow_mut().last_status = "Scanning...".to_string();
                });

                match perform_scan() {
                    Ok(Some((product_id, json))) => {
                        info("[DLSite Plugin] Metadata found");
                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.found_metadata = Some((product_id.clone(), json.clone()));
                            s.last_status = "Metadata found!".to_string();
                        });

                        // Emit metadata immediately
                        let metadata_json = generate_metadata_json(&product_id, Some(&json));
                        archust_plugin_sdk::emit_metadata(&metadata_json);
                    }
                    Ok(None) => {
                        info("[DLSite Plugin] No metadata found");
                        STATE.with(|state| {
                            state.borrow_mut().last_status = "No DLSite code found".to_string();
                        });
                    }
                    Err(e) => {
                        info(&format!("[DLSite Plugin] Scan failed: {}", e));
                        STATE.with(|state| {
                            state.borrow_mut().last_status = format!("Error: {}", e);
                        });
                    }
                }
            }
            "show_details" => {
                STATE.with(|state| {
                    if let Some((id, json)) = &state.borrow().found_metadata {
                        let title = json["work_name"].as_str().unwrap_or("Unknown");
                        let maker = json["maker_name"].as_str().unwrap_or("Unknown");
                        let price = json["price"].as_u64().unwrap_or(0);

                        let msg = format!(
                            "Title: {}\nCircle: {}\nPrice: {} JPY\nCode: {}",
                            title, maker, price, id
                        );
                        archust_plugin_sdk::show_message("DLSite Metadata Details", &msg);
                    }
                });
            }
            _ => {}
        }
    }
}

fn perform_scan() -> Result<Option<(String, serde_json::Value)>, String> {
    use archust_plugin_sdk::{current_archive_info, info, list_archive_files};

    let info_data = current_archive_info().ok_or("No archive open")?;
    info(&format!(
        "[DLSite Plugin] Scanning archive: {}",
        info_data.filename
    ));

    // 1. Check filename
    if let Some(code) = detect_dlsite_code(&info_data.filename) {
        info(&format!("[DLSite Plugin] Found code in filename: {}", code));
        if let Some(json) = fetch_dlsite_metadata(&code) {
            return Ok(Some((code, json)));
        }
    }

    // 2. Check archive contents (folders)
    info("[DLSite Plugin] Checking archive contents...");
    match list_archive_files() {
        Ok(files) => {
            for file in files {
                // Check if it's a folder (ends with /) or just check path components
                // We just look for the code in any path string
                if let Some(code) = detect_dlsite_code(&file) {
                    info(&format!(
                        "[DLSite Plugin] Found code in archive content: {}",
                        code
                    ));
                    if let Some(json) = fetch_dlsite_metadata(&code) {
                        return Ok(Some((code, json)));
                    }
                }
            }
        }
        Err(e) => info(&format!(
            "[DLSite Plugin] Failed to list archive files: {}",
            e
        )),
    }

    Ok(None)
}

/// Detect DLSite code using Regex
fn detect_dlsite_code(text: &str) -> Option<String> {
    use regex::Regex;
    // RJ/VJ/BJ followed by 6-8 digits
    // Case insensitive is handled by (?i)
    let re = Regex::new(r"(?i)(RJ|VJ|BJ)(\d{6,8})").unwrap();

    if let Some(caps) = re.captures(text) {
        let prefix = caps.get(1)?.as_str().to_uppercase();
        let digits = caps.get(2)?.as_str();
        return Some(format!("{}{}", prefix, digits));
    }
    None
}

fn fetch_dlsite_metadata(product_id: &str) -> Option<serde_json::Value> {
    use archust_plugin_sdk::{http_get, info};

    let url = format!(
        "https://www.dlsite.com/home/api/=/product.json?work_no={}",
        product_id
    );
    info(&format!("[DLSite Plugin] Fetching URL: {}", url));

    match http_get(&url) {
        Ok(response_body) => {
            match serde_json::from_str::<serde_json::Value>(&response_body) {
                Ok(json) => {
                    if let Some(arr) = json.as_array() {
                        if let Some(first) = arr.first() {
                            return Some(first.clone());
                        }
                    }
                    if json.is_object() {
                        // Check if it's an error response or empty
                        // DLSite API might return success but empty result?
                        return Some(json);
                    }
                    None
                }
                Err(_) => None,
            }
        }
        Err(_) => None,
    }
}

fn generate_metadata_json(product_id: &str, dlsite_data: Option<&serde_json::Value>) -> String {
    let (title, circle, description, price, release_date, tags) = if let Some(data) = dlsite_data {
        (
            data["work_name"].as_str().unwrap_or("Unknown Title"),
            data["maker_name"].as_str().unwrap_or("Unknown Circle"),
            data["intro_s"].as_str().unwrap_or(""),
            format!("{} JPY", data["price"].as_u64().unwrap_or(0)),
            data["regist_date"].as_str().unwrap_or(""),
            data["genres"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v["name"].as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )
    } else {
        // Fallback should not happen in new logic, but keep safe default
        ("Unknown", "Unknown", "", "0 JPY".to_string(), "", vec![])
    };

    serde_json::json!({
        "product_id": product_id,
        "source": "dlsite",
        "common": {
            "title": title,
            "description": description,
            "tags": tags,
            "release_date": release_date,
            "creator": circle
        },
        "dlsite": {
            "code": product_id,
            "circle": circle,
            "price": price,
            "raw_data": dlsite_data
        },
        "screenshots": []
    })
    .to_string()
}

archust_plugin_sdk::export!(Component with_types_in archust_plugin_sdk);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_dlsite_code() {
        // Standard codes
        assert_eq!(
            detect_dlsite_code("RJ123456.zip"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("rj123456.rar"),
            Some("RJ123456".to_string())
        );

        // Inside brackets
        assert_eq!(
            detect_dlsite_code("[RJ123456] Game Title"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("(RJ123456) Game Title"),
            Some("RJ123456".to_string())
        );

        // 7-digit codes (newer)
        assert_eq!(
            detect_dlsite_code("RJ1234567.zip"),
            Some("RJ1234567".to_string())
        );

        // 8-digit codes (future proofing)
        assert_eq!(
            detect_dlsite_code("RJ12345678.zip"),
            Some("RJ12345678".to_string())
        );

        // Invalid codes
        assert_eq!(detect_dlsite_code("NoCodeHere.zip"), None);
        assert_eq!(detect_dlsite_code("RJ123.zip"), None); // Too short
                                                           // Regex matches the first 8 digits found
        assert_eq!(
            detect_dlsite_code("RJ123456789.zip"),
            Some("RJ12345678".to_string())
        );
        assert_eq!(detect_dlsite_code("RJABCDEF.zip"), None); // Not digits

        // Multiple occurrences (takes first valid)
        assert_eq!(
            detect_dlsite_code("RJ123456_RJ999999.zip"),
            Some("RJ123456".to_string())
        );
    }

    #[test]
    fn test_generate_metadata_json() {
        let json = generate_metadata_json("RJ123456", None);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["product_id"], "RJ123456");
        assert_eq!(parsed["source"], "dlsite");
        assert_eq!(parsed["dlsite"]["code"], "RJ123456");
        assert_eq!(parsed["common"]["title"], "Unknown");
    }
}
