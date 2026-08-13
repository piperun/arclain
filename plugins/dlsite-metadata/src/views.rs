//! UI view rendering for the DLsite plugin.
//!
//! Was the body of `Component::get_ui_layout` — a single match block
//! that ate 1300+ LOC of `lib.rs`. Lifted out so the entry-point file
//! is just plumbing (state, lifecycle hooks, free-fn helpers, the WIT
//! `Guest` impl) instead of being buried under nested
//! `UiElement::*` constructors.
//!
//! `dispatch` keeps the original match shape — same arm patterns,
//! same arm bodies — so existing extension-point callers see no
//! behaviour change. The `Page:dlsite_browser:<id>` guard arm
//! recurses into `dispatch("Page:dlsite_browser")` instead of the
//! old `Self::get_ui_layout(…)` since we're outside the `Component`
//! impl now.

use crate::{
    detect_dlsite_code, format_description, generate_metadata_json, get_cached_dlsite_metadata,
    STATE,
};
use wirt_sdk::info;

pub(crate) fn dispatch(
    extension_point: &str,
) -> wirt_sdk::wirt::plugin::ui::PluginLayout {
    use wirt_sdk::wirt::plugin::ui::{
        ButtonAction, ButtonConfig, CarouselConfig, CheckboxConfig, DropdownConfig, ImageConfig,
        KeyValueListConfig, KeyValuePair, LabelConfig, ListContainerConfig, ListItemConfig,
        LoadingConfig, MetadataGridConfig, PluginLayout, SectionHeaderConfig, SettingsGroupHeader,
        SidebarWidth, SizeHint, SpacingStep, SplitConfig, TabsConfig, TagChipsConfig,
        TextInputConfig, TextRole, ToolbarButtonConfig, ToolbarConfig, UiElement, WarningConfig,
        WarningIcon,
    };

    match extension_point {
        "PluginButton" => {
            use wirt_sdk::current_archive_info;

            // Fetch button - only if archive is open. Show a Loading
            // spinner instead while a fetch is already in flight so the
            // button can't be re-clicked (and the user gets visual
            // feedback that something is happening).
            let mut buttons = vec![];
            if current_archive_info().is_some() {
                let in_progress = STATE.with(|s| s.borrow().fetch_in_progress);
                if in_progress {
                    buttons.push(UiElement::Loading(LoadingConfig {
                        message: Some("Fetching DLSite metadata\u{2026}".to_string()),
                    }));
                } else {
                    buttons.push(UiElement::Button(ButtonConfig {
                        id: "fetch_metadata".to_string(),
                        label: "Fetch DLSite".to_string(),
                        action: None,
                    }));
                }
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
        }
        "MainPage" => {
            let auto_fetch_enabled = STATE.with(|s| s.borrow().auto_fetch_enabled);
            let enable_cache = STATE.with(|s| s.borrow().enable_cache);
            let dump_html_debug = STATE.with(|s| s.borrow().dump_html_debug);
            let (cache_videos, video_quality) = STATE.with(|s| {
                let st = s.borrow();
                (st.cache_videos, st.video_quality.clone())
            });

            let mut elements = Vec::new();

            // Plugin Configuration group
            elements.push(UiElement::GroupBegin(SettingsGroupHeader {
                title: "Plugin Configuration".to_string(),
                description: None,
            }));
            elements.push(UiElement::Checkbox(CheckboxConfig {
                id: "auto_fetch_enabled".to_string(),
                label: "Auto-fetch metadata when archive opens".to_string(),
                checked: auto_fetch_enabled,
            }));
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
                placeholder: None,
            }));
            elements.push(UiElement::GroupEnd);

            // Cache Management group
            elements.push(UiElement::GroupBegin(SettingsGroupHeader {
                title: "Cache Management".to_string(),
                description: None,
            }));
            elements.push(UiElement::Button(ButtonConfig {
                id: "clear_invalid_cache".to_string(),
                label: "Prune Invalid/Corrupt Entries".to_string(),
                action: Some(ButtonAction::Custom("prune_cache".to_string())),
            }));
            elements.push(UiElement::Button(ButtonConfig {
                id: "clear_all_cache".to_string(),
                label: "Clear All DLSite Cache".to_string(),
                action: Some(ButtonAction::ShowDialog("confirm_clear_cache".to_string())),
            }));
            elements.push(UiElement::GroupEnd);

            // Media Downloads group
            elements.push(UiElement::GroupBegin(SettingsGroupHeader {
                title: "Media Downloads".to_string(),
                description: Some(
                    "Optionally download videos referenced from the work's description page. \
                     Videos are big — leave off unless you really want them locally."
                        .to_string(),
                ),
            }));
            elements.push(UiElement::Checkbox(CheckboxConfig {
                id: "cache_videos".to_string(),
                label: "Download chobit-embed videos".to_string(),
                checked: cache_videos,
            }));
            if cache_videos {
                let quality_options = vec![
                    "best".to_string(),
                    "1080".to_string(),
                    "720".to_string(),
                    "480".to_string(),
                    "360".to_string(),
                    "low".to_string(),
                ];
                elements.push(UiElement::Dropdown(DropdownConfig {
                    id: "video_quality".to_string(),
                    label: "Preferred quality".to_string(),
                    options: quality_options,
                    selected: video_quality.clone(),
                }));
            }
            elements.push(UiElement::GroupEnd);

            // Debug group
            elements.push(UiElement::GroupBegin(SettingsGroupHeader {
                title: "Debug".to_string(),
                description: Some(
                    "Diagnostic helpers — leave off unless troubleshooting.".to_string(),
                ),
            }));
            elements.push(UiElement::Checkbox(CheckboxConfig {
                id: "dump_html_debug".to_string(),
                label: "Dump HTML to file on Geo-Block".to_string(),
                checked: dump_html_debug,
            }));
            elements.push(UiElement::GroupEnd);

            PluginLayout::Single(elements)
        }
        "Panel" => {
            use wirt_sdk::current_archive_info;

            // Check if archive is open
            let archive_info = current_archive_info();
            if archive_info.is_none() {
                return PluginLayout::Single(vec![
                    UiElement::Label(LabelConfig {
                        text: "DLSite Metadata".to_string(),
                        role: TextRole::Subtitle,
                    }),
                    UiElement::Label(LabelConfig {
                        text: "No archive open".to_string(),
                        role: TextRole::Body,
                    }),
                ]);
            }

            // Get archive path to detect changes
            let archive_path = archive_info
                .as_ref()
                .map(|i| i.path.clone())
                .unwrap_or_default();

            // Check if DLSite code can be detected from filename
            let archive_name = archive_info
                .as_ref()
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
                            s.found_metadata =
                                Some((code.clone(), api_data.clone(), scraped.clone()));
                        });

                        // Emit to signal so Panel reads from it
                        let metadata_json =
                            generate_metadata_json(code, Some(&(api_data, scraped)));
                        crate::emit_dlsite_metadata(&metadata_json);
                    }
                }
            }

            let mut elements = vec![];

            STATE.with(|state| {
                let state = state.borrow();

                if let Some((id, data, scraped)) = &state.found_metadata {
                    // Metadata found - show info
                    // Handle both raw API format (work_name) and ProductMetadata format (title)

                    let title = data["work_name"]
                        .as_str()
                        .or_else(|| data["title"].as_str());

                    // Maker/Circle
                    let maker = data["maker_name"]
                        .as_str()
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
                    let cover_cache_key = gameta_lib::providers::dlsite::cache_keys::cover_key(id);
                    let cover_url = scraped.as_ref().and_then(|s| s.cover_image.clone());

                    // Always attempt to show cover - the host will check cache by key
                    elements.push(UiElement::Image(ImageConfig {
                        cache_key: Some(cover_cache_key),
                        url: cover_url, // May be None, host will use cache key
                        height: Some(SizeHint::Compact),
                    }));

                    // Title (prominent)
                    if let Some(t) = title {
                        elements.push(UiElement::Label(LabelConfig {
                            text: t.to_string(),
                            role: TextRole::Subtitle,
                        }));
                    }

                    // Metadata fields as key-value grid
                    let release_date = data["regist_date"]
                        .as_str()
                        .or_else(|| data["release_date"].as_str());
                    let date_clean = release_date
                        .filter(|d| !d.is_empty())
                        .map(|d| d.split_whitespace().next().unwrap_or(d));

                    let mut kv_items = vec![KeyValuePair {
                        key: "ID".to_string(),
                        value: id.to_string(),
                    }];
                    if let Some(m) = maker {
                        kv_items.push(KeyValuePair {
                            key: "Circle".to_string(),
                            value: m.to_string(),
                        });
                    }
                    if let Some(date) = date_clean {
                        kv_items.push(KeyValuePair {
                            key: "Released".to_string(),
                            value: date.to_string(),
                        });
                    }

                    elements.push(UiElement::KeyValueList(KeyValueListConfig {
                        items: kv_items,
                        columns: Some(1), // Single column in sidebar
                    }));

                    // Price removed from Panel - only shown in full info dialog

                    // Tags from scraped data (use TagChips for pill-style display)
                    if let Some(scraped_data) = scraped {
                        if !scraped_data.tags.is_empty() {
                            elements.push(UiElement::TagChips(TagChipsConfig {
                                tags: scraped_data.tags.clone(),
                                max_display: Some(5), // Show up to 5 tags, rest as "+N more"
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
                            role: TextRole::Body,
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
                            role: TextRole::Body,
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
            UiElement::Space(SpacingStep::Small),
            UiElement::Label(LabelConfig {
                text: "DLSite Info".to_string(),
                role: TextRole::Emphasis,
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
                        let title = data["work_name"]
                            .as_str()
                            .or_else(|| data["title"].as_str());

                        if let Some(t) = title {
                            elements.push(UiElement::Label(LabelConfig {
                                text: t.to_string(),
                                role: TextRole::Title,
                            }));
                        }

                        elements.push(UiElement::Separator);

                        // Circle/Maker (check both field names)
                        let maker = data["maker_name"]
                            .as_str()
                            .or_else(|| data["creator"].as_str());

                        if let Some(m) = maker {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Circle: {}", m),
                                role: TextRole::Body,
                            }));
                        }

                        // Product ID
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("Product ID: {}", id),
                            role: TextRole::Body,
                        }));

                        // Release date
                        if let Some(date) = data["regist_date"].as_str() {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Release Date: {}", date),
                                role: TextRole::Body,
                            }));
                        }

                        // Price
                        if let Some(price) = data["price"].as_u64() {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Price: ¥{}", price),
                                role: TextRole::Body,
                            }));
                        }

                        // Age rating
                        if let Some(rating) = data["age_category_string"].as_str() {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("Age Rating: {}", rating),
                                role: TextRole::Body,
                            }));
                        }

                        // File count
                        if let Some(count) = data["file_count"].as_str() {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("File Count: {}", count),
                                role: TextRole::Body,
                            }));
                        }

                        // File size
                        if let Some(size) = data["file_size"].as_str() {
                            elements.push(UiElement::Label(LabelConfig {
                                text: format!("File Size: {}", size),
                                role: TextRole::Body,
                            }));
                        }

                        elements.push(UiElement::Separator);

                        // Tags from scraped data (use TagChips for pill-style display)
                        if let Some(scraped_data) = scraped {
                            if !scraped_data.tags.is_empty() {
                                elements.push(UiElement::Space(SpacingStep::Small));
                                elements.push(UiElement::Label(LabelConfig {
                                    text: "Tags:".to_string(),
                                    role: TextRole::Emphasis,
                                }));
                                elements.push(UiElement::TagChips(TagChipsConfig {
                                    tags: scraped_data.tags.clone(),
                                    max_display: Some(10), // Show more tags in dialog
                                }));
                            }

                            // Description if available
                            if let Some(desc) = &scraped_data.description {
                                if !desc.is_empty() {
                                    elements.push(UiElement::Separator);
                                    elements.push(UiElement::Space(SpacingStep::Small));
                                    elements.push(UiElement::Label(LabelConfig {
                                        text: "Description:".to_string(),
                                        role: TextRole::Emphasis,
                                    }));
                                    // Truncate long descriptions
                                    let desc_text = if desc.len() > 500 {
                                        format!("{}...", &desc[..500])
                                    } else {
                                        desc.clone()
                                    };
                                    elements.push(UiElement::Label(LabelConfig {
                                        text: desc_text,
                                        role: TextRole::Body,
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
                            role: TextRole::Body,
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
                    role: TextRole::Subtitle,
                }));

                elements.push(UiElement::Separator);

                STATE.with(|state| {
                    let state = state.borrow();

                    elements.push(UiElement::TextInput(TextInputConfig {
                        id: "search_query".to_string(),
                        label: "Search Query".to_string(),
                        value: state.search_query.clone(),
                        placeholder: None,
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
                        elements.push(UiElement::Space(SpacingStep::Small));
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("{} results:", state.search_results.len()),
                            role: TextRole::Emphasis,
                        }));

                        for (code, title, maker, _thumb_url) in &state.search_results {
                            elements.push(UiElement::Button(ButtonConfig {
                                id: format!("select_result_{}", code),
                                label: format!("[{}] {} ({})", code, title, maker),
                                action: None,
                            }));
                        }
                    }
                });

                elements.push(UiElement::Separator);

                // Rename option
                let rename_checked = STATE.with(|s| s.borrow().rename_with_code);
                elements.push(UiElement::Checkbox(CheckboxConfig {
                    id: "rename_with_code".to_string(),
                    label: "Rename archive with code (e.g., [RJ123456] Title.7z)".to_string(),
                    checked: rename_checked,
                }));

                elements.push(UiElement::Space(SpacingStep::Medium));

                elements.push(UiElement::Button(ButtonConfig {
                    id: "close_search_dialog".to_string(),
                    label: "Cancel".to_string(),
                    action: Some(ButtonAction::CloseDialog),
                }));

                PluginLayout::Single(elements)

            // Confirm Clear Cache Dialog
            } else if dialog_id == "confirm_clear_cache" {
                PluginLayout::Single(vec![
                    UiElement::Label(LabelConfig {
                        text: "Confirm Deletion".to_string(),
                        role: TextRole::Title,
                    }),
                    UiElement::Separator,
                    UiElement::Label(LabelConfig {
                        text: "Are you sure you want to clear ALL DLSite cache?".to_string(),
                        role: TextRole::Body,
                    }),
                    UiElement::Warning(WarningConfig {
                        icon: WarningIcon::Warning,
                        message: "This action cannot be undone. All cached metadata and images will be removed.".to_string(),
                    }),
                    UiElement::Space(SpacingStep::Large),
                    UiElement::Button(ButtonConfig {
                        id: "do_clear_all_cache".to_string(),
                        label: "Yes, Clear All".to_string(),
                        action: Some(ButtonAction::Custom("do_clear_all_cache".to_string())),
                    }),
                    UiElement::Button(ButtonConfig {
                        id: "cancel_clear".to_string(),
                        label: "Cancel".to_string(),
                        action: Some(ButtonAction::CloseDialog),
                    }),
                ])

            // Cache viewer dialog
            } else if dialog_id == "dlsite_cache" {
                use wirt_sdk::list_cached_metadata;
                let mut elements = vec![];

                // Check if we have a selected entry to show details for

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
                        role: TextRole::Title,
                    }));

                    // Read the data from cache to display (no network fetch!)
                    info(&format!(
                        "[DLSite Plugin] get_ui_layout detail view for entry_id={}",
                        entry_id
                    ));
                    if let Some((json, scraped)) = get_cached_dlsite_metadata(&entry_id) {
                        info("[DLSite Plugin] get_cached_dlsite_metadata returned Some");
                        // Check geo-blocked status from stored JSON first (most reliable)
                        let json_geo_blocked = json
                            .get("geo_blocked")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let scraped_geo_blocked =
                            scraped.as_ref().map(|s| s.geo_blocked).unwrap_or(false);
                        let is_geo_blocked = json_geo_blocked || scraped_geo_blocked;

                        info(&format!(
                            "[DLSite Plugin] Cache browser: json_geo_blocked={}, scraped_geo_blocked={}, is_geo_blocked={}",
                            json_geo_blocked, scraped_geo_blocked, is_geo_blocked
                        ));

                        if is_geo_blocked {
                            info("[DLSite Plugin] Pushing Warning element to UI");
                            elements.push(UiElement::Warning(WarningConfig {
                                icon: WarningIcon::GlobeX,
                                message: "This product is geo-blocked. Metadata may be incomplete."
                                    .to_string(),
                            }));
                        }

                        // Copy-paste of info display logic (could be refactored into helper)
                        let title = json["work_name"].as_str().unwrap_or("Unknown Title");
                        elements.push(UiElement::Label(LabelConfig {
                            text: title.to_string(),
                            role: TextRole::Body,
                        }));

                        if let Some(scraped_data) = scraped {
                            if let Some(cover_url) = &scraped_data.cover_image {
                                elements.push(UiElement::Image(ImageConfig {
                                    cache_key: Some(
                                        gameta_lib::providers::dlsite::cache_keys::cover_key(
                                            &entry_id,
                                        ),
                                    ),
                                    url: Some(cover_url.clone()),
                                    height: Some(SizeHint::Regular),
                                }));
                            }
                            if let Some(desc) = &scraped_data.description {
                                elements.push(UiElement::Separator);
                                elements.push(UiElement::Label(LabelConfig {
                                    text: desc.clone(),
                                    role: TextRole::Body,
                                }));
                            }
                        }
                    } else {
                        elements.push(UiElement::Label(LabelConfig {
                            text: "Failed to load details".to_string(),
                            role: TextRole::Body,
                        }));
                    }
                } else {
                    // === LIST VIEW ===
                    elements.push(UiElement::Label(LabelConfig {
                        text: "DLSite Metadata Cache".to_string(),
                        role: TextRole::Subtitle,
                    }));

                    elements.push(UiElement::Separator);

                    // Search Box
                    STATE.with(|state| {
                        let state = state.borrow();
                        elements.push(UiElement::TextInput(TextInputConfig {
                            id: "search_query".to_string(),
                            label: "Filter Cache".to_string(),
                            value: state.search_query.clone(),
                            placeholder: None,
                        }));
                    });

                    // Refresh Button
                    elements.push(UiElement::Button(ButtonConfig {
                        id: "refresh_cache".to_string(),
                        label: "Refresh List".to_string(),
                        action: None,
                    }));

                    elements.push(UiElement::Separator);

                    // Dialog stays intentionally bounded; the full browser has
                    // explicit pagination controls.
                    let entries = list_cached_metadata("dlsite", 0, 50).unwrap_or_else(|e| {
                        info(&format!("Failed to list cache: {}", e));
                        vec![]
                    });

                    // Filter
                    let query = STATE.with(|s| s.borrow().search_query.to_lowercase());
                    let filtered_entries: Vec<_> = entries
                        .iter()
                        .filter(|id| query.is_empty() || id.to_lowercase().contains(&query))
                        .collect();

                    if filtered_entries.is_empty() {
                        elements.push(UiElement::Label(LabelConfig {
                            text: "No matching entries".to_string(),
                            role: TextRole::Body,
                        }));
                    } else {
                        elements.push(UiElement::Label(LabelConfig {
                            text: format!("{} entries", filtered_entries.len()),
                            role: TextRole::Body,
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

        // Handle Page:dlsite_browser:RJ123456 (navigation with product ID)
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
            return dispatch("Page:dlsite_browser");
        }

        "Page:dlsite_browser" => {
            use wirt_sdk::list_cached_entries;

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

            // Title removed - page title bar already shows "DLSite Browser"

            // 1. Tabs
            sidebar_elements.push(UiElement::Tabs(TabsConfig {
                id: "browser_tabs".to_string(),
                tabs: vec!["Cached".to_string(), "Search".to_string()],
                selected: if browser_tab == "search" {
                    "Search".to_string()
                } else {
                    "Cached".to_string()
                },
            }));

            sidebar_elements.push(UiElement::Separator);

            // 3. Search/Filter (use placeholder for simple input without label title)
            let filter_hint = if browser_tab == "search" {
                "Search..."
            } else {
                "Filter cache..."
            };
            sidebar_elements.push(UiElement::TextInput(TextInputConfig {
                id: "browser_query".to_string(),
                label: String::new(), // Not used when placeholder is set
                value: search_query.clone(),
                placeholder: Some(filter_hint.to_string()),
            }));

            if browser_tab == "search" {
                sidebar_elements.push(UiElement::Button(ButtonConfig {
                    id: "do_dlsite_search".to_string(),
                    label: "Search".to_string(),
                    action: None,
                }));

                // Show current archive filename with copy button
                if let Some(archive_info) = wirt_sdk::current_archive_info() {
                    sidebar_elements.push(UiElement::Space(SpacingStep::Small));
                    sidebar_elements.push(UiElement::Label(LabelConfig {
                        text: "Current Archive:".to_string(),
                        role: TextRole::Caption,
                    }));
                    // Truncate long filenames for display
                    let filename = &archive_info.filename;
                    let display_name = if filename.len() > 40 {
                        format!("{}...", &filename[..37])
                    } else {
                        filename.clone()
                    };
                    sidebar_elements.push(UiElement::Label(LabelConfig {
                        text: display_name,
                        role: TextRole::Caption,
                    }));
                    sidebar_elements.push(UiElement::Button(ButtonConfig {
                        id: "copy_archive_filename".to_string(),
                        label: "Copy Filename".to_string(),
                        action: None,
                    }));
                }
            }

            sidebar_elements.push(UiElement::Separator);

            // 4. List Content
            if browser_loading {
                sidebar_elements.push(UiElement::Loading(LoadingConfig {
                    message: Some("Searching...".to_string()),
                }));
            } else if browser_tab == "search" {
                // Search Results
                let search_results = STATE.with(|s| s.borrow().search_results.clone());

                let items: Vec<ListItemConfig> = search_results
                    .iter()
                    .filter(|(code, title, _, _)| {
                        search_query.is_empty()
                            || title.to_lowercase().contains(&search_query.to_lowercase())
                            || code.to_lowercase().contains(&search_query.to_lowercase())
                    })
                    .map(|(code, title, maker, thumb_url)| {
                        // Use thumbnail (small ~100x100) instead of full cover for fast loading
                        let thumb_key =
                            gameta_lib::providers::dlsite::cache_keys::thumbnail_key(code);
                        ListItemConfig {
                            id: code.clone(),
                            title: title.clone(),
                            subtitle: Some(maker.clone()),
                            badge: Some(code.clone()),
                            // Use thumbnail URL if available, host will cache it automatically
                            image_key: thumb_url.as_ref().map(|_| thumb_key),
                            image_url: thumb_url.clone(),
                            selected: selected_entry.as_ref() == Some(code),
                            warning_icon: None,
                        }
                    })
                    .collect();

                sidebar_elements.push(UiElement::ListContainer(ListContainerConfig {
                    id: "browser_list".to_string(),
                    items,
                    height: Some(SizeHint::Tall),
                    empty_message: Some("Enter a search term".to_string()),
                }));
            } else {
                // `list_cached_entries` is host-cached now (Path D step 1)
                // — calling it every frame is fine, no plugin-side memo
                // needed. This used to be a `STATE.with(...)` block that
                // memoized into `state.cached_entries`; that field was
                // removed when the host cache landed.
                let entries = list_cached_entries().unwrap_or_else(|e| {
                    info(&format!("Failed to list cache: {}", e));
                    vec![]
                });

                // Filter and limit entries first
                let filtered_ids: Vec<String> = entries
                    .iter()
                    .filter(|id| {
                        search_query.is_empty()
                            || id.to_lowercase().contains(&search_query.to_lowercase())
                    })
                    .take(100)
                    .cloned()
                    .collect();

                // Use cached summaries to avoid DB queries on every frame
                let entries_with_summaries: Vec<(String, Option<String>, bool)> = STATE.with(|s| {
                    let mut state = s.borrow_mut();
                    // Check if we need to refresh summaries (entries changed or not cached)
                    let need_refresh = state
                        .cached_summaries
                        .as_ref()
                        .map(|(ids, _)| ids != &filtered_ids)
                        .unwrap_or(true);

                    if need_refresh {
                        let summaries =
                            wirt_sdk::get_metadata_summaries(filtered_ids.clone());
                        let data: Vec<(String, Option<String>, bool)> = summaries
                            .into_iter()
                            .map(|s| (s.id, s.title, s.geo_blocked))
                            .collect();
                        state.cached_summaries = Some((filtered_ids, data.clone()));
                        data
                    } else {
                        state
                            .cached_summaries
                            .as_ref()
                            .map(|(_, d)| d.clone())
                            .unwrap_or_default()
                    }
                });

                let items: Vec<ListItemConfig> = entries_with_summaries
                    .into_iter()
                    .map(|(id, title, geo_blocked)| {
                        let display_title = title.unwrap_or_else(|| id.clone());
                        let selected = selected_entry.as_ref() == Some(&id);
                        // Use thumbnail for fast list rendering (small 240x240 from CDN)
                        let thumb_key =
                            gameta_lib::providers::dlsite::cache_keys::thumbnail_key(&id);
                        // Construct CDN thumbnail URL so it can be fetched if not cached
                        let thumb_url = gameta_lib::urls::dlsite::thumbnail_url(&id);
                        ListItemConfig {
                            id: format!("view_cache_entry_{}", id),
                            title: display_title,
                            subtitle: Some("Cached".to_string()),
                            badge: Some(id.clone()),
                            image_key: Some(thumb_key),
                            image_url: thumb_url,
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
                    height: Some(SizeHint::Tall),
                    empty_message: Some("No cached entries".to_string()),
                }));
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
                    let is_geo_blocked = json
                        .get("geo_blocked")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        || scraped.as_ref().map(|s| s.geo_blocked).unwrap_or(false);

                    if is_geo_blocked {
                        content_elements.push(UiElement::Warning(WarningConfig {
                            icon: WarningIcon::GlobeX,
                            message: "This product is geo-blocked. Metadata may be incomplete."
                                .to_string(),
                        }));
                    }

                    // Handle both DLSite API format (work_name, maker_name) and cache format (title, circle)
                    let title = json["title"]
                        .as_str()
                        .or(json["work_name"].as_str())
                        .unwrap_or("Unknown Title");
                    let maker = json["maker_name"]
                        .as_str()
                        .or(json["brand"].as_str())
                        .or(json["circle"].as_str())
                        .or(json["creator"].as_str())
                        .unwrap_or("Unknown Maker");
                    let release_date = json["release_date"]
                        .as_str()
                        .or(json["regist_date"].as_str());

                    // ===== TOOLBAR AT TOP =====
                    content_elements.push(UiElement::Toolbar(ToolbarConfig {
                        buttons: vec![
                            ToolbarButtonConfig {
                                id: format!("refresh_view_{}", selected_id),
                                label: "Refresh".to_string(),
                                icon: None,
                                primary: false,
                                spacer_before: false,
                            },
                            ToolbarButtonConfig {
                                id: format!("refetch_entry_{}", selected_id),
                                label: "Refetch".to_string(),
                                icon: None,
                                primary: false,
                                spacer_before: false,
                            },
                            ToolbarButtonConfig {
                                id: format!("select_entry_{}", selected_id),
                                label: "Select for Use".to_string(),
                                icon: None,
                                primary: true,
                                spacer_before: true, // Push to right side
                            },
                        ],
                    }));
                    content_elements.push(UiElement::Separator);

                    // ===== TITLE (Hero) =====
                    // Use SectionHeader level 1 for consistent styling (20px, bold)
                    content_elements.push(UiElement::SectionHeader(SectionHeaderConfig {
                        title: title.to_string(),
                        level: 1,
                        description: None,
                    }));

                    content_elements.push(UiElement::Space(SpacingStep::Small));

                    // ===== CAROUSEL GALLERY =====
                    // Use cached image list to avoid has_data() calls on every frame
                    // HashMap persists across entry switches for instant back-navigation
                    let (current_idx, _is_cached_tab, cached_images) = STATE.with(|s| {
                        let state = s.borrow();
                        let cached = state.cached_carousel_images.get(selected_id).cloned();
                        (
                            state.current_image_index,
                            state.browser_tab == "cached",
                            cached,
                        )
                    });

                    // Use cached images or build new list
                    let carousel_images: Vec<(String, Option<String>)> = if let Some(images) =
                        cached_images
                    {
                        images
                    } else {
                        // Build image list (only runs once per entry selection)
                        let mut images: Vec<(String, Option<String>)> = Vec::new();
                        let mut seen_urls: std::collections::HashSet<String> =
                            std::collections::HashSet::new();

                        // Add cover image first
                        let cover_key =
                            gameta_lib::providers::dlsite::cache_keys::cover_key(selected_id);
                        let cover_url = scraped.as_ref().and_then(|s| s.cover_image.clone());

                        // Show cover if we have a URL or cached bytes
                        let show_cover = cover_url.is_some()
                            || wirt_sdk::wirt::plugin::host::has_data(&cover_key);

                        if show_cover {
                            if let Some(ref url) = cover_url {
                                seen_urls.insert(url.clone());
                            }
                            images.push((cover_key, cover_url));
                        }

                        // Add screenshots: from scraped data URLs, or probe cache as fallback
                        let has_scraped_screenshots = scraped
                            .as_ref()
                            .map(|s| !s.screenshots.is_empty())
                            .unwrap_or(false);

                        if has_scraped_screenshots {
                            let scraped_data = scraped.as_ref().unwrap();
                            for (i, url) in scraped_data.screenshots.iter().enumerate() {
                                if !url.is_empty() && !seen_urls.contains(url) {
                                    let key =
                                        gameta_lib::providers::dlsite::cache_keys::screenshot_key(
                                            selected_id,
                                            i,
                                        );
                                    seen_urls.insert(url.clone());
                                    images.push((key, Some(url.clone())));
                                }
                            }
                        } else {
                            // Fallback: probe content cache by key when DB extras
                            // doesn't have screenshot URLs (stale migration data)
                            for i in 0..20 {
                                let key = gameta_lib::providers::dlsite::cache_keys::screenshot_key(
                                    selected_id,
                                    i,
                                );
                                if wirt_sdk::wirt::plugin::host::has_data(&key) {
                                    images.push((key, None));
                                } else {
                                    break;
                                }
                            }
                        }

                        // Only cache non-empty lists (empty = stale, will retry next frame
                        // after lazy repair has a chance to populate extras)
                        if !images.is_empty() {
                            STATE.with(|s| {
                                s.borrow_mut()
                                    .cached_carousel_images
                                    .insert(selected_id.to_string(), images.clone());
                            });
                        }

                        images
                    };

                    if carousel_images.is_empty() {
                        // Show placeholder if no images
                        content_elements.push(UiElement::Label(LabelConfig {
                            text: "[No images available]".to_string(),
                            role: TextRole::Caption,
                        }));
                    } else {
                        // Convert current_image_index to carousel index
                        // -1 (cover) = 0 in carousel, 0 (first sample) = 1 if cover exists
                        let carousel_index = if current_idx < 0 {
                            0u32
                        } else {
                            (current_idx + 1) as u32
                        };
                        let carousel_index = carousel_index.min((carousel_images.len() - 1) as u32);

                        content_elements.push(UiElement::Carousel(CarouselConfig {
                            id: format!("gallery_{}", selected_id),
                            images: carousel_images,
                            current_index: carousel_index,
                            height: Some(SizeHint::Regular),
                            enable_lightbox: true,
                        }));
                    }

                    content_elements.push(UiElement::Space(SpacingStep::Medium));

                    // ===== METADATA INFO (Card-style Grid) =====
                    let release_str = release_date.unwrap_or("Unknown");

                    content_elements.push(UiElement::MetadataGrid(MetadataGridConfig {
                        items: vec![
                            KeyValuePair {
                                key: "Product ID".to_string(),
                                value: selected_id.to_string(),
                            },
                            KeyValuePair {
                                key: "Released".to_string(),
                                value: release_str.to_string(),
                            },
                            KeyValuePair {
                                key: "Circle".to_string(),
                                value: maker.to_string(),
                            },
                        ],
                        columns: Some(3), // Show all 3 on one row
                    }));

                    content_elements.push(UiElement::Separator);

                    // ===== TAGS =====
                    let tags: Vec<String> = scraped
                        .as_ref()
                        .map(|s| s.tags.clone())
                        .filter(|t| !t.is_empty())
                        .or_else(|| {
                            json["tags"].as_array().map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                        })
                        .unwrap_or_default();

                    if !tags.is_empty() {
                        content_elements.push(UiElement::Label(LabelConfig {
                            text: "Tags".to_string(),
                            role: TextRole::Subtitle,
                        }));
                        content_elements.push(UiElement::TagChips(TagChipsConfig {
                            tags: tags.clone(),
                            max_display: Some(15),
                        }));
                        content_elements.push(UiElement::Space(SpacingStep::Medium));
                    }

                    // ===== DESCRIPTION =====
                    let description = scraped
                        .as_ref()
                        .and_then(|s| s.description.clone())
                        .or_else(|| json["description"].as_str().map(|s| s.to_string()));

                    if let Some(desc) = description {
                        content_elements.push(UiElement::Label(LabelConfig {
                            text: "Description".to_string(),
                            role: TextRole::Subtitle,
                        }));
                        // Truncate long descriptions for readability (respecting UTF-8 char boundaries)
                        // Format description to restore structure
                        let formatted = format_description(&desc);
                        // Truncate if excessively long (2000 chars)
                        let desc_display = if formatted.len() > 2000 {
                            let truncate_at = formatted
                                .char_indices()
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
                            role: TextRole::Body,
                        }));
                    }

                    content_elements.push(UiElement::Separator);

                    // ===== SAMPLE IMAGES (Gallery) =====
                    if let Some(samples) = json["sample_images"].as_array() {
                        if !samples.is_empty() {
                            content_elements.push(UiElement::Label(LabelConfig {
                                text: "Sample Images".to_string(),
                                role: TextRole::Subtitle,
                            }));
                            content_elements.push(UiElement::Space(SpacingStep::Small));

                            // Show up to 3 samples
                            for (i, sample) in samples.iter().take(3).enumerate() {
                                if let Some(url) = sample.as_str() {
                                    content_elements.push(UiElement::Image(ImageConfig {
                                        cache_key: Some(
                                            gameta_lib::providers::dlsite::cache_keys::screenshot_key(
                                                selected_id,
                                                i,
                                            ),
                                        ),
                                        url: Some(url.to_string()),
                                        height: Some(SizeHint::Regular),
                                    }));
                                    content_elements.push(UiElement::Space(SpacingStep::Small));
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
                content_elements.push(UiElement::Space(SpacingStep::Large));
                content_elements.push(UiElement::Label(LabelConfig {
                    text: "Select an item to view details".to_string(),
                    role: TextRole::Title,
                }));
            }

            PluginLayout::Split(SplitConfig {
                sidebar: sidebar_elements,
                content: content_elements,
                width: Some(SidebarWidth::Wide),
            })
        }

        _ => PluginLayout::Single(vec![]),
    }
}
