use archust_plugin_sdk::info;
use std::cell::RefCell;

// Plugin state to store found metadata
struct PluginState {
    found_metadata: Option<(String, serde_json::Value, Option<ScrapedData>)>, // (product_id, json, scraped)
    search_query: String,
    search_results: Vec<(String, String, String)>, // (code, title, maker)
    auto_fetch_enabled: bool, // Master switch: auto-fetch when archive opens
    enable_cache: bool, // Sub-option: cache fetched results (only relevant if auto_fetch enabled)
    cache_images: bool, // Cache cover and screenshot images
    fetch_in_progress: bool, // Prevent double-fetch when spamming buttons
    cached_entries: Option<Vec<String>>, // Cache of checking the cache (UI spam prevention)
    selected_cache_entry: Option<String>, // For cache viewer details
    last_archive_path: Option<String>, // Track current archive to reset state on change
    // Browser UI state
    browser_tab: String, // "cached" or "search"
    browser_loading: bool,
    // Cache for browser detail view to prevent fetch loop
    browser_detail_cache: Option<(String, serde_json::Value, Option<ScrapedData>)>,
    current_image_index: i32,
}

// Global state (thread-local for WASM component)
thread_local! {
    static STATE: RefCell<PluginState> = RefCell::new(PluginState {
        found_metadata: None,
        search_query: String::new(),
        search_results: Vec::new(),
        auto_fetch_enabled: true,
        enable_cache: true,
        cache_images: true,
        fetch_in_progress: false,
        last_archive_path: None,
        cached_entries: None,
        selected_cache_entry: None,
        browser_tab: "cached".to_string(),
        browser_loading: false,
        browser_detail_cache: None,
        current_image_index: -1, // -1 = Cover, 0+ = Sample index
    });
}

struct Component;

fn format_description(text: &str) -> String {
    let s = text.trim();
    
    // Check if text is flattened (few newlines)
    let newlines = s.chars().filter(|c| *c == '\n').count();
    // If length is substantial but newlines are rare, it's likely flat
    let is_flat = newlines < 3 && s.len() > 200;

    let mut result = s.to_string();
    if is_flat {
        result = result.replace("■", "\n\n■")
            .replace("●", "\n●")
            .replace("★", "\n★")
            .replace("・", "\n・");
    }
    
    result
}

