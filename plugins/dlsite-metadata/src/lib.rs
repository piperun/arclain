use archust_plugin_sdk::info;
use std::cell::RefCell;

// Search result with basic info for display
#[derive(Clone)]
struct SearchResultInfo {
    code: String,
    title: String,
    circle: String,
    author: String, // New field
    release_date: Option<String>,
    age_rating: Option<String>,
    price: Option<String>,
}

// Full work details for selected item
#[derive(Clone)]
struct WorkDetails {
    code: String,
    data: serde_json::Value,
    scraped: Option<ScrapedData>,
}

#[derive(Clone, PartialEq, Debug)]
enum AsyncFetchType {
    Search,
    FetchMetadata,
}

// Plugin state to store found metadata
struct PluginState {
    found_metadata: Option<(String, serde_json::Value, Option<ScrapedData>)>, // (product_id, json, scraped)
    last_status: String,
    
    // Async State
    async_fetch_id: Option<String>,
    async_fetch_type: Option<AsyncFetchType>,
    async_fetch_context: Option<String>,
    
    search_mode: bool,
    search_query: String,
    search_results: Vec<SearchResultInfo>,
    auto_load_enabled: bool, // Cache for the auto_load_cache setting
    
    // Manual search dialog state
    dialog_query: String,
    dialog_results: Vec<SearchResultInfo>,
    dialog_selected_index: Option<usize>,
    dialog_selected_details: Option<WorkDetails>,
    dialog_selected_image_index: usize,
    dialog_fetching: bool,
    dialog_status: String,
}

// Global state (thread-local for WASM component)
thread_local! {
    static STATE: RefCell<PluginState> = RefCell::new(PluginState {
        found_metadata: None,
        last_status: "Ready to scan".to_string(),
        
        async_fetch_id: None,
        async_fetch_type: None,
        async_fetch_context: None,
        
        search_mode: false,
        search_query: String::new(),
        search_results: Vec::new(),
        auto_load_enabled: true, // Default to enabled
        
        // Dialog state defaults
        dialog_query: String::new(),
        dialog_results: Vec::new(),
        dialog_selected_index: None,
        dialog_selected_details: None,
        dialog_selected_image_index: 0,
        dialog_fetching: false,
        dialog_status: String::new(),
    });
}


