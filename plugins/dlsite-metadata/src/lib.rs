use archust_plugin_sdk::info;
use std::cell::RefCell;

// Plugin state to store found metadata
struct PluginState {
    found_metadata: Option<(String, serde_json::Value, Option<ScrapedData>)>, // (product_id, json, scraped)
    search_query: String,
    search_results: Vec<(String, String, String)>, // (code, title, maker)
    auto_fetch_enabled: bool, // Master switch: auto-fetch when archive opens
    enable_cache: bool, // Sub-option: cache fetched results (only relevant if auto_fetch enabled)
    fetch_in_progress: bool, // Prevent double-fetch when spamming buttons
    last_archive_path: Option<String>, // Track current archive to reset state on change
}

// Global state (thread-local for WASM component)
thread_local! {
    static STATE: RefCell<PluginState> = RefCell::new(PluginState {
        found_metadata: None,
        search_query: String::new(),
        search_results: Vec::new(),
        auto_fetch_enabled: true,
        enable_cache: true,
        fetch_in_progress: false,
        last_archive_path: None,
    });
}

struct Component;

impl archust_plugin_sdk::Guest for Component {
    fn init() {
        info("DLSite Metadata plugin initialized");
        
        // Just read the auto-fetch setting, don't try to load yet (no archive open)
        let auto_fetch = archust_plugin_sdk::arclain::plugin::host::get_setting("auto_fetch_enabled")
            .unwrap_or_else(|| "true".to_string()) == "true";
        let enable_cache = archust_plugin_sdk::arclain::plugin::host::get_setting("enable_cache")
            .unwrap_or_else(|| "true".to_string()) == "true";
        
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.auto_fetch_enabled = auto_fetch;
            s.enable_cache = enable_cache;
        });
        
        // NOTE: Auto-load happens when archive is opened, not at init time
    }

    fn get_default_rules() -> Vec<archust_plugin_sdk::arclain::plugin::rules::PluginRuleDefinition> {
        use archust_plugin_sdk::arclain::plugin::rules::*;

        vec![PluginRuleDefinition {
            name: "DLSite Archive".to_string(),
            category: "Game".to_string(),
            description: Some("Organizes DLSite game archives logic".to_string()),
            trigger: PluginRuleTrigger {
                filename_pattern: None,
                has_file: None,
                extensions: None,
                min_size: None,
                max_size: None,
                metadata_source: Some("dlsite".to_string()),
            },
            actions: PluginRuleActions {
                root_folder: Some("$maker_name/$work_name".to_string()),
                move_files: vec![],
                move_to: None,
                rename_pattern: None,
                organize_content: true,
                delete_original: false,
                use_standard_layout: true,
            },
        }]
    }

    fn get_ui_layout(
        extension_point: String,
    ) -> Vec<archust_plugin_sdk::arclain::plugin::ui::UiElement> {
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match extension_point.as_str() {
            "PluginButton" => {
                // Fetch button - always visible for layout registration
                let mut buttons = vec![
                    UiElement::Button(ButtonConfig {
                        id: "fetch_metadata".to_string(),
                        label: "Fetch DLSite".to_string(),
                        action: None,
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "view_cache".to_string(),
                        label: "View Cache".to_string(),
                        action: Some(ButtonAction::ShowDialog("dlsite_cache".to_string())),
                    }),
                ];
                
                // DLSite Info button - only shown when we have metadata
                let has_metadata = STATE.with(|s| s.borrow().found_metadata.is_some());
                if has_metadata {
                    buttons.push(UiElement::Button(ButtonConfig {
                        id: "show_dlsite_info".to_string(),
                        label: "DLSite Info".to_string(),
                        action: Some(ButtonAction::ShowDialog("dlsite_info".to_string())),
                    }));
                }
                
                buttons
            },
            "MainPage" => {
                let auto_fetch_enabled = STATE.with(|s| s.borrow().auto_fetch_enabled);
                let enable_cache = STATE.with(|s| s.borrow().enable_cache);
                
                let mut elements = vec![
                    // Master switch: auto-fetch when archive opens
                    UiElement::Checkbox(CheckboxConfig {
                        id: "auto_fetch_enabled".to_string(),
                        label: "Auto-fetch metadata when archive opens".to_string(),
                        checked: auto_fetch_enabled,
                    }),
                ];
                
                // Only show cache option if auto-fetch is enabled
                if auto_fetch_enabled {
                    elements.push(UiElement::Checkbox(CheckboxConfig {
                        id: "enable_cache".to_string(),
                        label: "Cache fetched metadata".to_string(),
                        checked: enable_cache,
                    }));
                }
                
                elements.push(UiElement::TextInput(TextInputConfig {
                    id: "request_timeout".to_string(),
                    label: "API Request Timeout (seconds)".to_string(),
                    value: "30".to_string(),
                }));
                
                elements
            },
            "Panel" => {
                use archust_plugin_sdk::current_archive_info;
                
                // Check if archive is open
                let archive_info = current_archive_info();
                if archive_info.is_none() {
                    return vec![
                        UiElement::Label(LabelConfig {
                            text: "DLSite Metadata".to_string(),
                            bold: true,
                            size: Some(16.0),
                        }),
                        UiElement::Label(LabelConfig {
                            text: "No archive open".to_string(),
                            bold: false,
                            size: None,
                        }),
                    ];
                }
                
                // Get archive path to detect changes
                let archive_path = archive_info.as_ref()
                    .map(|i| i.path.clone())
                    .unwrap_or_default();
                
                // Reset state if archive changed
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    if s.last_archive_path.as_ref() != Some(&archive_path) {
                        // Archive changed - reset state
                        s.found_metadata = None;
                        s.search_query.clear();
                        s.search_results.clear();
                        s.fetch_in_progress = false;
                        s.last_archive_path = Some(archive_path.clone());
                    }
                });
                
                // Check if DLSite code can be detected from filename
                let archive_name = archive_info.as_ref()
                    .map(|i| i.filename.clone())
                    .unwrap_or_default();
                let detected_code = detect_dlsite_code(&archive_name);
                
                // NOTE: Auto-fetch is NOT done here because get_ui_layout runs on main thread.
                // Auto-fetch should be triggered via proper async event mechanism.
                
                let mut elements = vec![];

                STATE.with(|state| {
                    let state = state.borrow();

                    if let Some((id, data, scraped)) = &state.found_metadata {
                        // Metadata found - show info
                        let title = data["work_name"].as_str().unwrap_or("Unknown Title");
                        let maker = data["maker_name"].as_str().unwrap_or("Unknown");
                        
                        // Cover image at top (if available from scraped data)
                        if let Some(scraped_data) = scraped {
                            if let Some(cover_url) = &scraped_data.cover_image {
                                elements.push(UiElement::Image(ImageConfig {
                                    cache_key: Some(format!("dlsite:cover:{}", id)),
                                    url: Some(cover_url.clone()),
                                    max_height: Some(150.0),
                                }));
                            }
                        }
                        
                        elements.push(UiElement::Label(LabelConfig {
                            text: title.to_string(),
                            bold: true,
                            size: Some(14.0),
                        }));
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Circle: {}", maker),
                            bold: false,
                            size: None,
                        }));
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("ID: {}", id),
                            bold: false,
                            size: None,
                        }));
                        
                        // Release date if available
                        if let Some(date) = data["regist_date"].as_str() {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Released: {}", date),
                                bold: false,
                                size: None,
                            }));
                        }
                        
                        // Price removed from Panel - only shown in full info dialog
                        
                        // Tags from scraped data
                        if let Some(scraped_data) = scraped {
                            if !scraped_data.tags.is_empty() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Tags: {}", scraped_data.tags.join(", ")),
                                    bold: false,
                                    size: None,
                                }));
                            }
                        }
                        
                        elements.push(UiElement::Separator);
                        
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "show_dlsite_info".to_string(),
                            label: "Show Full Info".to_string(),
                            action: Some(ButtonAction::ShowDialog("dlsite_info".to_string())),
                        }));
                        
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "fetch_metadata".to_string(),
                            label: "Refresh".to_string(),
                            action: None,
                        }));
                    } else {
                        // No metadata yet
                        if let Some(code) = &detected_code {
                            // Code detected - show fetch button
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Detected: {}", code),
                                bold: false,
                                size: None,
                            }));
                            
                            elements.push(UiElement::Button(ButtonConfig {
                                id: "fetch_metadata".to_string(),
                                label: "Fetch DLSite Data".to_string(),
                                action: None,
                            }));
                        } else {
                            // No code detected - show manual search
                            elements.push(UiElement::Label(LabelConfig {
                                text: "No DLSite code detected".to_string(),
                                bold: false,
                                size: None,
                            }));
                            
                            elements.push(UiElement::Button(ButtonConfig {
                                id: "open_search".to_string(),
                                label: "Search Manually".to_string(),
                                action: Some(ButtonAction::ShowDialog("dlsite_search".to_string())),
                            }));
                        }
                    }
                });

                elements
            }

            "InfoPanel" => vec![
                UiElement::Label(LabelConfig {
                    text: "DLSite Info".to_string(),
                    bold: true,
                    size: None,
                }),
            ],
            
            // Dialog for full DLSite info
            ext if ext.starts_with("Dialog:") => {
                let dialog_id = ext.trim_start_matches("Dialog:");
                
                if dialog_id == "dlsite_info" {
                    let mut elements = vec![];
                    
                    STATE.with(|state| {
                        let state = state.borrow();
                        
                        if let Some((id, data, scraped)) = &state.found_metadata {
                            // Title
                            let title = data["work_name"].as_str().unwrap_or("Unknown Title");
                            elements.push(UiElement::Label(LabelConfig {
                                text: title.to_string(),
                                bold: true,
                                size: Some(18.0),
                            }));
                            
                            elements.push(UiElement::Separator);
                            
                            // Circle/Maker
                            let maker = data["maker_name"].as_str().unwrap_or("Unknown");
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Circle: {}", maker),
                                bold: false,
                                size: None,
                            }));
                            
                            // Product ID
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Product ID: {}", id),
                                bold: false,
                                size: None,
                            }));
                            
                            // Release date
                            if let Some(date) = data["regist_date"].as_str() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Release Date: {}", date),
                                    bold: false,
                                    size: None,
                                }));
                            }
                            
                            // Price
                            if let Some(price) = data["price"].as_u64() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Price: ¥{}", price),
                                    bold: false,
                                    size: None,
                                }));
                            }
                            
                            // Age rating
                            if let Some(rating) = data["age_category_string"].as_str() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Age Rating: {}", rating),
                                    bold: false,
                                    size: None,
                                }));
                            }
                            
                            // File count
                            if let Some(count) = data["file_count"].as_str() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("File Count: {}", count),
                                    bold: false,
                                    size: None,
                                }));
                            }
                            
                            // File size
                            if let Some(size) = data["file_size"].as_str() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("File Size: {}", size),
                                    bold: false,
                                    size: None,
                                }));
                            }
                            
                            elements.push(UiElement::Separator);
                            
                            // Tags from scraped data
                            if let Some(scraped_data) = scraped {
                                if !scraped_data.tags.is_empty() {
                                    elements.push(UiElement::Label(LabelConfig {
                                        text: "Tags:".to_string(),
                                        bold: true,
                                        size: None,
                                    }));
                                    elements.push(UiElement::Label(LabelConfig {
                                        text: scraped_data.tags.join(", "),
                                        bold: false,
                                        size: None,
                                    }));
                                }
                                
                                // Description if available
                                if let Some(desc) = &scraped_data.description {
                                    if !desc.is_empty() {
                                        elements.push(UiElement::Separator);
                                        elements.push(UiElement::Label(LabelConfig {
                                            text: "Description:".to_string(),
                                            bold: true,
                                            size: None,
                                        }));
                                        // Truncate long descriptions
                                        let desc_text = if desc.len() > 500 {
                                            format!("{}...", &desc[..500])
                                        } else {
                                            desc.clone()
                                        };
                                        elements.push(UiElement::Label(LabelConfig {
                                            text: desc_text,
                                            bold: false,
                                            size: None,
                                        }));
                                    }
                                }
                            }
                            
                            elements.push(UiElement::Separator);
                            
                            // Close button
                            elements.push(UiElement::Button(ButtonConfig {
                                id: "close_dialog".to_string(),
                                label: "Close".to_string(),
                                action: Some(ButtonAction::CloseDialog),
                            }));
                        } else {
                            elements.push(UiElement::Label(LabelConfig {
                                text: "No metadata available".to_string(),
                                bold: false,
                                size: None,
                            }));
                        elements.push(UiElement::Button(ButtonConfig {
                                id: "close_dialog".to_string(),
                                label: "Close".to_string(),
                                action: Some(ButtonAction::CloseDialog),
                            }));
                        }
                    });
                    
                    elements
                } else if dialog_id == "dlsite_search" {
                    // Search dialog
                    let mut elements = vec![];
                    
                    elements.push(UiElement::Label(LabelConfig {
                        text: "Search DLSite".to_string(),
                        bold: true,
                        size: Some(16.0),
                    }));
                    
                    elements.push(UiElement::Separator);
                    
                    STATE.with(|state| {
                        let state = state.borrow();
                        
                        elements.push(UiElement::TextInput(TextInputConfig {
                            id: "search_query".to_string(),
                            label: "Search Query".to_string(),
                            value: state.search_query.clone(),
                        }));
                    });
                    
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "perform_search".to_string(),
                        label: "Search".to_string(),
                        action: None,
                    }));
                    
                    // Show search results if any
                    STATE.with(|state| {
                        let state = state.borrow();
                        
                        if !state.search_results.is_empty() {
                            elements.push(UiElement::Separator);
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("{} results:", state.search_results.len()),
                                bold: true,
                                size: None,
                            }));
                            
                            for (code, title, maker) in &state.search_results {
                                elements.push(UiElement::Button(ButtonConfig {
                                    id: format!("select_result_{}", code),
                                    label: format!("[{}] {} ({})", code, title, maker),
                                    action: None,
                                }));
                            }
                        }
                    });
                    
                    elements.push(UiElement::Separator);
                    
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "close_search_dialog".to_string(),
                        label: "Cancel".to_string(),
                        action: Some(ButtonAction::CloseDialog),
                    }));
                    
                    elements
                // Cache viewer dialog
                } else if dialog_id == "dlsite_cache" {
                    use archust_plugin_sdk::list_cached_entries;
                    let mut elements = vec![];

                    elements.push(UiElement::Label(LabelConfig {
                        text: "DLSite Metadata Cache".to_string(),
                        bold: true,
                        size: Some(16.0),
                    }));
                    
                    elements.push(UiElement::Separator);

                    let entries = list_cached_entries().unwrap_or_else(|e| {
                         info(&format!("Failed to list cache: {}", e));
                         vec![]
                    });

                    if entries.is_empty() {
                         elements.push(UiElement::Label(LabelConfig {
                            text: "Cache is empty".to_string(),
                            bold: false,
                            size: None,
                        }));
                    } else {
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("{} cached entries", entries.len()),
                            bold: false,
                            size: None,
                        }));
                         
                        // Limit to top 50 for performance
                        for id in entries.iter().take(50) {
                            elements.push(UiElement::Button(ButtonConfig {
                                id: format!("cache_entry_{}", id),
                                label: id.clone(),
                                action: None, // TODO: Open details for this entry
                            }));
                        }
                    }

                    elements.push(UiElement::Separator);
                    
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "close_cache_dialog".to_string(),
                        label: "Close".to_string(),
                        action: Some(ButtonAction::CloseDialog),
                    }));

                    elements
                } else {
                    vec![]
                }
            }
            
            _ => vec![],
        }
    }



    fn on_ui_event(id: String, value: Option<String>) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
        // Handle system events dispatched as UI events
        if id == "event:archive_opened" {
            let path = value.unwrap_or_default();
            info(&format!("[DLSite Plugin] Archive opened event: {}", path));

            let auto_fetch = STATE.with(|s| s.borrow().auto_fetch_enabled);
            if auto_fetch {
                info("[DLSite Plugin] Auto-fetch enabled, scanning...");
                // Note: performing scan on background thread (host spawned thread for dispatch)
                match perform_scan() {
                    Ok(Some((id, _, _))) => {
                        info(&format!("[DLSite Plugin] Auto-fetched metadata for {}", id));
                    }
                    Ok(None) => info("[DLSite Plugin] No metadata found"),
                    Err(e) => info(&format!("[DLSite Plugin] Scan failed: {}", e)),
                }
            }
            return vec![];
        }

        info(&format!(
            "[DLSite Plugin] on_ui_event called: id={}, value={:?}",
            id, value
        ));

        if id.starts_with("select_result_") {
            let code = id.trim_start_matches("select_result_").to_string();
            info(&format!("[DLSite Plugin] Selected result: {}", code));
            
            STATE.with(|state| {
                let mut s = state.borrow_mut();
                s.search_results.clear();
                s.search_query.clear();
            });
            
            // Re-use logic to fetch and emit (Data API caches transparently)
            if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
                let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
                archust_plugin_sdk::emit_metadata(&metadata_json);
                
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.found_metadata = Some((code.clone(), json, scraped));
                });
            }
            
            // Dialog will be closed when user selects a result
            return vec![];
        }

        match id.as_str() {
            "auto_fetch_enabled" => {
                if let Some(val) = value {
                    let enabled = val == "true";
                    STATE.with(|state| {
                        state.borrow_mut().auto_fetch_enabled = enabled;
                    });
                    archust_plugin_sdk::arclain::plugin::host::set_setting("auto_fetch_enabled", &val);
                    info(&format!("[DLSite Plugin] Auto-fetch setting changed to: {}", enabled));
                }
            }
            "enable_cache" => {
                if let Some(val) = value {
                    let enabled = val == "true";
                    STATE.with(|state| {
                        state.borrow_mut().enable_cache = enabled;
                    });
                    archust_plugin_sdk::arclain::plugin::host::set_setting("enable_cache", &val);
                    info(&format!("[DLSite Plugin] Cache setting changed to: {}", enabled));
                }
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
                    let results = search_dlsite(&query);
                    
                    STATE.with(|state| {
                        state.borrow_mut().search_results = results;
                    });
                }
            }
            "fetch_metadata" => {
                // Debounce - prevent spam clicking
                let already_in_progress = STATE.with(|state| {
                    let s = state.borrow();
                    s.fetch_in_progress
                });
                
                if already_in_progress {
                    info("[DLSite Plugin] Fetch already in progress, ignoring");
                    return vec![];
                }
                
                // Mark as in progress
                STATE.with(|state| {
                    state.borrow_mut().fetch_in_progress = true;
                });
                
                info("[DLSite Plugin] Handling fetch_metadata");

                match perform_scan() {
                    Ok(Some((product_id, json, scraped))) => {
                        info("[DLSite Plugin] Metadata found");
                        
                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.found_metadata = Some((product_id, json, scraped));
                            s.fetch_in_progress = false;
                        });
                    }
                    Ok(None) => {
                        info("[DLSite Plugin] No metadata found");
                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.fetch_in_progress = false;
                        });
                    }
                    Err(e) => {
                        info(&format!("[DLSite Plugin] Scan failed: {}", e));
                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.fetch_in_progress = false;
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
        
        vec![]
    }
}

