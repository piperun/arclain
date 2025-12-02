use archust_plugin_sdk::info;
use std::cell::RefCell;
use std::sync::Mutex;

// Plugin state to store found metadata
struct PluginState {
    found_metadata: Option<(String, serde_json::Value, Option<ScrapedData>)>, // (product_id, json, scraped)
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

                    if let Some((id, data, _)) = &state.found_metadata {
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
                    Ok(Some((product_id, json, scraped))) => {
                        info("[DLSite Plugin] Metadata found");
                        
                        // Emit metadata immediately
                        let metadata_tuple = (json.clone(), scraped.clone());
                        let metadata_json = generate_metadata_json(&product_id, Some(&metadata_tuple));
                        archust_plugin_sdk::emit_metadata(&metadata_json);

                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.found_metadata = Some((product_id, json, scraped));
                            s.last_status = "Metadata found!".to_string();
                        });
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
                    if let Some((id, json, scraped)) = &state.borrow().found_metadata {
                        let title = json["work_name"].as_str().unwrap_or("Unknown");
                        let maker = json["maker_name"].as_str().unwrap_or("Unknown");
                        let price = json["price"].as_u64().unwrap_or(0);
                        
                        let desc_len = scraped.as_ref().and_then(|s| s.description.as_ref()).map(|s| s.len()).unwrap_or(0);
                        let screenshots_count = scraped.as_ref().map(|s| s.screenshots.len()).unwrap_or(0);

                        let msg = format!(
                            "Title: {}\nCircle: {}\nPrice: {} JPY\nCode: {}\nDescription Length: {}\nScreenshots: {}",
                            title, maker, price, id, desc_len, screenshots_count
                        );
                        archust_plugin_sdk::show_message("DLSite Metadata Details", &msg);
                    }
                });
            }
            _ => {}
        }
    }
}