// Check for async fetch results and update state
fn check_async_results() {
    let (id_opt, type_opt, ctx_opt) = STATE.with(|state| {
        let s = state.borrow();
        (s.async_fetch_id.clone(), s.async_fetch_type.clone(), s.async_fetch_context.clone())
    });

    if let Some(id) = id_opt {
        if let Some(result) = archust_plugin_sdk::poll_async_fetch(&id) {
            // Result is ready
            match result {
                Ok(body) => {
                    // Success
                    if let Some(fetch_type) = type_opt {
                         match fetch_type {
                             AsyncFetchType::Search => {
                                 let results = parse_search_results(&body);
                                 STATE.with(|state| {
                                     let mut s = state.borrow_mut();
                                     s.dialog_fetching = false;
                                     if results.is_empty() {
                                         s.dialog_status = "No results found".to_string();
                                     } else {
                                         s.dialog_status = format!("Found {} results", results.len());
                                     }
                                     s.dialog_results = results;
                                     s.dialog_selected_index = None;
                                     s.async_fetch_id = None;
                                     s.async_fetch_type = None;
                                     s.async_fetch_context = None;
                                 });
                             },
                             AsyncFetchType::FetchMetadata => {
                                 // Handle metadata result parsing
                                 if let Some(scraped) = scrape_html_metadata(&body) {
                                     if let Some(code) = ctx_opt {
                                         // Create JSON from scraped data for fallback
                                         let json = serde_json::json!({
                                             "work_name": scraped.title.clone().unwrap_or("Unknown".to_string()),
                                             "maker_name": scraped.circle.clone().unwrap_or("Unknown".to_string()),
                                             "regist_date": scraped.release_date.clone().unwrap_or_default(),
                                             "dlsite": {
                                                 "price": 0 // Unknown from simple scrape? Table might have it.
                                             }
                                         });
                                         
                                         // Generate full metadata JSON
                                         let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), Some(scraped.clone()))));
                                         
                                         // Save and Emit
                                         archust_plugin_sdk::save_cached_metadata(&code, &metadata_json);
                                         archust_plugin_sdk::emit_metadata(&metadata_json);
                                         
                                         STATE.with(|state| {
                                             let mut s = state.borrow_mut();
                                             s.found_metadata = Some((code.clone(), json, Some(scraped)));
                                             s.last_status = format!("Metadata found for {}", code);
                                             
                                             s.async_fetch_id = None;
                                             s.async_fetch_type = None;
                                             s.async_fetch_context = None;
                                         });
                                         
                                         archust_plugin_sdk::show_message("Success", "Metadata found and loaded!");
                                     }
                                 } else {
                                     STATE.with(|state| {
                                        let mut s = state.borrow_mut();
                                        s.last_status = "Failed to parse metadata".to_string();
                                        s.async_fetch_id = None;
                                        s.async_fetch_type = None;
                                        s.async_fetch_context = None;
                                     });
                                     archust_plugin_sdk::show_message("Error", "Failed to parse metadata from page.");
                                 }
                             }
                         }
                    }
                }
                Err(e) => {
                     // Error
                     STATE.with(|state| {
                         let mut s = state.borrow_mut();
                         if s.async_fetch_type == Some(AsyncFetchType::Search) {
                             s.dialog_fetching = false;
                             s.dialog_status = format!("Error: {}", e);
                         } else {
                             s.last_status = format!("Fetch Error: {}", e);
                             archust_plugin_sdk::show_message("Error", &format!("Fetch failed: {}", e));
                         }
                         s.async_fetch_id = None;
                         s.async_fetch_type = None;
                         s.async_fetch_context = None;
                     });
                }
            }
        }
    }
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
        // Poll for async results before rendering
        check_async_results();
        
        use archust_plugin_sdk::arclain::plugin::ui::*;

        match extension_point.as_str() {
            "MainPage" => vec![
                UiElement::TextInput(TextInputConfig {
                    id: "request_timeout".to_string(),
                    label: "API Request Timeout (seconds)".to_string(),
                    value: "30".to_string(),
                }),
                UiElement::TextInput(TextInputConfig {
                    id: "code_regex".to_string(),
                    label: "DLSite Code Regex Pattern".to_string(),
                    value: "(RJ|VJ|BJ)\\d{6,8}".to_string(),
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
            "PluginButton" => {
                STATE.with(|s| {
                    let state = s.borrow();
                    let mut buttons = vec![];
                    
                    // Show Info button - only if metadata is found
                    if state.found_metadata.is_some() {
                        buttons.push(UiElement::Button(ButtonConfig {
                            id: "toolbar_show_info".to_string(),
                            label: "Info".to_string(),
                            action: Some(ButtonAction::ShowDialog("dlsite_details".to_string())),
                        }));
                    } else {
                        // Fetch button
                        buttons.push(UiElement::Button(ButtonConfig {
                            id: "fetch_metadata".to_string(),
                            label: "Fetch".to_string(),
                            action: Some(ButtonAction::Custom("perform_fetch".to_string())),
                        }));
                        
                        // Manual Search button when no code found
                        buttons.push(UiElement::Button(ButtonConfig {
                            id: "toolbar_manual_search".to_string(),
                            label: "Search".to_string(),
                            action: Some(ButtonAction::ShowDialog("manual_search".to_string())),
                        }));
                    }
                    
                    // DLSite Page button - browse cached entries
                    buttons.push(UiElement::Button(ButtonConfig {
                        id: "toolbar_dlsite_page".to_string(),
                        label: "Cache".to_string(),
                        action: Some(ButtonAction::OpenPage("dlsite_cache".to_string())),
                    }));
                    
                    buttons
                })
            }
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
                            action: None,
                        }));
                        
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "cancel_search".to_string(),
                            label: "Cancel".to_string(),
                            action: None,
                        }));
                        
                        if !state.search_results.is_empty() {
                            elements.push(UiElement::Separator);
                            elements.push(UiElement::Label(LabelConfig {
                                text: "Results:".to_string(),
                                bold: true,
                                size: None,
                            }));
                            
                            for result in &state.search_results {
                                let mut label = format!("[{}] {}", result.code, result.title);
                                
                                // Maker/Author
                                let mut makers = Vec::new();
                                if !result.circle.is_empty() { makers.push(result.circle.clone()); }
                                if !result.author.is_empty() && result.author != result.circle { makers.push(result.author.clone()); }
                                if !makers.is_empty() {
                                    label.push_str(&format!(" ({})", makers.join(" / ")));
                                }
                                
                                // Extra info
                                let mut extras = Vec::new();
                                if let Some(price) = &result.price { extras.push(price.clone()); }
                                if let Some(date) = &result.release_date { extras.push(date.clone()); }
                                if let Some(rating) = &result.age_rating { extras.push(rating.clone()); }
                                
                                if !extras.is_empty() {
                                    label.push_str(&format!(" - {}", extras.join(", ")));
                                }

                                elements.push(UiElement::Button(ButtonConfig {
                                    id: format!("select_result_{}", result.code),
                                    label,
                                    action: None,
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
                                action: None,
                            }));
                        } else {
                            elements.push(UiElement::Button(ButtonConfig {
                                id: "fetch_metadata".to_string(),
                                label: "Fetch Metadata".to_string(),
                                action: None,
                            }));
                            
                            elements.push(UiElement::Button(ButtonConfig {
                                id: "toggle_search".to_string(),
                                label: "Search Manually".to_string(),
                                action: None,
                            }));
                        }
                    }
                });

                elements
            }

            "Panel" => {
                STATE.with(|s| {
                    let state = s.borrow();
                    
                    if let Some((code, data, scraped)) = &state.found_metadata {
                        // Extract metadata fields
                        let title = data.get("work_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown Title");
                        
                        let author = scraped.as_ref()
                            .and_then(|s| s.circle.as_deref())
                            .or_else(|| data.get("maker_name").and_then(|v| v.as_str()))
                            .unwrap_or("Unknown Author");
                        
                        // Truncate for compact display
                        let title_display = if title.len() > 35 {
                            format!("{}...", &title[..32])
                        } else {
                            title.to_string()
                        };
                        
                        let author_display = if author.len() > 30 {
                            format!("{}...", &author[..27])
                        } else {
                            author.to_string()
                        };
                        
                        vec![
                            // Small thumbnail image (first cached screenshot)
                            UiElement::Image(ImageConfig {
                                cache_key: Some(format!("dlsite:{}:screenshot_0", code)),
                                url: None,
                                max_height: Some(60.0),
                            }),
                            // DLSite code
                            UiElement::Label(LabelConfig {
                                text: code.clone(),
                                bold: true,
                                size: Some(12.0),
                            }),
                            // Title (truncated)
                            UiElement::Label(LabelConfig {
                                text: title_display,
                                bold: false,
                                size: None,
                            }),
                            // Author (truncated)
                            UiElement::Label(LabelConfig {
                                text: author_display,
                                bold: false,
                                size: Some(11.0),
                            }),
                            // Show Details button
                            UiElement::Button(ButtonConfig {
                                id: "show_dlsite_details".to_string(),
                                label: "Show Details".to_string(),
                                action: Some(ButtonAction::ShowDialog("dlsite_details".to_string())),
                            }),
                        ]
                    } else {
                        // No metadata found - show fetch and search options
                        vec![
                            UiElement::Label(LabelConfig {
                                text: "No DLSite code detected".to_string(),
                                bold: false,
                                size: Some(11.0),
                            }),
                            UiElement::Button(ButtonConfig {
                                id: "fetch_metadata".to_string(),
                                label: "Fetch Metadata".to_string(),
                                action: None,
                            }),
                            UiElement::Button(ButtonConfig {
                                id: "open_manual_search".to_string(),
                                label: "Manual Search".to_string(),
                                action: Some(ButtonAction::ShowDialog("manual_search".to_string())),
                            }),
                        ]
                    }
                })
            }
            "Dialog:dlsite_details" => {
                STATE.with(|s| {
                    let state = s.borrow();
                    
                    if let Some((code, data, scraped)) = &state.found_metadata {
                        let mut elements = vec![];
                        
                        // Main image (first screenshot)
                        elements.push(UiElement::Image(ImageConfig {
                            cache_key: Some(format!("dlsite:{}:screenshot_0", code)),
                            url: None,
                            max_height: Some(250.0),
                        }));
                        
                        elements.push(UiElement::Separator);
                        
                        // Title
                        let title = data.get("work_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown Title");
                        elements.push(UiElement::Label(LabelConfig {
                            text: title.to_string(),
                            bold: true,
                            size: Some(20.0),
                        }));
                        
                        // DLSite Code
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Code: {}", code),
                            bold: false,
                            size: Some(12.0),
                        }));
                        
                        // Author/Circle
                        let author = scraped.as_ref()
                            .and_then(|s| s.circle.as_deref())
                            .or_else(|| data.get("maker_name").and_then(|v| v.as_str()))
                            .unwrap_or("Unknown Author");
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Author: {}", author),
                            bold: false,
                            size: None,
                        }));
                        
                        // Release date
                        if let Some(sale_date) = data.get("regist_date").and_then(|v| v.as_str()) {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Released: {}", sale_date),
                                bold: false,
                                size: None,
                            }));
                        }

                        // Extended Metadata (Series, Illustrator, Voice, Rating)
                        if let Some(scraped) = scraped {
                            // Series
                            if let Some(series) = &scraped.series {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Series: {}", series),
                                    bold: false,
                                    size: None,
                                }));
                            }

                            // Illustrator
                            if let Some(illustrator) = &scraped.illustrator {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Illustrator: {}", illustrator),
                                    bold: false,
                                    size: None,
                                }));
                            }

                            // Voice Actors
                            if !scraped.voice_actors.is_empty() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Voice: {}", scraped.voice_actors.join(", ")),
                                    bold: false,
                                    size: None,
                                }));
                            }

                            // Rating
                            if let Some(rating) = scraped.rating {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Rating: {:.1} / 5.0", rating),
                                    bold: true,
                                    size: None,
                                }));
                            }
                        }
                        
                        elements.push(UiElement::Separator);
                        
                        // Description
                        if let Some(scraped) = scraped {
                            if let Some(desc) = &scraped.description {
                                // Truncate long descriptions
                                let desc_display = if desc.len() > 500 {
                                    format!("{}...", &desc[..497])
                                } else {
                                    desc.clone()
                                };
                                elements.push(UiElement::Label(LabelConfig {
                                    text: desc_display,
                                    bold: false,
                                    size: Some(12.0),
                                }));
                            }
                        }
                        
                        // Tags as comma-separated string
                        if let Some(tags) = data.get("genre").and_then(|v| v.as_array()) {
                            let tag_names: Vec<String> = tags.iter()
                                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                                .collect();
                            if !tag_names.is_empty() {
                                elements.push(UiElement::Separator);
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Tags: {}", tag_names.join(", ")),
                                    bold: false,
                                    size: Some(11.0),
                                }));
                            }
                        }
                        
                        elements.push(UiElement::Space(16.0));
                        
                        // Close button
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "close_dlsite_dialog".to_string(),
                            label: "Close".to_string(),
                            action: Some(ButtonAction::CloseDialog),
                        }));
                        
                        elements
                    } else {
                        vec![
                            UiElement::Label(LabelConfig {
                                text: "No metadata available".to_string(),
                                bold: false,
                                size: None,
                            }),
                            UiElement::Button(ButtonConfig {
                                id: "close_dlsite_dialog".to_string(),
                                label: "Close".to_string(),
                                action: Some(ButtonAction::CloseDialog),
                            }),
                        ]
                    }
                })
            }
            "Dialog:manual_search" => {
                STATE.with(|s| {
                    let state = s.borrow();
                    let mut elements = vec![];
                    
                    // 1. Search Bar (Fixed Top)
                    elements.push(UiElement::Label(LabelConfig {
                        text: "Manual Search".to_string(),
                        bold: true,
                        size: Some(18.0),
                    }));
                    
                    elements.push(UiElement::Space(8.0));
                    
                    elements.push(UiElement::TextInput(TextInputConfig {
                        id: "dialog_search_input".to_string(),
                        label: "Search Query (Code or Title)".to_string(),
                        value: state.dialog_query.clone(),
                    }));
                    
                    elements.push(UiElement::Space(4.0));
                    
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "dialog_perform_search".to_string(),
                        label: if state.dialog_fetching { "Searching...".to_string() } else { "Search".to_string() },
                        action: None,
                    }));

                    // Status
                    // Status
                    if !state.dialog_status.is_empty() {
                         elements.push(UiElement::Space(4.0));
                         elements.push(UiElement::Label(LabelConfig {
                            text: state.dialog_status.clone(),
                            bold: false,
                            size: Some(11.0),
                        }));
                    }
                    
                    elements.push(UiElement::Separator);

                    // 2. Results List
                    if !state.dialog_results.is_empty() {
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Results ({}):", state.dialog_results.len()),
                            bold: true,
                            size: Some(12.0),
                        }));
                        
                        for (idx, result) in state.dialog_results.iter().enumerate() {
                            let mut label_parts = Vec::new();
                            label_parts.push(result.code.clone());
                            if !result.title.is_empty() {
                                label_parts.push(result.title.clone());
                            }
                            // Show Circle/Author
                            let maker = if !result.author.is_empty() && result.author != result.circle {
                                format!("{} / {}", result.circle, result.author)
                            } else {
                                result.circle.clone()
                            };
                            if !maker.is_empty() && maker != "Unknown" {
                                label_parts.push(format!("[{}]", maker));
                            }
                            
                            // Show Price/Date
                            if let Some(price) = &result.price {
                                label_parts.push(format!("({})", price));
                            }
                            
                            let label = label_parts.join(" ");
                            
                            let is_selected = state.dialog_selected_index.map_or(false, |i| i == idx);
                            let display_label = if is_selected { format!("> {}", label) } else { label };

                            elements.push(UiElement::Button(ButtonConfig {
                                id: format!("dialog_select_{}", idx),
                                label: display_label,
                                action: None,
                            }));
                        }
                    }
                    
                    elements.push(UiElement::Separator);
                    
                    // Details pane
                    if let Some(details) = &state.dialog_selected_details {
                        // Main image
                        let img_idx = state.dialog_selected_image_index;
                        elements.push(UiElement::Image(ImageConfig {
                            cache_key: Some(format!("dlsite:{}:screenshot_{}", details.code, img_idx)),
                            url: None,
                            max_height: Some(200.0),
                        }));
                        
                        // Image selector (thumbnails as buttons)
                        if let Some(scraped) = &details.scraped {
                            if scraped.screenshots.len() > 1 {
                                for (i, _) in scraped.screenshots.iter().take(6).enumerate() {
                                    elements.push(UiElement::Button(ButtonConfig {
                                        id: format!("dialog_img_{}", i),
                                        label: if i == img_idx { format!("[{}]", i + 1) } else { format!("{}", i + 1) },
                                        action: None,
                                    }));
                                }
                            }
                        }
                        
                        elements.push(UiElement::Separator);
                        
                        // Title
                        let title = details.data.get("work_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown");
                        elements.push(UiElement::Label(LabelConfig {
                            text: title.to_string(),
                            bold: true,
                            size: Some(16.0),
                        }));
                        
                        // Code
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Code: {}", details.code),
                            bold: false,
                            size: Some(11.0),
                        }));
                        
                        // Author/Circle
                        let author = details.scraped.as_ref()
                            .and_then(|s| s.circle.as_deref())
                            .or_else(|| details.data.get("maker_name").and_then(|v| v.as_str()))
                            .unwrap_or("Unknown");
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Circle: {}", author),
                            bold: false,
                            size: None,
                        }));
                        
                        // Release date
                        if let Some(date) = details.data.get("regist_date").and_then(|v| v.as_str()) {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Released: {}", date),
                                bold: false,
                                size: None,
                            }));
                        }
                        
                        // Description (truncated)
                        if let Some(scraped) = &details.scraped {
                            if let Some(desc) = &scraped.description {
                                let desc_short = if desc.len() > 300 {
                                    format!("{}...", &desc[..297])
                                } else {
                                    desc.clone()
                                };
                                elements.push(UiElement::Label(LabelConfig {
                                    text: desc_short,
                                    bold: false,
                                    size: Some(11.0),
                                }));
                            }
                        }
                        
                        // Tags
                        if let Some(scraped) = &details.scraped {
                            if !scraped.tags.is_empty() {
                                elements.push(UiElement::Label(LabelConfig {
                                    text: format!("Tags: {}", scraped.tags.join(", ")),
                                    bold: false,
                                    size: Some(10.0),
                                }));
                            }
                        }
                        
                        elements.push(UiElement::Space(8.0));
                        
                        // Apply button
                        elements.push(UiElement::Button(ButtonConfig {
                            id: "dialog_apply_result".to_string(),
                            label: "Apply to Archive".to_string(),
                            action: None,
                        }));
                    } else {
                        elements.push(UiElement::Label(LabelConfig {
                            text: "Select a result to view details".to_string(),
                            bold: false,
                            size: None,
                        }));
                    }
                    
                    elements.push(UiElement::Space(16.0));
                    
                    // Bottom buttons
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "dialog_clear".to_string(),
                        label: "Clear".to_string(),
                        action: None,
                    }));
                    
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "dialog_close".to_string(),
                        label: "Close".to_string(),
                        action: Some(ButtonAction::CloseDialog),
                    }));
                    
                    elements
                })
            }
            "Page:dlsite_cache" => {
                // Cache browser page
                let mut elements = vec![
                    UiElement::Label(LabelConfig {
                        text: "DLSite Cache Browser".to_string(),
                        bold: true,
                        size: Some(18.0),
                    }),
                    UiElement::Separator,
                    UiElement::Label(LabelConfig {
                        text: "Browse and manage cached DLSite metadata entries.".to_string(),
                        bold: false,
                        size: Some(12.0),
                    }),
                    UiElement::Space(8.0),
                ];
                
                // List cached entries
                let entries = archust_plugin_sdk::list_cached_entries();
                
                if entries.is_empty() {
                    elements.push(UiElement::Label(LabelConfig {
                        text: "No cached entries yet.".to_string(),
                        bold: false,
                        size: Some(11.0),
                    }));
                } else {
                    elements.push(UiElement::Label(LabelConfig {
                        text: format!("{} cached entries:", entries.len()),
                        bold: true,
                        size: Some(12.0),
                    }));
                    
                    for (idx, code) in entries.iter().enumerate() {
                        elements.push(UiElement::Button(ButtonConfig {
                            id: format!("cache_entry_{}", idx),
                            label: code.clone(),
                            action: None, // Will load details on click
                        }));
                    }
                }
                
                elements.push(UiElement::Space(16.0));
                elements.push(UiElement::Button(ButtonConfig {
                    id: "cache_export".to_string(),
                    label: "Export Cache".to_string(),
                    action: None,
                }));
                elements.push(UiElement::Button(ButtonConfig {
                    id: "cache_import".to_string(),
                    label: "Import Cache".to_string(),
                    action: None,
                }));
                elements.push(UiElement::Space(16.0));
                elements.push(UiElement::Button(ButtonConfig {
                    id: "close_cache_page".to_string(),
                    label: "Close".to_string(),
                    action: Some(ButtonAction::ClosePage),
                }));
                
                elements
            }
            _ => vec![],
        }
    }

    fn on_ui_event(id: String, value: Option<String>) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
        use archust_plugin_sdk::arclain::plugin::ui::{PluginAction, ToastConfig, ToastLevel};
        
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
            return vec![];
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
                vec![]
            }
            "enable_cache" => {
                if let Some(val) = value {
                    archust_plugin_sdk::arclain::plugin::host::set_setting("enable_cache", &val);
                    info(&format!("[DLSite Plugin] Cache enabled setting changed to: {}", val));
                }
                vec![]
            }
            "cache_export" => {
                match archust_plugin_sdk::export_cache() {
                    Ok(msg) => vec![PluginAction::ShowToast(ToastConfig {
                        message: msg,
                        level: ToastLevel::Success,
                    })],
                    Err(e) => vec![PluginAction::ShowToast(ToastConfig {
                        message: format!("Export failed: {}", e),
                        level: ToastLevel::Error,
                    })],
                }
            }
            "cache_import" => {
                match archust_plugin_sdk::import_cache() {
                    Ok(msg) => {
                         // Refresh the cache page if possible, or atleast show success
                         vec![PluginAction::ShowToast(ToastConfig {
                            message: msg,
                            level: ToastLevel::Success,
                        }), PluginAction::RefreshPanel("dlsite_cache".to_string())]
                    },
                    Err(e) => vec![PluginAction::ShowToast(ToastConfig {
                        message: format!("Import failed: {}", e),
                        level: ToastLevel::Error,
                    })],
                }
            }
            "toggle_search" => {
                STATE.with(|state| {
                    state.borrow_mut().search_mode = true;
                });
                vec![]
            }
            "cancel_search" => {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.search_mode = false;
                    s.search_results.clear();
                });
                vec![]
            }
            "search_query" => {
                if let Some(query) = value {
                    STATE.with(|state| {
                        state.borrow_mut().search_query = query;
                    });
                }
                vec![]
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
                vec![]
            }
            "perform_fetch" => {
                use archust_plugin_sdk::{current_archive_info, list_archive_files, get_cached_metadata};

                // Helper to find code (Filename first, then inside archive)
                let find_code = || -> Option<String> {
                    let info = current_archive_info()?;
                    // 1. Check filename
                    if let Some(code) = detect_dlsite_code(&info.filename) {
                        return Some(code);
                    }
                    // 2. Check archive contents
                    if let Ok(files) = list_archive_files() {
                        for file in files {
                            if let Some(code) = detect_dlsite_code(&file) {
                                return Some(code);
                            }
                        }
                    }
                    None
                };

                if let Some(code) = find_code() {
                     // 1. Check cache first (Fast path)
                     if let Some(cached_json) = get_cached_metadata(&code) {
                        if let Ok(_cached_value) = serde_json::from_str::<serde_json::Value>(&cached_json) {
                             archust_plugin_sdk::emit_metadata(&cached_json);
                             STATE.with(|state| {
                                 let mut s = state.borrow_mut();
                                 s.last_status = format!("Loaded {} from cache", code);
                                 // Reconstruct minimal metadata for UI if needed (omitted for brevity, 
                                 // assuming emit_metadata handles the main app state, 
                                 // but we can populate found_metadata if we want the "Details" button to work immediately)
                                 
                                 // To make "Show Details" work, we'd need to reconstruct the scraped data or parse fully.
                                 // For now, just indicating success.
                             });
                             return vec![];
                        }
                     }

                     // 2. Fetch from network (Async path)
                     let url = format!("https://www.dlsite.com/home/work/=/product_id/{}.html", code);
                     let id = archust_plugin_sdk::start_async_fetch(&url);
                     STATE.with(|state| {
                         let mut s = state.borrow_mut();
                         s.async_fetch_id = Some(id);
                         s.async_fetch_type = Some(AsyncFetchType::FetchMetadata);
                         s.async_fetch_context = Some(code.clone());
                         s.last_status = format!("Fetching metadata for {}...", code);
                     });
                } else {
                     archust_plugin_sdk::show_message("Error", "No DLSite code found in archive.");
                }
                vec![]
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
                vec![]
            }
            // Dialog event handlers
            "dialog_search_input" => {
                if let Some(query) = value {
                    STATE.with(|state| {
                        state.borrow_mut().dialog_query = query;
                    });
                }
                vec![]
            }
            "dialog_perform_search" => {
                let query = STATE.with(|state| state.borrow().dialog_query.clone());
                info(&format!("[DLSite Plugin] Performing search for: {}", query));
                
                if !query.is_empty() {
                    let url = build_search_url(&query);
                    let id = archust_plugin_sdk::start_async_fetch(&url);
                    
                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.dialog_fetching = true;
                        s.dialog_status = format!("Searching for '{}'...", query);
                        s.async_fetch_id = Some(id);
                        s.async_fetch_type = Some(AsyncFetchType::Search);
                    });
                    
                    // Return RefreshPanel to trigger UI update (to show status)
                    vec![PluginAction::RefreshPanel("manual_search".to_string())]
                } else {
                    vec![]
                }
            }

            "dialog_clear" => {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.dialog_query.clear();
                    s.dialog_results.clear();
                    s.dialog_selected_index = None;
                    s.dialog_selected_details = None;
                    s.dialog_selected_image_index = 0;
                    s.dialog_status.clear();
                });
                vec![]
            }
            "dialog_apply_result" => {
                // Apply selected result to the archive
                let details = STATE.with(|s| s.borrow().dialog_selected_details.clone());
                if let Some(details) = details {
                    // Generate and emit metadata
                    let metadata_json = generate_metadata_json(&details.code, Some(&(details.data.clone(), details.scraped.clone())));
                    archust_plugin_sdk::save_cached_metadata(&details.code, &metadata_json);
                    archust_plugin_sdk::emit_metadata(&metadata_json);
                    
                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.found_metadata = Some((details.code.clone(), details.data, details.scraped));
                        s.last_status = format!("Applied metadata for {}", details.code);
                    });
                    
                    return vec![PluginAction::ShowToast(ToastConfig {
                        message: "Metadata applied successfully!".to_string(),
                        level: ToastLevel::Success,
                    })];
                }
                vec![]
            }
            _ if id.starts_with("dialog_select_") => {
                let idx: usize = id.trim_start_matches("dialog_select_").parse().unwrap_or(0);
                
                // Get the selected result info
                let result_info = STATE.with(|s| {
                    let state = s.borrow();
                    state.dialog_results.get(idx).cloned()
                });
                
                if let Some(result) = result_info {
                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.dialog_selected_index = Some(idx);
                        s.dialog_status = format!("Loading details for {}...", result.code);
                    });
                    
                    // Fetch full details
                    if let Some((json, scraped)) = fetch_dlsite_metadata(&result.code) {
                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.dialog_selected_details = Some(WorkDetails {
                                code: result.code.clone(),
                                data: json,
                                scraped,
                            });
                            s.dialog_selected_image_index = 0;
                            s.dialog_status = format!("Loaded: {}", result.code);
                        });
                    } else {
                        STATE.with(|state| {
                            state.borrow_mut().dialog_status = format!("Failed to load details for {}", result.code);
                        });
                    }
                }
                vec![]
            }
            _ if id.starts_with("dialog_img_") => {
                let idx: usize = id.trim_start_matches("dialog_img_").parse().unwrap_or(0);
                STATE.with(|state| {
                    state.borrow_mut().dialog_selected_image_index = idx;
                });
                vec![]
            }
            _ => vec![]
        }
    }
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
    author: Option<String>,
    release_date: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    series: Option<String>,
    illustrator: Option<String>,
    voice_actors: Vec<String>,
    rating: Option<f32>,
    screenshots: Vec<String>,
}