fn perform_scan() -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
    use archust_plugin_sdk::{current_archive_info, info, list_archive_files};

    let info_data = current_archive_info().ok_or("No archive open")?;
    info(&format!(
        "[DLSite Plugin] Scanning archive: {}",
        info_data.filename
    ));

    // Helper to process found code
    // Data API handles caching transparently via fetch_string_blocking
    let process_code = |code: String| -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
        info(&format!("[DLSite Plugin] Found code: {}", code));
        
        // Fetch from network (Data API handles caching transparently)
        if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
            // Generate final JSON and emit
            let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
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
    use archust_plugin_sdk::{fetch_string_blocking, log_network_activity};

    // 1. Fetch JSON API
    let api_url = format!(
        "https://www.dlsite.com/home/api/=/product.json?work_no={}",
        product_id
    );
    let cache_key = format!("dlsite:json:{}", product_id);

    log_network_activity(&format!("Fetching metadata for {} from DLSite API...", product_id));
    log_network_activity(&format!("GET {}", api_url));

    let json_data = match fetch_string_blocking(&cache_key, &api_url) {
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
    let html_key = format!("dlsite:html:{}", product_id);

    log_network_activity(&format!("Fetching HTML page for scraping..."));
    log_network_activity(&format!("GET {}", html_url));

    let scraped_data = match fetch_string_blocking(&html_key, &html_url) {
        Ok(html) => {
            log_network_activity(&format!("Response: {} bytes", html.len()));
            scrape_html_metadata(&html)
        },
        Err(e) => {
            log_network_activity(&format!("Failed to fetch HTML: {}", e));
            None
        }
    };

    // If we have a cover image, fetch and cache it now (for UI display)
    if let Some(data) = &scraped_data {
        if let Some(cover_url) = &data.cover_image {
            let cover_key = format!("dlsite:cover:{}", product_id);
            // Use fetch_blocking to download bytes and let Data API cache it
            // We ignore the result, rely on side-effect of caching
            use archust_plugin_sdk::ResourceType;
            // logging helper imported at top of file, or use SDK prefix? 
            // The function uses log_network_activity mostly, but on_event uses info.
            // Let's use log_network_activity for consistency in this function
            log_network_activity(&format!("Fetching cover image: {}", cover_url));
            
            if let Err(e) = archust_plugin_sdk::fetch_blocking(&cover_key, cover_url, ResourceType::Image) {
                 log_network_activity(&format!("Failed to fetch/cache cover image: {}", e));
            }
        }
    }

    Some((json_data, scraped_data))
}

