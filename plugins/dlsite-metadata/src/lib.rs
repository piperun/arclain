use archust_plugin_sdk::info;
use std::cell::RefCell;

// Plugin state to store found metadata
struct PluginState {
    found_metadata: Option<(String, serde_json::Value, Option<ScrapedData>)>, // (product_id, json, scraped)
    last_status: String,
    search_mode: bool,
    search_query: String,
    search_results: Vec<(String, String, String)>, // (code, title, maker)
    auto_load_enabled: bool, // Cache for the auto_load_cache setting
}

// Global state (thread-local for WASM component)
thread_local! {
    static STATE: RefCell<PluginState> = RefCell::new(PluginState {
        found_metadata: None,
        last_status: "Ready to scan".to_string(),
        search_mode: false,
        search_query: String::new(),
        search_results: Vec::new(),
        auto_load_enabled: true, // Default to enabled
    });
}

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn init() {
        info("DLSite Metadata Enricher initialized via Component Model");
        
        // Check if auto-load is enabled and try to load cached metadata for current archive
        let auto_load = archust_plugin_sdk::arclain::plugin::host::get_setting("auto_load_cache")
            .unwrap_or_else(|| "true".to_string()) == "true";
        
        STATE.with(|state| {
            state.borrow_mut().auto_load_enabled = auto_load;
        });
        
        if auto_load {
            info("[DLSite Plugin] Auto-load enabled, checking for cached metadata");
            if let Err(e) = try_auto_load_cache() {
                info(&format!("[DLSite Plugin] Auto-load failed: {}", e));
            }
        }
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
                UiElement::Checkbox(CheckboxConfig {
                    id: "auto_load_cache".to_string(),
                    label: "Auto-load cached metadata on archive open".to_string(),
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

                    if state.search_mode {
                        // Search UI
                        elements.push(UiElement::TextInput(TextInputConfig {
                            id: "search_query".to_string(),
                            label: "Search Query".to_string(),
                            value: state.search_query.clone(),
                        }));
                        
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "perform_search".to_string(),
                            label: "Search".to_string(),
                        }));
                        
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "cancel_search".to_string(),
                            label: "Cancel".to_string(),
                        }));
                        
                        if !state.search_results.is_empty() {
                            elements.push(UiElement::Separator);
                            elements.push(UiElement::Label(LabelConfig {
                                text: "Results:".to_string(),
                                bold: true,
                                size: None,
                            }));
                            
                            for (code, title, maker) in &state.search_results {
                                elements.push(UiElement::Button(ButtonConfig {
                                    id: format!("select_result_{}", code),
                                    label: format!("[{}] {} ({})", code, title, maker),
                                }));
                            }
                        }
                    } else {
                        // Normal UI
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
                            
                            elements.push(UiElement::Button(ButtonConfig {
                                id: "toggle_search".to_string(),
                                label: "Search Manually".to_string(),
                            }));
                        }
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

        if id.starts_with("select_result_") {
            let code = id.trim_start_matches("select_result_").to_string();
            info(&format!("[DLSite Plugin] Selected result: {}", code));
            
            STATE.with(|state| {
                state.borrow_mut().last_status = format!("Fetching {}...", code);
                state.borrow_mut().search_mode = false;
            });
            
            // Re-use logic to fetch and emit
            if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
                 // Generate final JSON to save to cache
                let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
                archust_plugin_sdk::save_cached_metadata(&code, &metadata_json);
                archust_plugin_sdk::emit_metadata(&metadata_json);
                
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.found_metadata = Some((code.clone(), json, scraped));
                    s.last_status = format!("Metadata found for {}", code);
                });
            } else {
                STATE.with(|state| {
                    state.borrow_mut().last_status = format!("Failed to fetch {}", code);
                });
            }
            return;
        }

        match id.as_str() {
            "auto_load_cache" => {
                if let Some(val) = value {
                    let enabled = val == "true";
                    STATE.with(|state| {
                        state.borrow_mut().auto_load_enabled = enabled;
                    });
                    archust_plugin_sdk::arclain::plugin::host::set_setting("auto_load_cache", &val);
                    info(&format!("[DLSite Plugin] Auto-load cache setting changed to: {}", enabled));
                }
            }
            "enable_cache" => {
                if let Some(val) = value {
                    archust_plugin_sdk::arclain::plugin::host::set_setting("enable_cache", &val);
                    info(&format!("[DLSite Plugin] Cache enabled setting changed to: {}", val));
                }
            }
            "toggle_search" => {
                STATE.with(|state| {
                    state.borrow_mut().search_mode = true;
                });
            }
            "cancel_search" => {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.search_mode = false;
                    s.search_results.clear();
                });
            }
            "search_query" => {
                if let Some(query) = value {
                    STATE.with(|state| {
                        state.borrow_mut().search_query = query;
                    });
                }
            }
            "perform_search" => {
                let query = STATE.with(|state| state.borrow().search_query.clone());
                if !query.is_empty() {
                    STATE.with(|state| {
                        state.borrow_mut().last_status = format!("Searching for '{}'...", query);
                    });
                    
                    let results = search_dlsite(&query);
                    
                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        if results.is_empty() {
                            s.last_status = "No results found".to_string();
                        } else {
                            s.last_status = format!("Found {} results", results.len());
                        }
                        s.search_results = results;
                    });
                }
            }
            "fetch_metadata" => {
                info("[DLSite Plugin] Handling fetch_metadata");
                STATE.with(|state| {
                    state.borrow_mut().last_status = "Scanning...".to_string();
                });

                match perform_scan() {
                    Ok(Some((product_id, json, scraped))) => {
                        info("[DLSite Plugin] Metadata found");
                        
                        // Note: perform_scan() already emits metadata (either from cache or fresh),
                        // so we don't emit again here to avoid double emission
                        
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
                        let title = json["title"].as_str().unwrap_or("Unknown");
                        let maker = json["creator"].as_str().unwrap_or("Unknown");
                        let price = json["dlsite"]["price"].as_u64().unwrap_or(0);
                        
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
    use archust_plugin_sdk::{current_archive_info, info, list_archive_files, get_cached_metadata, save_cached_metadata};

    let info_data = current_archive_info().ok_or("No archive open")?;
    info(&format!(
        "[DLSite Plugin] Scanning archive: {}",
        info_data.filename
    ));

    // Helper to process found code
    let process_code = |code: String| -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
        info(&format!("[DLSite Plugin] Found code: {}", code));
        
        // 1. Check cache
        if let Some(cached_json) = get_cached_metadata(&code) {
            info(&format!("[DLSite Plugin] Found cached metadata for {}", code));
            
            if let Ok(cached_value) = serde_json::from_str::<serde_json::Value>(&cached_json) {
                 // Emit the cached metadata directly
                 archust_plugin_sdk::emit_metadata(&cached_json);
                 
                 // Convert cached format to API response format for UI state
                 // Cached JSON has: title, creator, circle, description, dlsite.price
                 // API response has: work_name, maker_name, intro_s, price
                 let mut api_response = serde_json::json!({
                     "work_name": cached_value["title"].as_str().unwrap_or("Unknown Title"),
                     "maker_name": cached_value["circle"].as_str()
                         .or_else(|| cached_value["creator"].as_str())
                         .unwrap_or("Unknown Circle"),
                     "intro_s": cached_value["description"].as_str().unwrap_or(""),
                     "price": cached_value["dlsite"]["price"].as_str()
                         .and_then(|s| s.parse::<u64>().ok())
                         .unwrap_or(0),
                     "regist_date": cached_value["release_date"].as_str().unwrap_or(""),
                 });
                 
                 // Add tags if present
                 if let Some(tags) = cached_value["tags"].as_array() {
                     api_response["genres"] = serde_json::json!(
                         tags.iter().map(|t| serde_json::json!({"name": t})).collect::<Vec<_>>()
                     );
                 }
                 
                 return Ok(Some((code, api_response, None)));
            }
        }

        // 2. Fetch from network
        if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
            // Generate final JSON to save to cache
            let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
            save_cached_metadata(&code, &metadata_json);
            
            // Emit the fresh metadata
            archust_plugin_sdk::emit_metadata(&metadata_json);
            
            return Ok(Some((code, json, scraped)));
        }
        Ok(None)
    };

    // 1. Check filename
    if let Some(code) = detect_dlsite_code(&info_data.filename) {
        if let Ok(Some(result)) = process_code(code) {
            return Ok(Some(result));
        }
    }

    // 2. Check archive contents (folders)
    info("[DLSite Plugin] Checking archive contents...");
    match list_archive_files() {
        Ok(files) => {
            for file in files {
                if let Some(code) = detect_dlsite_code(&file) {
                    if let Ok(Some(result)) = process_code(code) {
                        return Ok(Some(result));
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
    use archust_plugin_sdk::{http_get, log_network_activity};

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
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
        )
    } else {
        (
            "Unknown Title".to_string(),
            "Unknown Circle".to_string(),
            "",
            0,
            "".to_string(),
            Vec::new(),
        )
    };

    // Override with scraped data if available
    if let Some(scraped) = scraped_data {
        if let Some(t) = &scraped.title {
            title = t.clone();
        }
        if let Some(c) = &scraped.circle {
            circle = c.clone();
        }
        if let Some(d) = &scraped.release_date {
            release_date = d.clone();
        }
        if !scraped.tags.is_empty() {
            tags = scraped.tags.clone();
        }
    }

    let description = if let Some(scraped) = scraped_data {
        scraped.description.clone().unwrap_or(short_desc.to_string())
    } else {
        short_desc.to_string()
    };

    let screenshots = if let Some(scraped) = scraped_data {
        scraped.screenshots.iter().map(|url| {
            // Convert to ScreenshotData::FilePath variant (using URL as path for now)
            // The core engine will handle downloading these
            serde_json::json!({ "FilePath": url })
        }).collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let metadata = serde_json::json!({
        "product_id": product_id,
        "source": "dlsite",
        "title": title,
        "circle": circle,
        "creator": circle, // Alias for core engine
        "description": description,
        "release_date": release_date,
        "tags": tags,
        "screenshots": screenshots,
        "dlsite": {
            "id": product_id,
            "code": product_id, // Required by RuleEngine for $code
            "price": price.to_string(),
            "url": format!("https://www.dlsite.com/home/work/=/product_id/{}.html", product_id)
        },
        "common": {
            "dlsite_id": product_id
        }
    });

    metadata.to_string()
}

/// Search DLSite for a query and return list of (code, title, maker)
fn search_dlsite(query: &str) -> Vec<(String, String, String)> {
    use archust_plugin_sdk::{http_get, log_network_activity};
    use scraper::{Html, Selector};

    let url = format!(
        "https://www.dlsite.com/home/fsr/=/keyword/{}",
        urlencoding::encode(query)
    );
    
    log_network_activity(&format!("Searching DLSite: {}", query));
    log_network_activity(&format!("GET {}", url));

    let html = match http_get(&url) {
        Ok(h) => h,
        Err(e) => {
            log_network_activity(&format!("Search failed: {}", e));
            return Vec::new();
        }
    };

    let document = Html::parse_document(&html);
    let mut results = Vec::new();

    // Select search results
    // Try multiple selectors as DLSite layout might vary
    let item_selector = Selector::parse("li.search_result_img_box_inner, tr.n_worklist_item").unwrap();
    let title_selector = Selector::parse("dt.work_name a, a.work_name").unwrap();
    let maker_selector = Selector::parse("dd.maker_name a, span.maker_name a").unwrap();

    for item in document.select(&item_selector) {
        let mut title = "Unknown".to_string();
        let mut maker = "Unknown".to_string();
        let mut code = String::new();

        if let Some(link) = item.select(&title_selector).next() {
            title = link.text().collect::<String>().trim().to_string();
            if let Some(href) = link.value().attr("href") {
                // Extract code from URL (.../product_id/RJ123456.html)
                if let Some(c) = detect_dlsite_code(href) {
                    code = c;
                }
            }
        }

        if let Some(maker_link) = item.select(&maker_selector).next() {
            maker = maker_link.text().collect::<String>().trim().to_string();
        }

        if !code.is_empty() {
            results.push((code, title, maker));
        }
        
        if results.len() >= 10 {
            break;
        }
    }
    
    log_network_activity(&format!("Found {} results", results.len()));
    results
}

/// Try to auto-load cached metadata for the current archive
fn try_auto_load_cache() -> Result<(), String> {
    use archust_plugin_sdk::{current_archive_info, get_cached_metadata, emit_metadata, info};
    
    let info_data = current_archive_info().ok_or("No archive open")?;
    
    // Check filename for DLsite code
    if let Some(code) = detect_dlsite_code(&info_data.filename) {
        info(&format!("[DLSite Plugin] Auto-checking cache for {}", code));
        
        if let Some(cached_json) = get_cached_metadata(&code) {
            info(&format!("[DLSite Plugin] Found cached metadata for {}, emitting", code));
            emit_metadata(&cached_json);
            
            // Update state with cached data
            if let Ok(cached_value) = serde_json::from_str::<serde_json::Value>(&cached_json) {
                let api_response = serde_json::json!({
                    "work_name": cached_value["title"].as_str().unwrap_or("Unknown Title"),
                    "maker_name": cached_value["circle"].as_str()
                        .or_else(|| cached_value["creator"].as_str())
                        .unwrap_or("Unknown Circle"),
                });
                
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.found_metadata = Some((code.clone(), api_response, None));
                    s.last_status = format!("Loaded from cache: {}", code);
                });
            }
            
            return Ok(());
        } else {
            info(&format!("[DLSite Plugin] No cache found for {}", code));
        }
    }
    
    Err("No DLsite code found in archive name".to_string())
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

        assert_eq!(parsed["dlsite"]["id"], "RJ123456");
        assert_eq!(parsed["common"]["dlsite_id"], "RJ123456");
        assert_eq!(parsed["title"], "Unknown Title");
    }
}