fn scrape_html_metadata(html: &str) -> Option<ScrapedData> {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    
    let mut data = ScrapedData {
        title: None,
        circle: None,
        author: None,
        release_date: None,
        description: None,
        tags: Vec::new(),
        series: None,
        illustrator: None,
        voice_actors: Vec::new(),
        rating: None,
        screenshots: Vec::new(),
    };

    // 1. Basic Title (h1#work_name)
    let title_selector = Selector::parse("h1#work_name").unwrap();
    if let Some(h1) = document.select(&title_selector).next() {
        data.title = Some(h1.text().collect::<String>().trim().to_string());
    }

    // 2. Parse Tables (work_maker and work_outline)
    // iterate over all TH elements, check text, then get corresponding TD
    let row_selector = Selector::parse("table#work_maker tr, table#work_outline tr").unwrap();
    let th_selector = Selector::parse("th").unwrap();
    let td_selector = Selector::parse("td").unwrap();
    let a_selector = Selector::parse("a").unwrap();
    
    for row in document.select(&row_selector) {
        let th_text = match row.select(&th_selector).next() {
            Some(th) => th.text().collect::<String>().trim().to_lowercase(),
            None => continue,
        };
        
        let td = match row.select(&td_selector).next() {
            Some(td) => td,
            None => continue,
        };

        if th_text.contains("circle") || th_text.contains("サークル名") || th_text.contains("brand") {
            let maker_selector = Selector::parse("span.maker_name").unwrap();
             if let Some(span) = td.select(&maker_selector).next() {
                 data.circle = Some(span.text().collect::<String>().trim().to_string());
            } else {
                 data.circle = Some(td.text().collect::<String>().trim().to_string());
            }
        } else if th_text.contains("author") || th_text.contains("作者") {
             data.author = Some(td.text().collect::<String>().trim().to_string());
        } else if th_text.contains("release date") || th_text.contains("販売日") {
             data.release_date = Some(td.text().collect::<String>().trim().to_string());
        } else if th_text.contains("series") || th_text.contains("シリーズ") {
             data.series = Some(td.text().collect::<String>().trim().to_string());
        } else if th_text.contains("illustration") || th_text.contains("イラスト") {
             data.illustrator = Some(td.text().collect::<String>().trim().to_string());
        } else if th_text.contains("voice actor") || th_text.contains("声優") {
             for a in td.select(&a_selector) {
                 data.voice_actors.push(a.text().collect::<String>().trim().to_string());
             }
        } else if th_text.contains("genre") || th_text.contains("ジャンル") {
             for a in td.select(&a_selector) {
                 data.tags.push(a.text().collect::<String>().trim().to_string());
             }
        }
    }

    // 3. Description (Meta or Div)
    let meta_desc_selector = Selector::parse("meta[name='description']").unwrap();
    if let Some(meta) = document.select(&meta_desc_selector).next() {
        if let Some(content) = meta.value().attr("content") {
            data.description = Some(content.trim().to_string());
        }
    }
    if data.description.is_none() {
        let parts_selector = Selector::parse("div.work_parts_area").unwrap();
        if let Some(div) = document.select(&parts_selector).next() {
            data.description = Some(div.text().collect::<String>().trim().to_string());
        }
    }
    
    // 4. Rating (Meta)
    let meta_rating_selector = Selector::parse("meta[itemprop='ratingValue']").unwrap();
    if let Some(meta) = document.select(&meta_rating_selector).next() {
        if let Some(val_str) = meta.value().attr("content") {
             if let Ok(val) = val_str.parse::<f32>() {
                 data.rating = Some(val);
             }
        }
    }

    // 5. Screenshots
    let slider_selector = Selector::parse("div.product-slider-data div").unwrap();
    for div in document.select(&slider_selector) {
        if let Some(src) = div.value().attr("data-src") {
             if !src.contains("_img_main") {
                let full_url = if src.starts_with("//") {
                    format!("https:{}", src)
                } else {
                    src.to_string()
                };
                data.screenshots.push(full_url);
             }
        }
    }

    Some(data)
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
            "dlsite_id": product_id,
            "rating": scraped_data.and_then(|s| s.rating),
            "series": scraped_data.and_then(|s| s.series.clone()),
            "illustrator": scraped_data.and_then(|s| s.illustrator.clone()),
            "voice_actors": scraped_data.map(|s| s.voice_actors.clone()).unwrap_or_default()
        }
    });

    metadata.to_string()
}