#[derive(Debug, Clone)]
struct ScrapedData {
    title: Option<String>,
    circle: Option<String>,
    release_date: Option<String>,
    tags: Vec<String>,
    description: Option<String>,
    cover_image: Option<String>,
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
    let mut cover_image = None;
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
            "サークル名" | "ブランド名" | "著者" | "出版社名" => {
                // Circle/Maker
                if let Some(maker) = td.select(&maker_selector).next() {
                    circle = Some(maker.text().collect::<String>().trim().to_string());
                }
            }
            "販売日" => {
                // Release date
                release_date = Some(td.text().collect::<String>().trim().to_string());
            }
            "ジャンル" => {
                // Tags/Genres
                for a in td.select(&a_selector) {
                    let tag = a.text().collect::<String>().trim().to_string();
                    if !tag.is_empty() {
                        tags.push(tag);
                    }
                }
            }
            _ => {}
        }
    }
    
    // 2. Parse Description (from meta or work_parts)
    let meta_selector = Selector::parse("meta[name='description']").unwrap();
    if let Some(meta) = document.select(&meta_selector).next() {
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

    // 4. Parse Main Cover Image
    // Try to find the main product image
    let img_selector = Selector::parse("div.product-slider-data div[data-src]").unwrap();
    if let Some(div) = document.select(&img_selector).next() {
        if let Some(src) = div.value().attr("data-src") {
            let full_url = if src.starts_with("//") {
                format!("https:{}", src)
            } else {
                src.to_string()
            };
            cover_image = Some(full_url);
        }
    }
    
    // Fallback: try to find the work_img element
    if cover_image.is_none() {
        let work_img_selector = Selector::parse("div#work_left img, img.work_img").unwrap();
        if let Some(img) = document.select(&work_img_selector).next() {
            if let Some(src) = img.value().attr("src") {
                let full_url = if src.starts_with("//") {
                    format!("https:{}", src)
                } else {
                    src.to_string()
                };
                cover_image = Some(full_url);
            }
        }
    }

    // 5. Parse Screenshots
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
        cover_image,
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
    use archust_plugin_sdk::{fetch_string_blocking, log_network_activity};
    use scraper::{Html, Selector};

    let url = format!(
        "https://www.dlsite.com/home/fsr/=/keyword/{}",
        urlencoding::encode(query)
    );
    let key = format!("dlsite:search:{}", urlencoding::encode(query));
    
    log_network_activity(&format!("Searching DLSite: {}", query));
    log_network_activity(&format!("GET {}", url));

    let html = match fetch_string_blocking(&key, &url) {
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