fn perform_scan() -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
    use archust_plugin_sdk::{current_archive_info, info, list_archive_files};

    let info_data = current_archive_info().ok_or("No archive open")?;
    info(&format!(
        "[DLSite Plugin] Scanning archive: {}",
        info_data.filename
    ));

    // 1. Check filename
    if let Some(code) = detect_dlsite_code(&info_data.filename) {
        info(&format!("[DLSite Plugin] Found code in filename: {}", code));
        if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
            return Ok(Some((code, json, scraped)));
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
                    if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
                        return Ok(Some((code, json, scraped)));
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

fn fetch_dlsite_metadata(product_id: &str) -> Option<(serde_json::Value, Option<ScrapedData>)> {
    use archust_plugin_sdk::{http_get, info, log_network_activity};

    // 1. Fetch JSON API
    let api_url = format!(
        "https://www.dlsite.com/home/api/=/product.json?work_no={}",
        product_id
    );
    log_network_activity(&format!("Fetching metadata for {} from DLSite API...", product_id));
    log_network_activity(&format!("GET {}", api_url));

    let json_data = match http_get(&api_url) {
        Ok(response_body) => {
            log_network_activity(&format!("Response: {} bytes", response_body.len()));
            match serde_json::from_str::<serde_json::Value>(&response_body) {
                Ok(json) => {
                    if let Some(arr) = json.as_array() {
                        arr.first().cloned()
                    } else if json.is_object() {
                        Some(json)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    log_network_activity(&format!("Failed to parse JSON: {}", e));
                    None
                }
            }
        },
        Err(e) => {
            log_network_activity(&format!("HTTP Request failed: {}", e));
            None
        }
    };

    if json_data.is_none() {
        return None;
    }
    let json_data = json_data.unwrap();

    // 2. Fetch HTML Page for scraping
    let html_url = format!(
        "https://www.dlsite.com/home/work/=/product_id/{}.html",
        product_id
    );
    log_network_activity(&format!("Fetching HTML page for scraping..."));
    log_network_activity(&format!("GET {}", html_url));

    let scraped_data = match http_get(&html_url) {
        Ok(html) => {
            log_network_activity(&format!("Response: {} bytes", html.len()));
            scrape_html_metadata(&html)
        },
        Err(e) => {
            log_network_activity(&format!("Failed to fetch HTML: {}", e));
            None
        }
    };

    Some((json_data, scraped_data))
}

#[derive(Debug, Clone)]
struct ScrapedData {
    title: Option<String>,
    circle: Option<String>,
    release_date: Option<String>,
    tags: Vec<String>,
    description: Option<String>,
    screenshots: Vec<String>,
}

fn scrape_html_metadata(html: &str) -> Option<ScrapedData> {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    
    let mut title = None;
    let mut circle = None;
    let mut release_date = None;
    let mut tags = Vec::new();
    let mut description = None;
    let mut screenshots = Vec::new();

    // 1. Parse Tables (work_maker and work_outline)
    let tr_selector = Selector::parse("table#work_maker tr, table#work_outline tr").unwrap();
    let th_selector = Selector::parse("th").unwrap();
    let td_selector = Selector::parse("td").unwrap();
    let maker_selector = Selector::parse("span.maker_name").unwrap();
    let a_selector = Selector::parse("a").unwrap();

    for tr in document.select(&tr_selector) {
        let th = match tr.select(&th_selector).next() {
            Some(el) => el.text().collect::<String>().trim().to_string(),
            None => continue,
        };
        let td = match tr.select(&td_selector).next() {
            Some(el) => el,
            None => continue,
        };

        match th.as_str() {
            "Circle" | "サークル名" | "Brand" | "ブランド名" | "Publisher" | "出版社名" | "Label" | "レーベル" => {
                if let Some(span) = td.select(&maker_selector).next() {
                    circle = Some(span.text().collect::<String>().trim().to_string());
                } else {
                    circle = Some(td.text().collect::<String>().trim().to_string());
                }
            },
            "Published date" | "販売日" | "予告開始日" => {
                let date_str = td.text().collect::<String>().trim().to_string();
                // Try to clean up date string (e.g. "2024年01月01日" -> "2024-01-01")
                // For now just keep it as is, or do simple replacement
                release_date = Some(date_str.replace("年", "-").replace("月", "-").replace("日", ""));
            },
            "Genre" | "ジャンル" => {
                for a in td.select(&a_selector) {
                    tags.push(a.text().collect::<String>().trim().to_string());
                }
            },
            "Series" | "シリーズ名" => {
                // Could extract series here if needed
            },
            _ => {}
        }
    }

    // 2. Parse Description
    // Try meta description first
    let meta_desc_selector = Selector::parse("meta[name='description']").unwrap();
    if let Some(meta) = document.select(&meta_desc_selector).next() {
        if let Some(content) = meta.value().attr("content") {
            description = Some(content.trim().to_string());
        }
    }

    // Fallback to work_parts_area if meta is empty
    if description.is_none() {
        let parts_selector = Selector::parse("div.work_parts_area").unwrap();
        if let Some(div) = document.select(&parts_selector).next() {
            description = Some(div.text().collect::<String>().trim().to_string());
        }
    }
    
    // 3. Parse Title (h1#work_name)
    let title_selector = Selector::parse("h1#work_name").unwrap();
    if let Some(h1) = document.select(&title_selector).next() {
        title = Some(h1.text().collect::<String>().trim().to_string());
    }

    // 4. Parse Screenshots
    // Look for slider data
    let slider_selector = Selector::parse("div.product-slider-data div").unwrap();
    for div in document.select(&slider_selector) {
        if let Some(src) = div.value().attr("data-src") {
             if !src.contains("_img_main") {
                let full_url = if src.starts_with("//") {
                    format!("https:{}", src)
                } else {
                    src.to_string()
                };
                screenshots.push(full_url);
             }
        }
    }

    Some(ScrapedData {
        title,
        circle,
        release_date,
        tags,
        description,
        screenshots,
    })
}

fn generate_metadata_json(
    product_id: &str,
    data: Option<&(serde_json::Value, Option<ScrapedData>)>,
) -> String {
    let (json_data, scraped_data) = if let Some((j, s)) = data {
        (Some(j), s.as_ref())
    } else {
        (None, None)
    };

    // Extract from JSON first (fallback)
    let (mut title, mut circle, short_desc, price, mut release_date, mut tags) = if let Some(data) = json_data {
        (
            data["work_name"].as_str().unwrap_or("Unknown Title").to_string(),
            data["maker_name"].as_str().unwrap_or("Unknown Circle").to_string(),
            data["intro_s"].as_str().unwrap_or(""),
            data["price"].as_u64().unwrap_or(0),
            data["regist_date"].as_str().unwrap_or("").to_string(),
            data["genres"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v["name"].as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )
    } else {
        ("Unknown Title".to_string(), "Unknown Circle".to_string(), "", 0, "".to_string(), vec![])
    };

    // Override with scraped data if available
    if let Some(scraped) = scraped_data {
        if let Some(t) = &scraped.title {
            if !t.is_empty() { title = t.clone(); }
        }
        if let Some(c) = &scraped.circle {
            if !c.is_empty() { circle = c.clone(); }
        }
        if let Some(d) = &scraped.release_date {
            if !d.is_empty() { release_date = d.clone(); }
        }
        if !scraped.tags.is_empty() {
            tags = scraped.tags.clone();
        }
    }

    let description = scraped_data
        .and_then(|s| s.description.as_deref())
        .unwrap_or(short_desc);
        
    let screenshots = scraped_data
        .map(|s| {
            s.screenshots
                .iter()
                .map(|url| serde_json::json!({ "FilePath": url }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Generate layered JSON for metadata_json field
    serde_json::json!({
        "product_id": product_id,
        "source": "dlsite",
        "title": title,
        "description": description,
        "tags": tags,
        "release_date": release_date,
        "creator": circle,
        "screenshots": screenshots,
        "dlsite": {
            "code": product_id,
            "circle": circle,
            "price": price.to_string(),
            "short_description": short_desc
        }
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