/// Build search URL for DLSite
fn build_search_url(query: &str) -> String {
    format!(
        "https://www.dlsite.com/home/fsr/=/keyword/{}",
        urlencoding::encode(query)
    )
}

/// Parse DLSite search results HTML
fn parse_search_results(html: &str) -> Vec<SearchResultInfo> {
    use scraper::{Html, Selector};
    
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    // Select search results
    let item_selector = Selector::parse("li.search_result_img_box_inner, tr.n_worklist_item").unwrap();
    let title_selector = Selector::parse("dt.work_name a, a.work_name").unwrap();
    let maker_selector = Selector::parse("dd.maker_name a, span.maker_name a").unwrap();
    let author_selector = Selector::parse("dd.author_name a, span.author_name a").unwrap();
    let work_text_selector = Selector::parse("dt.work_text").unwrap(); // For description/rating sometimes
    
    // Additional selectors for table view or other layouts
    let date_selector = Selector::parse("span.release_date").unwrap();
    let price_selector = Selector::parse("span.work_price").unwrap();

    for item in document.select(&item_selector) {
        let mut info = SearchResultInfo {
            code: String::new(),
            title: "Unknown".to_string(),
            circle: "Unknown".to_string(),
            author: "Unknown".to_string(),
            release_date: None,
            age_rating: None,
            price: None,
        };

        if let Some(link) = item.select(&title_selector).next() {
            info.title = link.text().collect::<String>().trim().to_string();
            if let Some(href) = link.value().attr("href") {
                if let Some(c) = detect_dlsite_code(href) {
                    info.code = c;
                }
            }
        }

        if let Some(maker_link) = item.select(&maker_selector).next() {
            info.circle = maker_link.text().collect::<String>().trim().to_string();
        }
        
        if let Some(author_link) = item.select(&author_selector).next() {
            info.author = author_link.text().collect::<String>().trim().to_string();
        }

        // Try to find release date
        if let Some(date_elem) = item.select(&date_selector).next() {
             info.release_date = Some(date_elem.text().collect::<String>().trim().to_string());
        }
        
        // Try to find price
        if let Some(price_elem) = item.select(&price_selector).next() {
            info.price = Some(price_elem.text().collect::<String>().trim().to_string());
        }
        
        // Try to extract age rating from work text or classes
        // Simple heuristic: if we find "R18" or similar in text
        if let Some(text_elem) = item.select(&work_text_selector).next() {
            let text = text_elem.text().collect::<String>();
            if text.contains("R18") || text.contains("Adult") {
                info.age_rating = Some("R18".to_string());
            } else if text.contains("All Ages") {
                info.age_rating = Some("All".to_string());
            }
        }
        
        // Also check for specific age icons if available (often span.icon_R18 or similar)
        let r18_selector = Selector::parse("span.icon_R18").unwrap();
        if item.select(&r18_selector).next().is_some() {
             info.age_rating = Some("R18".to_string());
        }
        
        if !info.code.is_empty() {
            results.push(info);
        }
        
        if results.len() >= 20 {
            break;
        }
    }
    
    results
}

/// Search DLSite for a query and return list of results
/// Blocking wrapper for compatibility
fn search_dlsite(query: &str) -> Vec<SearchResultInfo> {
    use archust_plugin_sdk::{http_get, log_network_activity};

    let url = build_search_url(query);
    
    log_network_activity(&format!("Searching DLSite: {}", query));
    log_network_activity(&format!("GET {}", url));

    let html = match http_get(&url) {
        Ok(h) => h,
        Err(e) => {
            log_network_activity(&format!("Search failed: {}", e));
            return Vec::new();
        }
    };
    
    let results = parse_search_results(&html);
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