impl archust_plugin_sdk::Guest for Component {
    fn init() {
        info("DLSite Metadata plugin initialized");
        
        // Read plugin settings
        let auto_fetch = archust_plugin_sdk::arclain::plugin::host::get_setting("auto_fetch_enabled")
            .unwrap_or_else(|| "true".to_string()) == "true";
        let enable_cache = archust_plugin_sdk::arclain::plugin::host::get_setting("enable_cache")
            .unwrap_or_else(|| "true".to_string()) == "true";
        let cache_images = archust_plugin_sdk::arclain::plugin::host::get_setting("cache_images")
            .unwrap_or_else(|| "true".to_string()) == "true";
        
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.auto_fetch_enabled = auto_fetch;
            s.enable_cache = enable_cache;
            s.cache_images = cache_images;
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
    ) -> archust_plugin_sdk::arclain::plugin::ui::PluginLayout {
        use archust_plugin_sdk::arclain::plugin::ui::{
            PluginLayout, SplitConfig, UiElement, LabelConfig, TabsConfig, TextInputConfig,
            ButtonConfig, ButtonAction, ListContainerConfig, ListItemConfig, ImageConfig, LoadingConfig,
            CheckboxConfig, WarningConfig, WarningIcon, TagChipsConfig, ToolbarConfig, ToolbarButtonConfig
        };

        match extension_point.as_str() {
            "PluginButton" => {
                use archust_plugin_sdk::current_archive_info;
                
                // Fetch button - only if archive is open
                let mut buttons = vec![];
                if current_archive_info().is_some() {
                    buttons.push(UiElement::Button(ButtonConfig {
                        id: "fetch_metadata".to_string(),
                        label: "Fetch DLSite".to_string(),
                        action: None,
                    }));
                }
                
                buttons.push(UiElement::Button(ButtonConfig {
                    id: "view_cache".to_string(),
                    label: "Browse DLSite".to_string(),
                    action: Some(ButtonAction::OpenPage("dlsite_browser".to_string())),
                }));
                
                // DLSite Info button - only shown when we have metadata
                let has_metadata = STATE.with(|s| s.borrow().found_metadata.is_some());
                if has_metadata {
                    buttons.push(UiElement::Button(ButtonConfig {
                        id: "show_dlsite_info".to_string(),
                        label: "DLSite Info".to_string(),
                        action: Some(ButtonAction::ShowDialog("dlsite_info".to_string())),
                    }));
                }
                
                PluginLayout::Single(buttons)
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
                
                PluginLayout::Single(elements)
            },
            "Panel" => {
                use archust_plugin_sdk::current_archive_info;
                
                // Check if archive is open
                let archive_info = current_archive_info();
                if archive_info.is_none() {
                    return PluginLayout::Single(vec![
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
                    ]);
                }
                
                // Get archive path to detect changes
                let archive_path = archive_info.as_ref()
                    .map(|i| i.path.clone())
                    .unwrap_or_default();
                
                // Check if DLSite code can be detected from filename
                let archive_name = archive_info.as_ref()
                    .map(|i| i.filename.clone())
                    .unwrap_or_default();
                let detected_code = detect_dlsite_code(&archive_name);
                
                // Reset state if archive changed, then check cache for detected code
                let archive_changed = STATE.with(|state| {
                    let s = state.borrow();
                    s.last_archive_path.as_ref() != Some(&archive_path)
                });
                
                if archive_changed {
                    // Reset state first
                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.found_metadata = None;
                        s.search_query.clear();
                        s.search_results.clear();
                        s.fetch_in_progress = false;
                        s.last_archive_path = Some(archive_path.clone());
                    });
                    
                    // If we have a detected code, check cache and emit to signal if found
                    if let Some(ref code) = detected_code {
                        if let Some((api_data, scraped)) = get_cached_dlsite_metadata(code) {
                            // Found in cache! Populate state
                            STATE.with(|state| {
                                let mut s = state.borrow_mut();
                                s.found_metadata = Some((code.clone(), api_data.clone(), scraped.clone()));
                            });
                            
                            // Emit to signal so Panel reads from it
                            let metadata_json = generate_metadata_json(code, Some(&(api_data, scraped)));
                            archust_plugin_sdk::arclain::plugin::host::emit_metadata(&metadata_json);
                        }
                    }
                }
                
                let mut elements = vec![];

                STATE.with(|state| {
                    let state = state.borrow();

                    if let Some((id, data, scraped)) = &state.found_metadata {
                        // Metadata found - show info
                        // Handle both raw API format (work_name) and ProductMetadata format (title)

                        let title = data["work_name"].as_str()
                            .or_else(|| data["title"].as_str());
                            
                        // Maker/Circle
                        let maker = data["maker_name"].as_str()
                            .or_else(|| data["circle"].as_str())
                            .or_else(|| data["creator"].as_str());
                        
                        // Show warning if geo-blocked
                        if let Some(scraped_data) = scraped {
                            if scraped_data.geo_blocked {
                                elements.push(UiElement::Warning(WarningConfig {
                                    icon: WarningIcon::GlobeX,
                                    message: "This product is geo-blocked in your region. Metadata may be incomplete.".to_string(),
                                }));
                            }
                        }

                        // Cover image at top - try to display from cache or scraped URL
                        let cover_cache_key = metastore_providers::dlsite::cache_keys::cover_key(id);
                        let cover_url = scraped.as_ref()
                            .and_then(|s| s.cover_image.clone());
                        
                        // Always attempt to show cover - the host will check cache by key
                        elements.push(UiElement::Image(ImageConfig {
                            cache_key: Some(cover_cache_key),
                            url: cover_url, // May be None, host will use cache key
                            max_height: Some(150.0),
                        }));
                        
                        if let Some(t) = title {
                            elements.push(UiElement::Label(LabelConfig {
                                text: t.to_string(),
                                bold: true,
                                size: Some(14.0),
                            }));
                        }
                        
                        if let Some(m) = maker {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Circle: {}", m),
                                bold: false,
                                size: None,
                            }));
                        }
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("ID: {}", id),
                            bold: false,
                            size: None,
                        }));
                        
                        // Release date if available (check both API and ProductMetadata field names)
                        let release_date = data["regist_date"].as_str()
                            .or_else(|| data["release_date"].as_str());
                        if let Some(date) = release_date {
                            if !date.is_empty() {
                                // Strip time component if present (e.g. "2026-03-06 00:00:00" -> "2026-03-06")
                                let date_clean = date.split_whitespace().next().unwrap_or(date);
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Released: {}", date_clean),
                                    bold: false,
                                    size: None,
                                }));
                            }
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
                        
                        // Navigate to DLSite browser page with product ID
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "show_in_browser".to_string(),
                            label: "View in DLSite Browser".to_string(),
                            action: Some(ButtonAction::OpenPage(format!("dlsite_browser:{}", id))),
                        }));
                        
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

                PluginLayout::Single(elements)
            }

            "InfoPanel" => PluginLayout::Single(vec![
                UiElement::Label(LabelConfig {
                    text: "DLSite Info".to_string(),
                    bold: true,
                    size: None,
                }),
            ]),
            
            // Dialog for full DLSite info
            ext if ext.starts_with("Dialog:") => {
                let dialog_id = ext.trim_start_matches("Dialog:");
                
                if dialog_id == "dlsite_info" {
                    let mut elements = vec![];
                    
                    STATE.with(|state| {
                        let state = state.borrow();
                        
                        if let Some((id, data, scraped)) = &state.found_metadata {
                            // Title (check both raw API and ProductMetadata field names)
                            let title = data["work_name"].as_str()
                                .or_else(|| data["title"].as_str());
                                
                            if let Some(t) = title {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: t.to_string(),
                                    bold: true,
                                    size: Some(18.0),
                                }));
                            }
                            
                            elements.push(UiElement::Separator);
                            
                            // Circle/Maker (check both field names)
                            let maker = data["maker_name"].as_str()
                                .or_else(|| data["creator"].as_str());
                                
                            if let Some(m) = maker {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Circle: {}", m),
                                    bold: false,
                                    size: None,
                                }));
                            }
                            
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
                    
                    PluginLayout::Single(elements)
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
                    
                    PluginLayout::Single(elements)
                // Cache viewer dialog
                } else if dialog_id == "dlsite_cache" {
                    use archust_plugin_sdk::list_cached_entries;
                    let mut elements = vec![];

                    // Check if we have a selected entry to show details for
                    let selected = STATE.with(|s| s.borrow().selected_cache_entry.clone());
                    
                    if let Some(entry_id) = selected {
                        // === DETAIL VIEW ===
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "back_to_cache_list".to_string(),
                            label: "< Back to List".to_string(),
                            action: None,
                        }));
                        
                        elements.push(UiElement::Separator);
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Details: {}", entry_id),
                            bold: true,
                            size: Some(18.0),
                        }));

                        // Read the data from cache to display (no network fetch!)
                        info(&format!("[DLSite Plugin] get_ui_layout detail view for entry_id={}", entry_id));
                         if let Some((json, scraped)) = get_cached_dlsite_metadata(&entry_id) {
                            info("[DLSite Plugin] get_cached_dlsite_metadata returned Some");
                            // Check geo-blocked status from stored JSON first (most reliable)
                            let json_geo_blocked = json.get("geo_blocked")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let scraped_geo_blocked = scraped.as_ref().map(|s| s.geo_blocked).unwrap_or(false);
                            let is_geo_blocked = json_geo_blocked || scraped_geo_blocked;
                            
                            info(&format!(
                                "[DLSite Plugin] Cache browser: json_geo_blocked={}, scraped_geo_blocked={}, is_geo_blocked={}",
                                json_geo_blocked, scraped_geo_blocked, is_geo_blocked
                            ));
                            
                            if is_geo_blocked {
                                info("[DLSite Plugin] Pushing Warning element to UI");
                                elements.push(UiElement::Warning(WarningConfig {
                                    icon: WarningIcon::GlobeX,
                                    message: "This product is geo-blocked. Metadata may be incomplete.".to_string(),
                                }));
                            }
                            
                            // Copy-paste of info display logic (could be refactored into helper)
                            let title = json["work_name"].as_str().unwrap_or("Unknown Title");
                            elements.push(UiElement::Label(LabelConfig { text: title.to_string(), bold: false, size: None }));
                            
                             if let Some(scraped_data) = scraped {
                                if let Some(cover_url) = &scraped_data.cover_image {
                                    elements.push(UiElement::Image(ImageConfig {
                                        cache_key: Some(metastore_providers::dlsite::cache_keys::cover_key(&entry_id)),
                                        url: Some(cover_url.clone()),
                                        max_height: Some(200.0),
                                    }));
                                }
                                if let Some(desc) = &scraped_data.description {
                                    elements.push(UiElement::Separator);
                                    elements.push(UiElement::Label(LabelConfig { text: desc.clone(), bold: false, size: None }));
                                }
                            }
                         } else {
                             elements.push(UiElement::Label(LabelConfig { text: "Failed to load details".to_string(), bold: false, size: None }));
                         }

                    } else {
                        // === LIST VIEW ===
                        elements.push(UiElement::Label(LabelConfig {
                            text: "DLSite Metadata Cache".to_string(),
                            bold: true,
                            size: Some(16.0),
                        }));
                        
                        elements.push(UiElement::Separator);
                        
                        // Search Box
                        STATE.with(|state| {
                            let state = state.borrow();
                            elements.push(UiElement::TextInput(TextInputConfig {
                                id: "search_query".to_string(),
                                label: "Filter Cache".to_string(),
                                value: state.search_query.clone(),
                            }));
                        });

                        // Refresh Button
                         elements.push(UiElement::Button(ButtonConfig {
                            id: "refresh_cache".to_string(),
                            label: "Refresh List".to_string(),
                            action: None,
                        }));

                        elements.push(UiElement::Separator);

                        // Always fetch fresh from host to see latest data
                        let entries = list_cached_entries().unwrap_or_else(|e| {
                            info(&format!("Failed to list cache: {}", e));
                            vec![]
                        });
                        
                        // Filter
                        let query = STATE.with(|s| s.borrow().search_query.to_lowercase());
                        let filtered_entries: Vec<_> = entries.iter()
                            .filter(|id| query.is_empty() || id.to_lowercase().contains(&query))
                            .collect();

                        if filtered_entries.is_empty() {
                             elements.push(UiElement::Label(LabelConfig {
                                text: "No matching entries".to_string(),
                                bold: false,
                                size: None,
                            }));
                        } else {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("{} entries", filtered_entries.len()),
                                bold: false,
                                size: None,
                            }));
                             
                            // Limit to top 50 for performance
                            for id in filtered_entries.iter().take(50) {
                                elements.push(UiElement::Button(ButtonConfig {
                                    id: format!("view_cache_entry_{}", id),
                                    label: id.to_string(),
                                    action: None,
                                }));
                            }
                        }
                    }

                    elements.push(UiElement::Separator);
                    
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "close_cache_dialog".to_string(),
                        label: "Close".to_string(),
                        action: Some(ButtonAction::CloseDialog),
                    }));

                    PluginLayout::Single(elements)
                } else {
                    PluginLayout::Single(vec![])
                }
            }
            
            // Handle Page:dlsite_browser:VJ012345 (navigation with product ID)
            ext if ext.starts_with("Page:dlsite_browser:") => {
                // Extract product ID from extension point
                let product_id = ext.trim_start_matches("Page:dlsite_browser:").to_string();
                
                // Set the selected entry in state so the browser shows the detail view
                STATE.with(|s| {
                    let mut state = s.borrow_mut();
                    state.selected_cache_entry = Some(product_id);
                });
                
                // Fall through to the normal browser rendering (which will now show detail view)
                // Re-call ourselves with the base extension point
                return Self::get_ui_layout("Page:dlsite_browser".to_string());
            }
            
            "Page:dlsite_browser" => {
                use archust_plugin_sdk::list_cached_entries;
                
                let (browser_tab, search_query, selected_entry, browser_loading) = STATE.with(|s| {
                    let state = s.borrow();
                    (
                        state.browser_tab.clone(),
                        state.search_query.clone(),
                        state.selected_cache_entry.clone(),
                        state.browser_loading,
                    )
                });
                
    // === Sidebar Generation ===
    let mut sidebar_elements = Vec::new();

    // 1. Title
    sidebar_elements.push(UiElement::Label(LabelConfig {
        text: "DLSite Browser".to_string(),
        bold: true,
        size: Some(20.0),
    }));
    
    sidebar_elements.push(UiElement::Separator);
    
    // 2. Tabs
    sidebar_elements.push(UiElement::Tabs(TabsConfig {
        id: "browser_tabs".to_string(),
        tabs: vec!["Cached".to_string(), "Search DLSite".to_string()],
        selected: if browser_tab == "search" { "Search DLSite".to_string() } else { "Cached".to_string() },
    }));
    
    sidebar_elements.push(UiElement::Separator);
    
    // 3. Search/Filter
    sidebar_elements.push(UiElement::TextInput(TextInputConfig {
        id: "browser_query".to_string(),
        label: if browser_tab == "search" { "Search DLSite...".to_string() } else { "Filter cache...".to_string() },
        value: search_query.clone(),
    }));
    
    if browser_tab == "search" {
        sidebar_elements.push(UiElement::Button(ButtonConfig {
            id: "do_dlsite_search".to_string(),
            label: "Search".to_string(),
            action: None,
        }));
    }
    
    sidebar_elements.push(UiElement::Separator);
    
    // 4. List Content
    if browser_loading {
        sidebar_elements.push(UiElement::Loading(LoadingConfig {
            message: Some("Searching...".to_string()),
        }));
    } else {
        if browser_tab == "search" {
            // Search Results
             let search_results = STATE.with(|s| s.borrow().search_results.clone());
             
             let items: Vec<ListItemConfig> = search_results.iter()
                 .filter(|(code, title, _)| {
                     search_query.is_empty() || 
                     title.to_lowercase().contains(&search_query.to_lowercase()) ||
                     code.to_lowercase().contains(&search_query.to_lowercase())
                 })
                 .map(|(code, title, maker)| ListItemConfig {
                     id: code.clone(),
                     title: title.clone(),
                     subtitle: Some(maker.clone()),
                     badge: Some(code.clone()),
                     image_key: None,
                     selected: selected_entry.as_ref() == Some(code),
                     warning_icon: None,
                 })
                 .collect();
             
             sidebar_elements.push(UiElement::ListContainer(ListContainerConfig {
                 id: "browser_list".to_string(),
                 items,
                 max_height: Some(700.0), // Taller list for sidebar
                 empty_message: Some("Search DLSite to see results".to_string()),
             }));
        } else {
             // Cached Entries - always fetch fresh from host to see latest data
             let entries = list_cached_entries().unwrap_or_else(|e| {
                 info(&format!("Failed to list cache: {}", e));
                 vec![]
             });
             
             // Filter and limit entries first
             let filtered_ids: Vec<String> = entries.iter()
                 .filter(|id| {
                     search_query.is_empty() || 
                     id.to_lowercase().contains(&search_query.to_lowercase())
                 })
                 .take(100)
                 .cloned()
                 .collect();
             
             // Batch query for all summaries at once (single DB query!)
             let summaries = archust_plugin_sdk::get_metadata_summaries(filtered_ids.clone());
             
             // Convert to our format
             let entries_with_summaries: Vec<(String, Option<String>, bool)> = summaries
                 .into_iter()
                 .map(|s| (s.id, s.title, s.geo_blocked))
                 .collect();
             
             let items: Vec<ListItemConfig> = entries_with_summaries.into_iter()
                 .map(|(id, title, geo_blocked)| {
                     let display_title = title.unwrap_or_else(|| id.clone());
                     let selected = selected_entry.as_ref() == Some(&id);
                     ListItemConfig {
                         id: format!("view_cache_entry_{}", id),
                         title: display_title,
                         subtitle: Some("Cached".to_string()),
                         badge: Some(id.clone()),
                         image_key: None,
                         selected,
                         warning_icon: if geo_blocked {
                             Some(WarningIcon::GlobeX)
                         } else {
                             None
                         },
                     }
                 })
                 .collect();
             
             sidebar_elements.push(UiElement::ListContainer(ListContainerConfig {
                 id: "browser_list".to_string(),
                 items,
                 max_height: Some(700.0),
                 empty_message: Some("No cached entries".to_string()),
             }));
        }
    }

    // === Content Generation ===
    let mut content_elements = Vec::new();
    
    if let Some(selected_id) = &selected_entry {
        // Check for state change
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            if let Some((cached_id, _, _)) = &state.browser_detail_cache {
                if cached_id != selected_id {
                    // ID changed - reset cache and UI state
                    state.browser_detail_cache = None;
                    state.current_image_index = -1;
                }
            }
        });

        // Retrieve loaded details
        let detail_data = STATE.with(|s| {
            let state = s.borrow();
            if let Some((cached_id, json, scraped)) = &state.browser_detail_cache {
                if cached_id == selected_id {
                    return Some((json.clone(), scraped.clone()));
                }
            }
            // Fallback: Check if found_metadata has it (from search scan)
            if let Some((scan_id, json, scraped)) = &state.found_metadata {
                 if scan_id == selected_id {
                     return Some((json.clone(), scraped.clone()));
                 }
            }
            None
        });
        
        if let Some((json, scraped)) = detail_data {
             // Check if geo-blocked
             let is_geo_blocked = json.get("geo_blocked")
                 .and_then(|v| v.as_bool())
                 .unwrap_or(false)
                 || scraped.as_ref().map(|s| s.geo_blocked).unwrap_or(false);
             
             if is_geo_blocked {
                 content_elements.push(UiElement::Warning(WarningConfig {
                     icon: WarningIcon::GlobeX,
                     message: "This product is geo-blocked. Metadata may be incomplete.".to_string(),
                 }));
             }
             
             // Handle both DLSite API format (work_name, maker_name) and cache format (title, circle)
             let title = json["title"].as_str()
                 .or(json["work_name"].as_str())
                 .unwrap_or("Unknown Title");
             let maker = json["maker_name"].as_str()
                 .or(json["brand"].as_str())
                 .or(json["circle"].as_str())
                 .or(json["creator"].as_str())
                 .unwrap_or("Unknown Maker");
             let release_date = json["release_date"].as_str()
                 .or(json["regist_date"].as_str());
             
             // ===== TOOLBAR AT TOP =====
             content_elements.push(UiElement::Toolbar(ToolbarConfig {
                 buttons: vec![
                     ToolbarButtonConfig {
                         id: format!("refresh_view_{}", selected_id),
                         label: "Refresh".to_string(),
                         icon: None,
                         primary: false,
                     },
                     ToolbarButtonConfig {
                         id: format!("refetch_entry_{}", selected_id),
                         label: "Refetch".to_string(),
                         icon: None,
                         primary: false,
                     },
                     ToolbarButtonConfig {
                         id: format!("select_entry_{}", selected_id),
                         label: "Select for Use".to_string(),
                         icon: None,
                         primary: true,
                     },
                 ],
             }));
             content_elements.push(UiElement::Separator);
             
             // ===== TITLE (Hero) =====
             content_elements.push(UiElement::Label(LabelConfig {
                 text: title.to_string(),
                 bold: true,
                 size: Some(24.0),
             }));
             
             content_elements.push(UiElement::Space(8.0));
             
             // ===== HERO IMAGE WITH NAVIGATION =====
             // Get current image index and sample count
             let (current_idx, samples) = STATE.with(|s| {
                 let state = s.borrow();
                 let samples = json["sample_images"].as_array().cloned().unwrap_or_default();
                 (state.current_image_index, samples)
             });
             
             let samples_count = samples.len();
             
             // Navigation Toolbar (only if we have samples)
             if samples_count > 0 {
                 let status_label = if current_idx == -1 {
                     "Cover".to_string()
                 } else {
                     format!("Sample {}/{}", current_idx + 1, samples_count)
                 };
                 
                 content_elements.push(UiElement::Toolbar(ToolbarConfig {
                     buttons: vec![
                         ToolbarButtonConfig {
                             id: format!("prev_image_{}", selected_id),
                             label: "< Prev".to_string(),
                             icon: None,
                             primary: false,
                          },
                         ToolbarButtonConfig {
                             id: format!("reset_image_{}", selected_id), // Clicking label resets to cover
                             label: status_label,
                             icon: None,
                             primary: true, // Highlight current status
                          },
                         ToolbarButtonConfig {
                             id: format!("next_image_{}", selected_id),
                             label: "Next >".to_string(),
                             icon: None,
                             primary: false,
                          },
                     ],
                 }));
             }
             
             // Determine which image to show
             let (start_key, display_url) = if current_idx == -1 || samples_count == 0 {
                 // Show Cover
                 let url = scraped.as_ref().and_then(|s| s.cover_image.clone())
                     .or_else(|| {
                         // Fallback to first sample if no cover explicitly found? 
                         // Logic below handled this before, let's keep it consistent
                         samples.first().and_then(|v| v.as_str()).map(|s| s.to_string())
                     });
                 (Some(metastore_providers::dlsite::cache_keys::cover_key(selected_id)), url)
             } else {
                 // Show Sample
                 let idx = current_idx as usize;
                 let url = samples.get(idx).and_then(|v| v.as_str()).map(|s| s.to_string());
                 (Some(metastore_providers::dlsite::cache_keys::screenshot_key(selected_id, idx)), url)
             };
             
             if let Some(url) = display_url {
                 content_elements.push(UiElement::Image(ImageConfig {
                     cache_key: start_key,
                     url: Some(url),
                     max_height: Some(400.0), // Larger hero image
                 }));
             } else {
                 // Show placeholder
                 content_elements.push(UiElement::Label(LabelConfig {
                     text: "📷 [Image not available]".to_string(),
                     bold: false,
                     size: Some(12.0),
                 }));
             }
             
             content_elements.push(UiElement::Space(12.0));
             
             // ===== METADATA INFO (Compact Single Line) =====
             // ===== METADATA INFO (Rows) =====
             let release_str = release_date.unwrap_or("Unknown Release");
             
             content_elements.push(UiElement::Label(LabelConfig {
                 text: format!("ID: {}", selected_id),
                 bold: false,
                 size: Some(13.0),
             }));
             content_elements.push(UiElement::Label(LabelConfig {
                 text: format!("Released: {}", release_str),
                 bold: false,
                 size: Some(13.0),
             }));
             content_elements.push(UiElement::Label(LabelConfig {
                 text: format!("Circle: {}", maker),
                 bold: false,
                 size: Some(13.0),
             }));
             
             content_elements.push(UiElement::Separator);
             
             // ===== TAGS =====
             let tags: Vec<String> = scraped.as_ref()
                 .map(|s| s.tags.clone())
                 .filter(|t| !t.is_empty())
                 .or_else(|| {
                     // Try to parse tags_json from ProductMetadata
                     json["tags_json"].as_str()
                         .or_else(|| json["tags"].as_str())
                         .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                         .or_else(|| {
                             // Might be a direct array
                             json["tags"].as_array().map(|arr| {
                                 arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
                             })
                         })
                 })
                 .unwrap_or_default();
             
             if !tags.is_empty() {
                 content_elements.push(UiElement::Label(LabelConfig {
                     text: "Tags".to_string(),
                     bold: true,
                     size: Some(14.0),
                 }));
                 content_elements.push(UiElement::TagChips(TagChipsConfig {
                     tags: tags.clone(),
                     max_display: Some(15),
                 }));
                 content_elements.push(UiElement::Space(10.0));
             }
             
             // ===== DESCRIPTION =====
             let description = scraped.as_ref()
                 .and_then(|s| s.description.clone())
                 .or_else(|| json["description"].as_str().map(|s| s.to_string()));
             
             if let Some(desc) = description {
                 content_elements.push(UiElement::Label(LabelConfig {
                     text: "Description".to_string(),
                     bold: true,
                     size: Some(14.0),
                 }));
                 // Truncate long descriptions for readability (respecting UTF-8 char boundaries)
                 // Format description to restore structure
                 let formatted = format_description(&desc);
                 // Truncate if excessively long (2000 chars)
                 let desc_display = if formatted.len() > 2000 {
                      let truncate_at = formatted.char_indices()
                          .take_while(|(i, _)| *i <= 2000)
                          .last()
                          .map(|(i, _)| i)
                          .unwrap_or(2000);
                      format!("{}...", &formatted[..truncate_at])
                 } else {
                     formatted
                 };
                 content_elements.push(UiElement::Label(LabelConfig {
                     text: desc_display,
                     bold: false,
                     size: None,
                 }));
             }
             
             content_elements.push(UiElement::Separator);
             
             // ===== SAMPLE IMAGES (Gallery) =====
             if let Some(samples) = json["sample_images"].as_array() {
                 if !samples.is_empty() {
                     content_elements.push(UiElement::Label(LabelConfig {
                         text: "Sample Images".to_string(),
                         bold: true,
                         size: Some(14.0),
                     }));
                     content_elements.push(UiElement::Space(5.0));
                     
                     // Show up to 3 samples
                     for (i, sample) in samples.iter().take(3).enumerate() {
                         if let Some(url) = sample.as_str() {
                             content_elements.push(UiElement::Image(ImageConfig {
                                 cache_key: Some(metastore_providers::dlsite::cache_keys::screenshot_key(selected_id, i)),
                                 url: Some(url.to_string()),
                                 max_height: Some(200.0),
                             }));
                             content_elements.push(UiElement::Space(8.0));
                         }
                     }
                 }
             }
             
        } else {
             content_elements.push(UiElement::Loading(LoadingConfig {
                 message: Some("Loading details...".to_string()),
             }));
             // Logic in on_ui_event ensures this loads quickly, but if it takes a frame, show loading
        }
    } else {
        content_elements.push(UiElement::Space(50.0));
        content_elements.push(UiElement::Label(LabelConfig {
             text: "Select an item to view details".to_string(),
             bold: true,
             size: Some(18.0),
        }));
    }
    
    PluginLayout::Split(SplitConfig {
        sidebar: sidebar_elements,
        content: content_elements,
        sidebar_width: Some(300.0),
    })
            },
            
            _ => PluginLayout::Single(vec![]),
        }
    }

    fn get_top_tabs() -> Vec<archust_plugin_sdk::arclain::plugin::ui::TopTabConfig> {
        use archust_plugin_sdk::arclain::plugin::ui::{TopTabConfig, BadgeConfig};
        
        // Check if we have cached entries to show a badge
        let cache_count = STATE.with(|s| {
            let state = s.borrow();
            state.cached_entries.as_ref().map(|v| v.len() as u32)
        });
        
        vec![TopTabConfig {
            id: "dlsite_browser".to_string(),
            label: "DLSite".to_string(),
            icon: "MAGNIFYING_GLASS".to_string(),
            badge: cache_count.map(|count| BadgeConfig {
                count: if count > 0 { Some(count) } else { None },
                dot: count == 0,  // Show dot if no count but tab is active
                color: "blue".to_string(),
            }),
            priority: 100,  // After host tabs (0-99 reserved for host)
        }]
    }


    fn on_ui_event(id: String, value: Option<String>) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
        use archust_plugin_sdk::arclain::plugin::ui::PluginAction;
        // Handle system events dispatched as UI events
        if id == "event:archive_opened" {
            let path = value.unwrap_or_default();
            info(&format!("[DLSite Plugin] Archive opened event: {}", path));

            let auto_fetch = STATE.with(|s| s.borrow().auto_fetch_enabled);
            if auto_fetch {
                info("[DLSite Plugin] Auto-fetch enabled, scanning...");
                
                // Note: performing scan on background thread (host spawned thread for dispatch)
                match perform_scan() {
                    Ok(Some((id, json, scraped))) => {
                        info(&format!("[DLSite Plugin] Auto-fetched metadata for {}", id));
                        // Automatically emit to library so Organizer can pick it up via signals
                        let metadata_json = generate_metadata_json(&id, Some(&(json.clone(), scraped.clone())));
                        archust_plugin_sdk::emit_metadata(&metadata_json);
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

        // Image Gallery Navigation
        if id.starts_with("prev_image_") {
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                 let count = if let Some((_, json, _)) = &state.browser_detail_cache {
                     json["sample_images"].as_array().map(|a| a.len() as i32).unwrap_or(0)
                 } else { 0 };

                if count > 0 {
                    let mut new_idx = state.current_image_index - 1;
                    if new_idx < -1 { new_idx = count - 1; }
                    state.current_image_index = new_idx;
                    // Force refresh
                }
            });
            return vec![archust_plugin_sdk::arclain::plugin::ui::PluginAction::RefreshPanel("dlsite_browser".to_string())];
        }

        if id.starts_with("next_image_") {
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                 let count = if let Some((_, json, _)) = &state.browser_detail_cache {
                     json["sample_images"].as_array().map(|a| a.len() as i32).unwrap_or(0)
                 } else { 0 };

                if count > 0 {
                    let mut new_idx = state.current_image_index + 1;
                    if new_idx >= count { new_idx = -1; }
                    state.current_image_index = new_idx;
                }
            });
            return vec![archust_plugin_sdk::arclain::plugin::ui::PluginAction::RefreshPanel("dlsite_browser".to_string())];
        }

        if id.starts_with("reset_image_") {
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                state.current_image_index = -1;
            });
            return vec![archust_plugin_sdk::arclain::plugin::ui::PluginAction::RefreshPanel("dlsite_browser".to_string())];
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
            "refresh_cache" => {
                STATE.with(|state| {
                    state.borrow_mut().cached_entries = None; // clear to force refetch
                });
            }
            "back_to_cache_list" => {
                STATE.with(|state| {
                    state.borrow_mut().selected_cache_entry = None;
                });
            }
            id if id.starts_with("view_cache_entry_") => {
                 let entry_id = id.trim_start_matches("view_cache_entry_").to_string();
                 
                 // Read from local cache only - no network fetch for cached entries!
                 if let Some((json, scraped)) = get_cached_dlsite_metadata(&entry_id) {
                     STATE.with(|state| {
                         state.borrow_mut().browser_detail_cache = Some((entry_id.clone(), json, scraped));
                     });
                 }
                 
                 STATE.with(|state| {
                    state.borrow_mut().selected_cache_entry = Some(entry_id);
                });
                
                // Refresh panel to show the new selection and its details
                return vec![PluginAction::RefreshPanel("Dialog:dlsite_cache".to_string())];
            }
            id if id.starts_with("load_details_") => {
                // One-time fetch of details for the selected entry
                let entry_id = id.trim_start_matches("load_details_").to_string();
                info(&format!("[DLSite Plugin] Loading details for: {}", entry_id));
                
                if let Some((json, scraped)) = fetch_dlsite_metadata(&entry_id) {
                    STATE.with(|state| {
                        state.borrow_mut().browser_detail_cache = Some((entry_id.clone(), json, scraped));
                    });
                    info(&format!("[DLSite Plugin] Details loaded and cached for: {}", entry_id));
                } else {
                    info(&format!("[DLSite Plugin] Failed to load details for: {}", entry_id));
                }
            }
            id if id.starts_with("select_entry_") => {
                let entry_id = id.trim_start_matches("select_entry_").to_string();
                let meta_json = STATE.with(|s| {
                    let state = s.borrow();
                    if let Some((cached_id, json, _)) = &state.browser_detail_cache {
                        if cached_id == &entry_id {
                            return Some(serde_json::to_string(json).unwrap_or_default());
                        }
                    }
                     // Fallback to found_metadata
                     if let Some((scan_id, json, _)) = &state.found_metadata {
                         if scan_id == &entry_id {
                             return Some(serde_json::to_string(json).unwrap_or_default());
                         }
                     }
                    None
                });

                if let Some(json) = meta_json {
                     use archust_plugin_sdk::emit_metadata;
                     use archust_plugin_sdk::arclain::plugin::ui::{PluginAction, ToastConfig, ToastLevel};
                     
                     emit_metadata(&json);
                     
                     return vec![PluginAction::ShowToast(ToastConfig {
                         message: format!("Selected for Use: {}", entry_id),
                         level: ToastLevel::Success,
                     })];
                } else {
                     use archust_plugin_sdk::arclain::plugin::ui::{PluginAction, ToastConfig, ToastLevel};
                     return vec![PluginAction::ShowToast(ToastConfig {
                         message: "Could not find cached details".to_string(),
                         level: ToastLevel::Error,
                     })];
                }
            }

            // Browser UI handlers
            "browser_tabs" => {
                if let Some(tab) = value {
                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.browser_tab = if tab.contains("Search") { "search".to_string() } else { "cached".to_string() };
                        s.selected_cache_entry = None; // Clear selection on tab switch
                        s.search_results.clear(); // Clear search results on tab switch
                    });
                }
            }
            "browser_query" => {
                if let Some(query) = value {
                    STATE.with(|state| {
                        state.borrow_mut().search_query = query;
                    });
                }
            }
            "do_dlsite_search" => {
                let query = STATE.with(|s| s.borrow().search_query.clone());
                if !query.is_empty() {
                    STATE.with(|s| s.borrow_mut().browser_loading = true);
                    
                    // Perform search
                    let results = search_dlsite(&query);
                    
                    STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        state.search_results = results;
                        state.browser_loading = false;
                    });
                }
            }
            // List item selection (from ListContainer)
            id if id.starts_with("RJ") || id.starts_with("VJ") || id.starts_with("BJ") => {
                STATE.with(|state| {
                    state.borrow_mut().selected_cache_entry = Some(id.to_string());
                });
            }
            id if id.starts_with("apply_metadata_") => {
                let code = id.trim_start_matches("apply_metadata_").to_string();
                // Emit metadata for this code
                if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
                    STATE.with(|state| {
                        state.borrow_mut().found_metadata = Some((code.clone(), json.clone(), scraped.clone()));
                    });
                    
                    let metadata_json = generate_metadata_json(&code, Some(&(json, scraped)));
                    archust_plugin_sdk::emit_metadata(&metadata_json);
                    archust_plugin_sdk::show_message("Success", &format!("Applied metadata for {}", code));
                }
            }
            "close_browser" => {
                // Reset browser state
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.selected_cache_entry = None;
                    s.search_results.clear();
                });
            }
            // Refresh view - re-read from cache
            id if id.starts_with("refresh_view_") => {
                let entry_id = id.trim_start_matches("refresh_view_").to_string();
                info(&format!("[DLSite Plugin] Refresh view for: {}", entry_id));
                
                // Re-read from cache (no network)
                if let Some((json, scraped)) = get_cached_dlsite_metadata(&entry_id) {
                    STATE.with(|state| {
                        state.borrow_mut().browser_detail_cache = Some((entry_id.clone(), json, scraped));
                    });
                    info(&format!("[DLSite Plugin] Cache re-read for: {}", entry_id));
                }
                
                return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
            }
            // Refetch from network
            id if id.starts_with("refetch_entry_") => {
                let entry_id = id.trim_start_matches("refetch_entry_").to_string();
                info(&format!("[DLSite Plugin] Refetching from network: {}", entry_id));
                
                // Clear local state cache
                STATE.with(|state| {
                    state.borrow_mut().browser_detail_cache = None;
                });
                
                // Get cache keys for invalidation
                let json_key = metastore_providers::dlsite::cache_keys::json_key(&entry_id);
                let html_key = metastore_providers::dlsite::cache_keys::html_key(&entry_id);
                
                // Backup current cached data before invalidating
                // This way if refetch fails, we don't lose the existing entry
                let backup_data = get_cached_dlsite_metadata(&entry_id);
                
                // Invalidate cache to force network fetch
                archust_plugin_sdk::invalidate_cache(&json_key);
                archust_plugin_sdk::invalidate_cache(&html_key);
                info(&format!("[DLSite Plugin] Invalidated cache for: {}, {}", json_key, html_key));
                
                // Fetch from network (updates cache)
                match fetch_dlsite_metadata(&entry_id) {
                    Some((json, scraped)) => {
                        STATE.with(|state| {
                            state.borrow_mut().browser_detail_cache = Some((entry_id.clone(), json, scraped));
                        });
                        info(&format!("[DLSite Plugin] Refetched and cached: {}", entry_id));
                        archust_plugin_sdk::show_message("Success", &format!("Refetched {}", entry_id));
                    }
                    None => {
                        info(&format!("[DLSite Plugin] Refetch FAILED for: {}", entry_id));
                        
                        // Restore from backup if we had data before
                        if let Some((json, scraped)) = backup_data {
                            info(&format!("[DLSite Plugin] Restoring backup data for: {}", entry_id));
                            // Re-emit the old metadata to re-persist it
                            let metadata_json = generate_metadata_json(&entry_id, Some(&(json.clone(), scraped.clone())));
                            archust_plugin_sdk::emit_metadata(&metadata_json);
                            
                            STATE.with(|state| {
                                state.borrow_mut().browser_detail_cache = Some((entry_id.clone(), json, scraped));
                            });
                            archust_plugin_sdk::show_message("Warning", &format!("Refetch failed for {}. Restored previous data.", entry_id));
                        } else {
                            archust_plugin_sdk::show_message("Error", &format!("Failed to refetch {}. Entry may be deleted.", entry_id));
                        }
                    }
                }
                
                return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
            }
            // Select for use - emit metadata and set status message
            id if id.starts_with("select_entry_") => {
                let entry_id = id.trim_start_matches("select_entry_").to_string();
                info(&format!("[DLSite Plugin] Selected entry: {}", entry_id));
                
                // Get cached data and emit metadata
                if let Some((json, scraped)) = get_cached_dlsite_metadata(&entry_id) {
                    STATE.with(|state| {
                        state.borrow_mut().found_metadata = Some((entry_id.clone(), json.clone(), scraped.clone()));
                    });
                    
                    let metadata_json = generate_metadata_json(&entry_id, Some(&(json, scraped)));
                    archust_plugin_sdk::emit_metadata(&metadata_json);
                    
                    // Set status message via host
                    archust_plugin_sdk::arclain::plugin::host::set_status_message(&format!("Entry selected: {}", entry_id));
                    
                    archust_plugin_sdk::show_message("Selected", &format!("Entry {} selected for use", entry_id));
                } else {
                    archust_plugin_sdk::show_message("Error", &format!("Could not load data for {}", entry_id));
                }
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
    // Check cache first, then fall back to network if not cached
    let process_code = |code: String| -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
        info(&format!("[DLSite Plugin] Found code: {}", code));
        
        // Check cache first - avoids network if we already have metadata
        if let Some((json, scraped)) = get_cached_dlsite_metadata(&code) {
            info(&format!("[DLSite Plugin] Using cached metadata for {}", code));
            // Always emit to ensure MetadataStore (SQLite) is populated
            // This is safe because emit_metadata is idempotent
            let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
            archust_plugin_sdk::emit_metadata(&metadata_json);
            return Ok(Some((code, json, scraped)));
        }
        
        // Not in cache, fetch from network (Data API handles caching transparently)
        info(&format!("[DLSite Plugin] Cache miss, fetching {} from network", code));
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

/// Detect DLSite code using metastore provider
fn detect_dlsite_code(text: &str) -> Option<String> {
    metastore_providers::dlsite::detect_dlsite_code(text)
}

/// Read metadata from local cache ONLY - no network fetch!
/// Use this for viewing already-cached entries.
/// The host handles cache lookup transparently via get_data.
fn get_cached_dlsite_metadata(product_id: &str) -> Option<(serde_json::Value, Option<ScrapedData>)> {
    use archust_plugin_sdk::arclain::plugin::host::get_data;
    
    let json_key = metastore_providers::dlsite::cache_keys::json_key(product_id);
    let html_key = metastore_providers::dlsite::cache_keys::html_key(product_id);
    
    // get_data checks MetadataCache first (handled by host), no network
    let json_bytes = get_data(&json_key)?;
    let json_str = String::from_utf8(json_bytes).ok()?;
    let json_data = match serde_json::from_str::<serde_json::Value>(&json_str) {
        Ok(json) => {
            if let Some(arr) = json.as_array() {
                arr.first().cloned()?
            } else if json.is_object() {
                json
            } else {
                return None;
            }
        }
        Err(_) => return None,
    };
    
    // Read HTML from cache and scrape
    let scraped_data = if let Some(html_bytes) = get_data(&html_key) {
        if let Ok(html_str) = String::from_utf8(html_bytes) {
            scrape_html_metadata(&html_str)
        } else {
            None
        }
    } else {
        None
    };
    
    Some((json_data, scraped_data))
}

/// Fetch metadata from DLSite network (for new entries or search results)
fn fetch_dlsite_metadata(product_id: &str) -> Option<(serde_json::Value, Option<ScrapedData>)> {
    use archust_plugin_sdk::{fetch_string_blocking, log_network_activity};
    use metastore_providers::dlsite::api::{ajax_url, html_url, get_site_id};

    let site_id = get_site_id(product_id);
    
    // 1. Fetch JSON API using ajax endpoint (like dlsite-async library)
    let api_url = ajax_url(product_id);
    let cache_key = metastore_providers::dlsite::cache_keys::json_key(product_id);

    log_network_activity(&format!("Fetching metadata for {} from DLSite API (site: {})...", product_id, site_id));
    log_network_activity(&format!("GET {}", api_url));

    let json_data = match fetch_string_blocking(&cache_key, &api_url) {
        Ok(response_body) => {
            log_network_activity(&format!("Response: {} bytes", response_body.len()));
            match serde_json::from_str::<serde_json::Value>(&response_body) {
                Ok(json) => {
                    // Ajax endpoint returns {product_id: {...}} format
                    if let Some(obj) = json.as_object() {
                        obj.get(product_id).cloned()
                    } else if let Some(arr) = json.as_array() {
                        arr.first().cloned()
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

    // 2. Fetch HTML Page for scraping (using correct site_id)
    let html_page_url = html_url(product_id);
    let html_key = metastore_providers::dlsite::cache_keys::html_key(product_id);

    log_network_activity(&format!("Fetching HTML page for scraping..."));
    log_network_activity(&format!("GET {}", html_page_url));

    let scraped_data = match fetch_string_blocking(&html_key, &html_page_url) {
        Ok(html) => {
            log_network_activity(&format!("Response: {} bytes", html.len()));
            scrape_html_metadata(&html)
        },
        Err(e) => {
            log_network_activity(&format!("Failed to fetch HTML: {}", e));
            None
        }
    };

    // Check if image caching is enabled
    let cache_images = STATE.with(|state| state.borrow().cache_images);

    // Check if content is geo-blocked (don't cache images for incomplete/geo-blocked content)
    let is_geo_blocked = scraped_data.as_ref().map(|d| d.geo_blocked).unwrap_or(false);
    
    // If image caching is enabled, we have scraped data, AND content is NOT geo-blocked, fetch images
    if cache_images && !is_geo_blocked {
        if let Some(data) = &scraped_data {
            use archust_plugin_sdk::ResourceType;

            // Fetch cover image
            if let Some(cover_url) = &data.cover_image {
                let cover_key = metastore_providers::dlsite::cache_keys::cover_key(product_id);
                log_network_activity(&format!("Fetching cover image: {}", cover_url));
                
                if let Err(e) = archust_plugin_sdk::fetch_blocking(&cover_key, cover_url, ResourceType::Image) {
                    log_network_activity(&format!("Failed to fetch/cache cover image: {}", e));
                }
            }

            // Fetch screenshots (limit to first 5 to be reasonable)
            for (idx, screenshot_url) in data.screenshots.iter().take(5).enumerate() {
                let screenshot_key = format!("dlsite:screenshot:{}:{}", product_id, idx);
                log_network_activity(&format!("Fetching screenshot {}: {}", idx, screenshot_url));
                
                if let Err(e) = archust_plugin_sdk::fetch_blocking(&screenshot_key, screenshot_url, ResourceType::Image) {
                    log_network_activity(&format!("Failed to fetch/cache screenshot {}: {}", idx, e));
                }
            }
        }
    } else if is_geo_blocked {
        log_network_activity(&format!("[DLSite Plugin] Skipping image cache for geo-blocked content"));
    }

    Some((json_data, scraped_data))
}

/// Scraped data from HTML - wraps metastore provider's type
#[derive(Debug, Clone)]
struct ScrapedData {
    title: Option<String>,
    circle: Option<String>,
    release_date: Option<String>,
    update_date: Option<String>,
    tags: Vec<String>,
    description: Option<String>,
    cover_image: Option<String>,
    screenshots: Vec<String>,
    voice_actors: Vec<String>,
    authors: Vec<String>,
    illustrators: Vec<String>,
    scenarios: Vec<String>,
    musicians: Vec<String>,
    writers: Vec<String>,
    brand: Option<String>,
    publisher: Option<String>,
    series: Option<String>,
    page_count: Option<i64>,
    file_size: Option<String>,
    genres: Vec<String>,
    geo_blocked: bool,
}

/// Scrape HTML using metastore provider
fn scrape_html_metadata(html: &str) -> Option<ScrapedData> {
    use metastore_providers::dlsite::parse_html_response;
    use archust_plugin_sdk::info;
    
    let scraped = parse_html_response(html)?;
    
    // Log geo_blocked status for debugging
    // Log geo_blocked status for debugging
    info(&format!(
        "[DLSite Plugin] Scraped geo_blocked={}, cover={}, genres={}, circle={}",
        scraped.geo_blocked,
        scraped.cover_image.is_some(),
        scraped.genres.len(),
        scraped.circle.is_some()
    ));

    if scraped.geo_blocked {
        let snippet: String = html.chars().take(10000).collect();
        info(&format!("[DLSite Plugin] BLOCKED CONTENT (10k chars): {}", snippet));
        
        // Manual content check for debugging
        let has_outline = html.contains("work_outline");
        let has_name = html.contains("work_name");
        
        info(&format!("[DLSite Plugin] Content Check: has_outline={}, has_name={}", has_outline, has_name));

        // Replicate pattern match logic to find the culprit
        let html_lower = html.to_lowercase();
        let patterns = [
            "お住いの国・地域からは本作品は購入できません",
            "this product cannot be purchased",
            "このページはお住まいの地域からは表示できません",
            "this page cannot be displayed",
            "access denied",
            "region restricted",
            "not available in your country",
            "geographic restrictions",
        ];
        
        for pattern in patterns {
            let pattern_lower = pattern.to_lowercase();
            if let Some((prefix, suffix)) = html_lower.split_once(&pattern_lower) {
                info(&format!("[DLSite Plugin] DETECTED BLOCK PATTERN: '{}'", pattern));
                // Safely extract context using iterators to handle UTF-8 chars
                let pre_snip: String = prefix.chars().rev().take(200).collect::<String>().chars().rev().collect();
                let post_snip: String = suffix.chars().take(200).collect();
                info(&format!("[DLSite Plugin] PATTERN CONTEXT: ...{} >> {} << {}...", pre_snip, pattern, post_snip));
            }
        }
        
        // Log Title
        if let Some(start) = html_lower.find("<title>") {
            if let Some(end) = html_lower[start..].find("</title>") {
                 let title = &html[start+7..start+end];
                 info(&format!("[DLSite Plugin] Page Title: {}", title));
            }
        }
    }
    
    // Convert metastore's ScrapedData to our local type
    Some(ScrapedData {
        title: scraped.title,
        circle: scraped.circle,
        release_date: scraped.release_date,
        update_date: scraped.update_date,
        tags: scraped.tags,
        description: scraped.description,
        cover_image: scraped.cover_image,
        screenshots: scraped.screenshots,
        voice_actors: scraped.voice_actors,
        authors: scraped.authors,
        illustrators: scraped.illustrators,
        scenarios: scraped.scenarios,
        musicians: scraped.musicians,
        writers: scraped.writers,
        brand: scraped.brand,
        publisher: scraped.publisher,
        series: scraped.series,
        page_count: scraped.page_count,
        file_size: scraped.file_size,
        genres: scraped.genres,
        geo_blocked: scraped.geo_blocked,
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
        // Strip time from date (e.g. "2026-03-06 00:00:00" -> "2026-03-06")
        let date_raw = data["regist_date"].as_str().unwrap_or("");
        
        // DEBUG: Trace date cleaning
        archust_plugin_sdk::info(&format!("[DateDebug] Raw: '{}', Len: {}", date_raw, date_raw.len()));

        let date_clean = if date_raw.is_empty() {
             None 
        } else {
             let clean = date_raw.split_whitespace().next().unwrap_or(date_raw).to_string();
             archust_plugin_sdk::info(&format!("[DateDebug] Clean: '{}'", clean));
             Some(clean)
        };

        (
            data["work_name"].as_str().map(|s| s.to_string()),
            data["maker_name"].as_str().map(|s| s.to_string()),
            data["intro_s"].as_str().unwrap_or(""),
            data["price"].as_u64().unwrap_or(0),
            date_clean,
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
            None,
            None,
            "",
            0,
            None,
            Vec::new(),
        )
    };

    // Override with scraped data if available
    if let Some(scraped) = scraped_data {
        if let Some(t) = &scraped.title {
            title = Some(t.clone());
        }
        if let Some(c) = &scraped.circle {
            circle = Some(c.clone());
        }
        if let Some(d) = &scraped.release_date {
            // Also strip time from scraped date
            release_date = Some(d.split_whitespace().next().unwrap_or(d).to_string());
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

    // Debug: log what we're generating
    if let Some(scraped) = scraped_data {
        info(&format!(
            "[DLSite Plugin] Scraped: screenshots={}, voice_actors={}, genres={}, cover={}",
            scraped.screenshots.len(),
            scraped.voice_actors.len(),
            scraped.genres.len(),
            scraped.cover_image.is_some()
        ));
    } else {
        info("[DLSite Plugin] No scraped data available");
    }

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
        "voice_actors": scraped_data.map(|s| s.voice_actors.clone()).unwrap_or_default(),
        "authors": scraped_data.map(|s| s.authors.clone()).unwrap_or_default(),
        "illustrators": scraped_data.map(|s| s.illustrators.clone()).unwrap_or_default(),
        "scenarios": scraped_data.map(|s| s.scenarios.clone()).unwrap_or_default(),
        "musicians": scraped_data.map(|s| s.musicians.clone()).unwrap_or_default(),
        "writers": scraped_data.map(|s| s.writers.clone()).unwrap_or_default(),
        "brand": scraped_data.and_then(|s| s.brand.clone()),
        "publisher": scraped_data.and_then(|s| s.publisher.clone()),
        "series": scraped_data.and_then(|s| s.series.clone()),
        "page_count": scraped_data.and_then(|s| s.page_count),
        "file_size": scraped_data.and_then(|s| s.file_size.clone()),
        "update_date": scraped_data.and_then(|s| s.update_date.clone()),
        "genres": scraped_data.map(|s| s.genres.clone()).unwrap_or_default(),
        "geo_blocked": scraped_data.map(|s| s.geo_blocked).unwrap_or(false),
        "dlsite": {
            "id": product_id,
            "code": product_id, // Required by RuleEngine for $code
            "price": price.to_string(),
            "url": format!("https://www.dlsite.com/pro/work/=/product_id/{}.html", product_id)
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
    use metastore_providers::dlsite::parse_search_response;

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

    // Use metastore provider for parsing
    let results = parse_search_response(&html);
    
    log_network_activity(&format!("Found {} results", results.len()));
    
    // Convert SearchResult to (code, title, maker) tuple
    results
        .into_iter()
        .take(10)
        .map(|r| (r.external_id, r.title, r.creator.unwrap_or_else(|| "Unknown".to_string())))
        .collect()
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
        // Case 1: Minimal input (missing data)
        let json = generate_metadata_json("RJ123456", None);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["dlsite"]["id"], "RJ123456");
        assert_eq!(parsed["common"]["dlsite_id"], "RJ123456");
        // Should now be null, not "Unknown Title"
        assert_eq!(parsed["title"], serde_json::Value::Null);
        assert_eq!(parsed["circle"], serde_json::Value::Null);
        assert_eq!(parsed["release_date"], serde_json::Value::Null);

        // Case 2: Date with time
        let json_time = serde_json::json!({
            "work_name": "Test Title",
            "maker_name": "Test Circle",
            "regist_date": "2026-03-06 00:00:00"
        });
        let data_time = (json_time, None);
        let output_time = generate_metadata_json("RJ123456", Some(&data_time));
        let parsed_time: serde_json::Value = serde_json::from_str(&output_time).unwrap();
        
        assert_eq!(parsed_time["release_date"], "2026-03-06");
    }
}
