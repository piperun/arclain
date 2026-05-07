use archust_plugin_sdk::info;
use std::cell::RefCell;

mod views;

// Plugin state to store found metadata.
//
// Fields are `pub(crate)` so the sibling `views` / `events` modules
// can read/write through `STATE.with(|s| s.borrow().…)`. The struct
// itself is `pub(crate)` for the same reason; it never escapes the
// crate (this is a `cdylib` plugin, no public API).
pub(crate) struct PluginState {
    pub(crate) found_metadata: Option<(String, serde_json::Value, Option<ScrapedData>)>, // (product_id, json, scraped)
    pub(crate) search_query: String,
    pub(crate) search_results: Vec<(String, String, String, Option<String>)>, // (code, title, maker, thumbnail_url)
    pub(crate) auto_fetch_enabled: bool, // Master switch: auto-fetch when archive opens
    pub(crate) enable_cache: bool, // Sub-option: cache fetched results (only relevant if auto_fetch enabled)
    pub(crate) cache_images: bool, // Cache cover and screenshot images
    pub(crate) cache_videos: bool, // Download chobit-embed videos referenced from the description
    /// Video quality preference: "best", "low" (lowest available), or a
    /// numeric resolution like "720" / "480". Defaults to "best".
    pub(crate) video_quality: String,
    pub(crate) dump_html_debug: bool, // Debug: dump HTML to file when geo-blocked detected
    pub(crate) fetch_in_progress: bool, // Prevent double-fetch when spamming buttons
    pub(crate) cached_entries: Option<Vec<String>>, // Cache of checking the cache (UI spam prevention)
    pub(crate) selected_cache_entry: Option<String>, // For cache viewer details
    pub(crate) last_archive_path: Option<String>, // Track current archive to reset state on change
    // Browser UI state
    pub(crate) browser_tab: String, // "cached" or "search"
    pub(crate) browser_loading: bool,
    // Cache for browser detail view to prevent fetch loop
    pub(crate) browser_detail_cache: Option<(String, serde_json::Value, Option<ScrapedData>)>,
    pub(crate) current_image_index: i32,
    // Rename archive option when selecting from search
    pub(crate) rename_with_code: bool,
    // Cached carousel images to avoid has_data() calls every frame
    // HashMap<product_id, images> - persists across entry switches
    pub(crate) cached_carousel_images: std::collections::HashMap<String, Vec<(String, Option<String>)>>,
    // Cached metadata summaries to avoid DB queries every frame
    // (filtered_ids, summaries) - rebuilt when filter changes
    pub(crate) cached_summaries: Option<(Vec<String>, Vec<(String, Option<String>, bool)>)>,
}

// Global state (thread-local for WASM component)
thread_local! {
    pub(crate) static STATE: RefCell<PluginState> = RefCell::new(PluginState {
        found_metadata: None,
        search_query: String::new(),
        search_results: Vec::new(),
        auto_fetch_enabled: true,
        enable_cache: true,
        cache_images: true,
        cache_videos: false, // Videos are big — opt-in
        video_quality: "best".to_string(),
        dump_html_debug: true, // Default to enabled for debugging
        fetch_in_progress: false,
        last_archive_path: None,
        cached_entries: None,
        selected_cache_entry: None,
        browser_tab: "cached".to_string(),
        browser_loading: false,
        browser_detail_cache: None,
        current_image_index: -1, // -1 = Cover, 0+ = Sample index
        rename_with_code: false, // Default: don't rename
        cached_carousel_images: std::collections::HashMap::new(),
        cached_summaries: None,
    });
}

struct Component;

pub(crate) fn format_description(text: &str) -> String {
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

/// Sanitize a string for use as a filename
pub(crate) fn sanitize_filename(name: &str) -> String {
    // Remove characters that are invalid in filenames
    let invalid_chars = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    let mut result: String = name
        .chars()
        .map(|c| if invalid_chars.contains(&c) { '_' } else { c })
        .collect();

    // Trim whitespace and limit length
    result = result.trim().to_string();
    if result.len() > 200 {
        result.truncate(200);
    }

    result
}

/// Get total image count (cover + screenshots) from current browser detail cache
pub(crate) fn get_total_image_count(state: &PluginState) -> usize {
    if let Some((product_id, _json, scraped)) = &state.browser_detail_cache {
        let cover_url = scraped.as_ref().and_then(|s| s.cover_image.clone());
        let has_cover = cover_url.is_some()
            || archust_plugin_sdk::arclain::plugin::host::has_data(
                &gameta_lib::providers::dlsite::cache_keys::cover_key(product_id)
            );
        // Count non-empty, non-duplicate screenshot URLs
        let sample_count = scraped.as_ref()
            .map(|s| {
                s.screenshots.iter()
                    .filter(|url| !url.is_empty())
                    .filter(|url| cover_url.as_ref() != Some(*url)) // Skip if same as cover
                    .count()
            })
            .unwrap_or(0);
        (if has_cover { 1 } else { 0 }) + sample_count
    } else {
        0
    }
}

impl archust_plugin_sdk::Guest for Component {
    fn get_metadata() -> archust_plugin_sdk::arclain::plugin::meta::PluginMetadata {
        // Pre-2026-05-07 the host's `runtime::get_metadata` returned a
        // hardcoded "Unknown Plugin" placeholder because the WIT had
        // no such export. install_plugin therefore couldn't derive a
        // stable id from a bare .wasm. Now it asks the plugin
        // directly. Values mirror dlsite-metadata.toml.
        archust_plugin_sdk::arclain::plugin::meta::PluginMetadata {
            id: "dlsite-metadata".to_string(),
            name: "DLSite Metadata".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "Archust Team".to_string(),
            description:
                "Extracts DLSite product codes (RJ/VJ/BJ) from archive names and enriches them \
                 with metadata from DLSite API"
                    .to_string(),
        }
    }

    fn init() {
        info("DLSite Metadata plugin initialized");
        
        // Read plugin settings
        let auto_fetch = archust_plugin_sdk::arclain::plugin::host::get_setting("auto_fetch_enabled")
            .unwrap_or_else(|| "true".to_string()) == "true";
        let enable_cache = archust_plugin_sdk::arclain::plugin::host::get_setting("enable_cache")
            .unwrap_or_else(|| "true".to_string()) == "true";
        let cache_images = archust_plugin_sdk::arclain::plugin::host::get_setting("cache_images")
            .unwrap_or_else(|| "true".to_string()) == "true";
        let cache_videos = archust_plugin_sdk::arclain::plugin::host::get_setting("cache_videos")
            .unwrap_or_else(|| "false".to_string()) == "true";
        let video_quality = archust_plugin_sdk::arclain::plugin::host::get_setting("video_quality")
            .unwrap_or_else(|| "best".to_string());

        STATE.with(|state| {
            let mut s = state.borrow_mut();
            s.auto_fetch_enabled = auto_fetch;
            s.enable_cache = enable_cache;
            s.cache_images = cache_images;
            s.cache_videos = cache_videos;
            s.video_quality = video_quality;
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
        views::dispatch(&extension_point)
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
        use archust_plugin_sdk::arclain::plugin::ui::{PluginAction, OpenLightboxConfig};

        // Handle page initialization - set display name for breadcrumb
        if id == "__page_init" {
            if let Some(page_id) = &value {
                if page_id.starts_with("dlsite_browser") {
                    return vec![PluginAction::SetPageDisplayName("DLSite Browser".to_string())];
                }
            }
            return vec![];
        }

        // Handle system events dispatched as UI events
        if id == "event:archive_opened" {
            let path = value.unwrap_or_default();
            info(&format!("[DLSite Plugin] Archive opened event: {}", path));

            let auto_fetch = STATE.with(|s| s.borrow().auto_fetch_enabled);
            if auto_fetch {
                // Only detect code (instant regex) — no cache lookup, no emit, no DB.
                // Return RequestFetch so the host handles everything asynchronously
                // without holding the plugin mutex.
                if let Some(code) = detect_code_from_archive() {
                    info(&format!("[DLSite Plugin] Detected code: {}", code));
                    return vec![PluginAction::RequestFetch(format!("dlsite:{}", code))];
                }
                info("[DLSite Plugin] No DLSite code detected");
            }
            return vec![];
        }

        // Background-fetch completion events from the host. The host emits
        // these whenever a `RequestFetch` action finishes (success OR failure)
        // so the plugin can clear its in-progress flag and re-check the cache
        // for newly-arrived data. Without these, the plugin hangs in the
        // "Fetch already in progress, ignoring" state until the archive is
        // reopened.
        if let Some(key) = id.strip_prefix("background_fetch_complete:") {
            info(&format!("[DLSite Plugin] Background fetch complete: {}", key));
            STATE.with(|s| s.borrow_mut().fetch_in_progress = false);
            if let Some(code) = key.strip_prefix("dlsite:") {
                if let Some((json, scraped)) = get_cached_dlsite_metadata(code) {
                    info(&format!("[DLSite Plugin] Loaded {} from cache after fetch", code));
                    let metadata_json =
                        generate_metadata_json(code, Some(&(json.clone(), scraped.clone())));
                    archust_plugin_sdk::emit_metadata(&metadata_json);
                    STATE.with(|s| {
                        s.borrow_mut().found_metadata =
                            Some((code.to_string(), json, scraped));
                    });
                } else {
                    info(&format!(
                        "[DLSite Plugin] Fetch reported done but {} not in cache",
                        code
                    ));
                }
            }
            return vec![];
        }
        if let Some(key) = id.strip_prefix("background_fetch_failed:") {
            info(&format!("[DLSite Plugin] Background fetch FAILED: {}", key));
            STATE.with(|s| s.borrow_mut().fetch_in_progress = false);
            return vec![];
        }

        // "Play in external player" button — the panel emits
        // `play_video:<cache_key>` when the user clicks. Hand off
        // the cache key to the host, which writes the bytes to a
        // temp file and shells out to the OS default app for `.mp4`
        // (mpv / VLC / etc.). Bytes never enter the WASM heap; the
        // plugin doesn't even read them.
        if let Some(cache_key) = id.strip_prefix("play_video:") {
            info(&format!("[DLSite Plugin] Play requested for {}", cache_key));
            // chobit always serves mp4; if that ever stops being
            // true we'd need to track per-key extension at fetch
            // time. For now, mp4 is the right guess.
            if let Err(e) = archust_plugin_sdk::play_cached_blob(cache_key, "mp4") {
                archust_plugin_sdk::log_network_activity(&format!(
                    "Failed to play {}: {}",
                    cache_key, e,
                ));
                archust_plugin_sdk::show_message(
                    "Playback failed",
                    &format!("Could not launch external player for the cached video:\n\n{}", e),
                );
            }
            return vec![];
        }

        // The host asks the plugin to perform its own DLsite fetch when no
        // gameta server is available (or the gameta fetch returned nothing).
        // We hold the plugin lock for the duration of the HTTP call, but
        // since this only fires when the user has explicitly clicked Fetch
        // or auto-fetch is enabled, the cost is contained to occasional
        // archive opens / button clicks rather than every UI frame.
        if let Some(key) = id.strip_prefix("do_native_fetch:") {
            info(&format!("[DLSite Plugin] Native fetch requested for {}", key));
            let code = key.strip_prefix("dlsite:").unwrap_or(key).to_string();

            match fetch_dlsite_metadata(&code) {
                Some((json, scraped)) => {
                    info(&format!(
                        "[DLSite Plugin] Native fetch succeeded for {}",
                        code
                    ));
                    let metadata_json =
                        generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
                    archust_plugin_sdk::emit_metadata(&metadata_json);
                    STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        state.found_metadata = Some((code.clone(), json, scraped));
                        state.fetch_in_progress = false;
                    });
                }
                None => {
                    info(&format!(
                        "[DLSite Plugin] Native fetch returned no metadata for {}",
                        code
                    ));
                    STATE.with(|s| s.borrow_mut().fetch_in_progress = false);
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

            // Check if we should rename the archive
            let should_rename = STATE.with(|s| s.borrow().rename_with_code);

            STATE.with(|state| {
                let mut s = state.borrow_mut();
                s.search_results.clear();
                s.search_query.clear();
            });

            // Re-use logic to fetch and emit (Data API caches transparently)
            if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
                let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
                archust_plugin_sdk::emit_metadata(&metadata_json);

                // Download images + videos with progress
                if let Some(ref s) = scraped {
                    fetch_images_with_progress(&code, s);
                    fetch_videos_with_progress(&code, s);
                }

                // Rename archive if option is checked
                if should_rename {
                    if let Some(archive_info) = archust_plugin_sdk::current_archive_info() {
                        // Get title from metadata
                        let title = json["work_name"].as_str()
                            .or_else(|| json["title"].as_str())
                            .map(|s| sanitize_filename(s));

                        // Get current extension
                        let current_name = &archive_info.filename;
                        let extension = current_name.rsplit('.').next().unwrap_or("7z");

                        // Build new filename: [CODE] Title.ext or [CODE] original.ext
                        let new_name = match title {
                            Some(t) if !t.is_empty() => format!("[{}] {}.{}", code, t, extension),
                            _ => format!("[{}] {}", code, current_name),
                        };

                        info(&format!("[DLSite Plugin] Renaming archive to: {}", new_name));
                        match archust_plugin_sdk::rename_archive(&new_name) {
                            Ok(new_path) => {
                                info(&format!("[DLSite Plugin] Archive renamed successfully to: {}", new_path));
                            }
                            Err(e) => {
                                info(&format!("[DLSite Plugin] Failed to rename archive: {}", e));
                            }
                        }
                    }
                }

                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.found_metadata = Some((code.clone(), json, scraped));
                });
            }

            // Dialog will be closed when user selects a result
            return vec![archust_plugin_sdk::arclain::plugin::ui::PluginAction::CloseDialog];
        }

        // Carousel Gallery Navigation
        if id.starts_with("gallery_") {
            // Extract the product_id from the event ID (format: gallery_{product_id}_{action})
            let parts: Vec<&str> = id.splitn(3, '_').collect();
            if parts.len() >= 3 {
                let action_part = parts[2];

                if action_part == "prev" {
                    // Navigate to previous image (wraps around)
                    STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        let total_images = get_total_image_count(&state);
                        if total_images > 1 {
                            // current_image_index: -1 = cover (carousel 0), 0+ = sample (carousel 1+)
                            // Going prev from cover (-1) wraps to last sample
                            let mut new_idx = state.current_image_index - 1;
                            if new_idx < -1 {
                                new_idx = (total_images - 2) as i32; // -2 because: total includes cover, and samples are 0-indexed
                            }
                            state.current_image_index = new_idx;
                        }
                    });
                    return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
                }

                if action_part == "next" {
                    // Navigate to next image (wraps around)
                    STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        let total_images = get_total_image_count(&state);
                        if total_images > 1 {
                            let mut new_idx = state.current_image_index + 1;
                            // If we go past the last sample, wrap to cover (-1)
                            if new_idx >= (total_images - 1) as i32 {
                                new_idx = -1;
                            }
                            state.current_image_index = new_idx;
                        }
                    });
                    return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
                }

                if action_part.starts_with("select_") {
                    // Direct select by carousel index
                    if let Ok(carousel_idx) = action_part.trim_start_matches("select_").parse::<usize>() {
                        STATE.with(|s| {
                            let mut state = s.borrow_mut();
                            // Carousel index 0 = cover (-1), 1+ = sample (carousel_idx - 1)
                            state.current_image_index = if carousel_idx == 0 { -1 } else { (carousel_idx - 1) as i32 };
                        });
                        return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
                    }
                }

                if action_part == "open_lightbox" {
                    // Open lightbox with all images (respecting cached tab filter)
                    return STATE.with(|s| {
                        let state = s.borrow();
                        let is_cached_tab = state.browser_tab == "cached";

                        if let Some((product_id, _json, scraped)) = &state.browser_detail_cache {
                            let mut images: Vec<(String, Option<String>)> = Vec::new();
                            let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();

                            // Add cover (only if cached when on cached tab)
                            let cover_key = gameta_lib::providers::dlsite::cache_keys::cover_key(product_id);
                            let cover_url = scraped.as_ref().and_then(|s| s.cover_image.clone());
                            let cover_is_cached = archust_plugin_sdk::arclain::plugin::host::has_data(&cover_key);

                            let show_cover = if is_cached_tab { cover_is_cached } else { cover_url.is_some() || cover_is_cached };

                            if show_cover {
                                if let Some(ref url) = cover_url {
                                    seen_urls.insert(url.clone());
                                }
                                images.push((cover_key, cover_url));
                            }

                            // Add screenshots (only cached ones when on cached tab)
                            if let Some(scraped_data) = scraped {
                                for (i, url) in scraped_data.screenshots.iter().enumerate() {
                                    if !url.is_empty() && !seen_urls.contains(url) {
                                        let key = gameta_lib::providers::dlsite::cache_keys::screenshot_key(product_id, i);
                                        let is_cached = archust_plugin_sdk::arclain::plugin::host::has_data(&key);

                                        let should_include = if is_cached_tab { is_cached } else { true };

                                        if should_include {
                                            seen_urls.insert(url.clone());
                                            images.push((key, Some(url.clone())));
                                        }
                                    }
                                }
                            }

                            // Convert current_image_index to carousel start index
                            let start_idx = if state.current_image_index < 0 { 0u32 } else { (state.current_image_index + 1) as u32 };
                            let start_idx = start_idx.min((images.len().saturating_sub(1)) as u32);

                            vec![PluginAction::OpenLightbox(OpenLightboxConfig {
                                images,
                                start_index: start_idx,
                                title: scraped.as_ref().and_then(|s| s.title.clone()),
                            })]
                        } else {
                            vec![]
                        }
                    });
                }
            }
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
            "rename_with_code" => {
                if let Some(val) = value {
                    let enabled = val == "true";
                    STATE.with(|state| {
                        state.borrow_mut().rename_with_code = enabled;
                    });
                    info(&format!("[DLSite Plugin] Rename with code setting changed to: {}", enabled));
                }
            }
            "dump_html_debug" => {
                if let Some(val) = value {
                    let enabled = val == "true";
                    STATE.with(|state| {
                        state.borrow_mut().dump_html_debug = enabled;
                    });
                    // We don't persist this one to disk as it's a temp debug setting, or we could if needed
                    info(&format!("[DLSite Plugin] Dump HTML debug setting changed to: {}", enabled));
                }
            }
            "cache_videos" => {
                if let Some(val) = value {
                    let enabled = val == "true";
                    STATE.with(|state| {
                        state.borrow_mut().cache_videos = enabled;
                    });
                    archust_plugin_sdk::arclain::plugin::host::set_setting("cache_videos", &val);
                    info(&format!("[DLSite Plugin] Cache videos setting changed to: {}", enabled));
                }
            }
            "video_quality" => {
                if let Some(val) = value {
                    STATE.with(|state| {
                        state.borrow_mut().video_quality = val.clone();
                    });
                    archust_plugin_sdk::arclain::plugin::host::set_setting("video_quality", &val);
                    info(&format!("[DLSite Plugin] Video quality setting changed to: {}", val));
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
            "do_clear_all_cache" => {
                info("[DLSite Plugin] Clearing ALL cache...");
                archust_plugin_sdk::invalidate_cache("dlsite:*");
                archust_plugin_sdk::show_message("Cache Cleared", "All DLSite cache entries have been removed.");
                // Dialog closing is handled by the button's ButtonAction::CloseDialog
                return vec![]; 
            }
            "prune_cache" => {
                 use archust_plugin_sdk::{list_cached_entries, invalidate_cache};
                 use archust_plugin_sdk::arclain::plugin::host::get_data;
                 
                 info("[DLSite Plugin] Starting cache prune...");
                 let entries = list_cached_entries().unwrap_or_default();
                 let mut scanned = 0;
                 let mut removed = 0;
                 
                 for key in entries {
                     if !key.starts_with("dlsite:") { continue; }
                     scanned += 1;
                     
                     // Read direct from host (no network)
                     if let Some(data_bytes) = get_data(&key) {
                        let data_str = String::from_utf8_lossy(&data_bytes);

                         // Check based on key type
                         let is_valid = if key.contains(":json:") {
                             gameta_lib::providers::dlsite::parse_api_response("", &data_str).is_ok()
                         } else if key.contains(":html:") {
                             gameta_lib::providers::dlsite::parse_html_response(&data_str).is_some()
                         } else {
                             true // Skip other types (images etc)
                         };
                         
                         if !is_valid {
                             info(&format!("[DLSite Plugin] Pruning invalid entry: {}", key));
                             if invalidate_cache(&key) { removed += 1; }
                         }
                     }
                 }
                 
                 info(&format!("[DLSite Plugin] Prune finished. Scanned {}, Removed {}.", scanned, removed));
                 archust_plugin_sdk::show_message("Prune Complete", &format!("Scanned {} DLSite entries.\nRemoved {} invalid/corrupted items.", scanned, removed));
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

                info("[DLSite Plugin] Handling fetch_metadata");

                // Fast path: check cache first (instant, no network)
                match perform_scan_cached_only() {
                    Ok(Some((product_id, json, scraped))) => {
                        info("[DLSite Plugin] Found cached metadata");
                        let metadata_json = generate_metadata_json(&product_id, Some(&(json.clone(), scraped.clone())));
                        archust_plugin_sdk::emit_metadata(&metadata_json);

                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.found_metadata = Some((product_id, json, scraped));
                        });
                        return vec![];
                    }
                    Ok(None) => {
                        // Not cached — detect code and request async fetch from host
                        info("[DLSite Plugin] Not cached, requesting background fetch");

                        let code = detect_code_from_archive();
                        if let Some(code) = code {
                            STATE.with(|state| {
                                state.borrow_mut().fetch_in_progress = true;
                            });
                            archust_plugin_sdk::arclain::plugin::host::set_status_message(
                                &format!("Fetching metadata for {}...", code),
                            );
                            return vec![PluginAction::RequestFetch(format!("dlsite:{}", code))];
                        }
                        info("[DLSite Plugin] No DLSite code detected");
                        return vec![];
                    }
                    Err(e) => {
                        info(&format!("[DLSite Plugin] Scan failed: {}", e));
                        return vec![];
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
                    let mut s = state.borrow_mut();
                    s.cached_entries = None; // clear to force refetch
                    s.cached_summaries = None;
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
                 // (host side does lazy repair of stale extras on read)
                 let cached_data = get_cached_dlsite_metadata(&entry_id);

                 STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    if let Some((json, scraped)) = cached_data {
                        s.browser_detail_cache = Some((entry_id.clone(), json, scraped));
                    }
                    // Clear carousel cache so it rebuilds with potentially repaired data
                    s.cached_carousel_images.remove(&entry_id);
                    s.selected_cache_entry = Some(entry_id);
                    s.current_image_index = -1; // Reset to cover when switching entries
                    // Note: Keep cached_carousel_images HashMap intact for instant back-navigation
                });

                // Refresh panel to show the new selection and its details
                return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
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
                        s.browser_tab = if tab == "Search" { "search".to_string() } else { "cached".to_string() };
                        s.selected_cache_entry = None; // Clear selection on tab switch
                        s.search_results.clear(); // Clear search results on tab switch
                        s.cached_carousel_images.clear(); // Clear all cached images - filter logic differs per tab
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
                    // Check if query is a direct product code (RJ/VJ/BJ followed by digits)
                    let trimmed = query.trim().to_uppercase();
                    let is_product_code = (trimmed.starts_with("RJ") || trimmed.starts_with("VJ") || trimmed.starts_with("BJ"))
                        && trimmed.len() >= 4
                        && trimmed[2..].chars().all(|c| c.is_ascii_digit());

                    if is_product_code {
                        // Direct lookup - skip search and go straight to details
                        info(&format!("[DLSite Plugin] Direct lookup for product code: {}", trimmed));

                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.selected_cache_entry = Some(trimmed.clone());
                            s.current_image_index = -1;
                            s.browser_loading = true;
                            // Note: Keep cached_carousel_images HashMap intact for instant back-navigation
                        });

                        // Try cache first, then network
                        let (json, scraped) = if let Some(cached) = get_cached_dlsite_metadata(&trimmed) {
                            info(&format!("[DLSite Plugin] Found {} in cache", trimmed));
                            cached
                        } else {
                            info(&format!("[DLSite Plugin] Fetching {} from network", trimmed));
                            match fetch_dlsite_metadata(&trimmed) {
                                Some(data) => data,
                                None => {
                                    STATE.with(|s| s.borrow_mut().browser_loading = false);
                                    archust_plugin_sdk::show_message("Not Found", &format!("Could not find product: {}", trimmed));
                                    return vec![];
                                }
                            }
                        };

                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.browser_detail_cache = Some((trimmed.clone(), json, scraped));
                            s.browser_loading = false;
                        });

                        return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
                    }

                    // Regular search
                    STATE.with(|s| s.borrow_mut().browser_loading = true);

                    // Perform search - just display results, don't auto-cache
                    // User must explicitly click on an entry to fetch and cache it
                    let results = search_dlsite(&query);

                    STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        state.search_results = results;
                        state.browser_loading = false;
                    });
                }
            }
            "copy_archive_filename" => {
                use archust_plugin_sdk::arclain::plugin::ui::{ToastConfig, ToastLevel};
                if let Some(archive_info) = archust_plugin_sdk::current_archive_info() {
                    // Copy the filename without extension to clipboard
                    let filename = &archive_info.filename;
                    // Remove extension for easier searching
                    let name_without_ext = filename.rsplit('.').skip(1).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(".");
                    let copy_name = if name_without_ext.is_empty() { filename.clone() } else { name_without_ext };
                    return vec![
                        PluginAction::CopyToClipboard(copy_name),
                        PluginAction::ShowToast(ToastConfig {
                            message: "Filename copied to clipboard".to_string(),
                            level: ToastLevel::Success,
                        }),
                    ];
                }
            }
            // List item selection (from ListContainer)
            id if id.starts_with("RJ") || id.starts_with("VJ") || id.starts_with("BJ") => {
                info(&format!("[DLSite Plugin] Selected item: {}", id));

                // Set the selected entry and reset carousel index
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.selected_cache_entry = Some(id.to_string());
                    s.current_image_index = -1; // Reset to cover when switching entries
                });

                // Try to load from cache first, otherwise fetch from network
                if let Some((json, scraped)) = get_cached_dlsite_metadata(id) {
                    info(&format!("[DLSite Plugin] Loaded {} from cache", id));
                    STATE.with(|state| {
                        state.borrow_mut().browser_detail_cache = Some((id.to_string(), json, scraped));
                    });
                } else {
                    info(&format!("[DLSite Plugin] Fetching {} from network", id));
                    if let Some((json, scraped)) = fetch_dlsite_metadata(id) {
                        STATE.with(|state| {
                            state.borrow_mut().browser_detail_cache = Some((id.to_string(), json, scraped));
                        });
                    }
                }

                // Refresh to show details
                return vec![PluginAction::RefreshPanel("Page:dlsite_browser".to_string())];
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
                let json_key = gameta_lib::providers::dlsite::cache_keys::json_key(&entry_id);
                let html_key = gameta_lib::providers::dlsite::cache_keys::html_key(&entry_id);
                
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
                        // Emit metadata immediately
                        let metadata_json = generate_metadata_json(&entry_id, Some(&(json.clone(), scraped.clone())));
                        archust_plugin_sdk::emit_metadata(&metadata_json);

                        // Download images + videos with progress
                        if let Some(ref s) = scraped {
                            fetch_images_with_progress(&entry_id, s);
                            fetch_videos_with_progress(&entry_id, s);
                        }

                        STATE.with(|state| {
                            let mut s = state.borrow_mut();
                            s.browser_detail_cache = Some((entry_id.clone(), json, scraped));
                            s.cached_summaries = None;
                            s.cached_carousel_images.remove(&entry_id);
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

/// Detect a DLSite code from the current archive filename.
/// Only checks the filename — never re-lists archive contents (which spawns
/// a 7z subprocess and takes 5+ seconds).
pub(crate) fn detect_code_from_archive() -> Option<String> {
    let info_data = archust_plugin_sdk::current_archive_info()?;
    detect_dlsite_code(&info_data.filename)
}

/// Fast scan: detect DLSite code + check cache only. Never hits the network.
/// Used during archive_opened to avoid blocking the UI.
pub(crate) fn perform_scan_cached_only() -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
    use archust_plugin_sdk::{current_archive_info, info, list_archive_files};

    let info_data = current_archive_info().ok_or("No archive open")?;
    info(&format!(
        "[DLSite Plugin] Scanning archive: {}",
        info_data.filename
    ));

    let mut checked_codes: Vec<String> = Vec::new();

    let mut check_cached = |code: String| -> Option<(String, serde_json::Value, Option<ScrapedData>)> {
        if checked_codes.contains(&code) {
            return None;
        }
        checked_codes.push(code.clone());
        info(&format!("[DLSite Plugin] Found code: {}", code));

        if let Some((json, scraped)) = get_cached_dlsite_metadata(&code) {
            info(&format!("[DLSite Plugin] Using cached metadata for {}", code));
            return Some((code, json, scraped));
        }
        info(&format!("[DLSite Plugin] {} not cached, skipping network fetch", code));
        None
    };

    // 1. Check filename
    if let Some(code) = detect_dlsite_code(&info_data.filename) {
        if let Some(result) = check_cached(code) {
            return Ok(Some(result));
        }
    }

    // 2. Check archive contents
    if let Ok(files) = list_archive_files() {
        for file in files {
            if let Some(code) = detect_dlsite_code(&file) {
                if let Some(result) = check_cached(code) {
                    return Ok(Some(result));
                }
            }
        }
    }

    Ok(None)
}

/// Full scan: detect code + check cache + fetch from network if needed.
/// Used for explicit "Fetch DLSite" button clicks (user-initiated).
pub(crate) fn perform_scan() -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
    use archust_plugin_sdk::{current_archive_info, info, list_archive_files};

    let info_data = current_archive_info().ok_or("No archive open")?;
    info(&format!(
        "[DLSite Plugin] Scanning archive: {}",
        info_data.filename
    ));

    // Track checked codes to prevent redundant fetches/checks in this scan session
    let mut checked_codes: Vec<String> = Vec::new();

    // Helper to process found code
    // Check cache first, then fall back to network if not cached
    let mut process_code = |code: String| -> Result<Option<(String, serde_json::Value, Option<ScrapedData>)>, String> {
        // Skip if already checked in this scan
        if checked_codes.contains(&code) {
            return Ok(None);
        }
        checked_codes.push(code.clone());

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
            // Emit metadata immediately so the UI gets it
            let metadata_json = generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
            archust_plugin_sdk::emit_metadata(&metadata_json);

            // Then download images + videos with progress (non-blocking for metadata signal)
            if let Some(ref s) = scraped {
                fetch_images_with_progress(&code, s);
                fetch_videos_with_progress(&code, s);
            }

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

/// Detect DLSite code using gameta_lib (same path as `arclain_core::utilities::detect_dlsite_code`).
pub(crate) fn detect_dlsite_code(text: &str) -> Option<String> {
    gameta_lib::detect::detect_dlsite_code(text)
}

/// Read metadata from local cache - uses host's get_product_metadata which handles:
/// 1. metadata.sqlite (instant - already parsed)
/// 2. JSON cache (host parses + saves to DB)
/// 3. HTML cache (host parses + saves to DB)
/// No WASM-side parsing - all heavy lifting done by host.
pub(crate) fn get_cached_dlsite_metadata(product_id: &str) -> Option<(serde_json::Value, Option<ScrapedData>)> {
    use archust_plugin_sdk::get_product_metadata;

    // Get ProductMetadata from host (handles all parsing on host side)
    let meta_json_str = get_product_metadata(product_id, "dlsite")?;

    // Parse the ProductMetadata JSON (gameta format)
    let meta: serde_json::Value = serde_json::from_str(&meta_json_str).ok()?;

    // Gameta stores platform-specific data in "extras" as a JSON object (not a string)
    let extras = &meta["extras"];

    // Helper to extract a string array from a JSON value
    fn str_array(val: &serde_json::Value) -> Vec<String> {
        val.as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default()
    }

    // Reconstruct ScrapedData from gameta ProductMetadata
    let scraped = ScrapedData {
        title: meta["title"].as_str().map(|s| s.to_string()),
        circle: meta["creator"].as_str().map(|s| s.to_string()),
        release_date: meta["release_date"].as_str().map(|s| s.to_string()),
        update_date: extras["update_date"].as_str().map(|s| s.to_string()),
        tags: str_array(&meta["tags"]),
        description: meta["description"].as_str().map(|s| s.to_string()),
        cover_image: extras["cover_image"].as_str().map(|s| s.to_string()),
        screenshots: str_array(&extras["screenshots"]),
        voice_actors: str_array(&extras["voice_actors"]),
        authors: str_array(&extras["authors"]),
        illustrators: str_array(&extras["illustrators"]),
        scenarios: str_array(&extras["scenarios"]),
        musicians: str_array(&extras["musicians"]),
        writers: str_array(&extras["writers"]),
        brand: extras["brand"].as_str().map(|s| s.to_string()),
        publisher: extras["publisher"].as_str().map(|s| s.to_string()),
        series: extras["series"].as_str().map(|s| s.to_string()),
        page_count: extras["page_count"].as_i64(),
        file_size: meta["file_size"].as_str().map(|s| s.to_string()),
        genres: str_array(&meta["genres"]),
        geo_blocked: meta["geo_blocked"].as_bool().unwrap_or(false),
        description_images: str_array(&extras["description_images"]),
        description_structure: Vec::new(),
    };

    // Build backward-compatible JSON for the plugin's legacy code paths
    let json_data = serde_json::json!({
        "work_name": scraped.title,
        "maker_name": scraped.circle,
        "regist_date": scraped.release_date,
        "update_date": scraped.update_date,
        "intro_s": scraped.description,
        "source": "metadata_db"
    });

    Some((json_data, Some(scraped)))
}

/// Fetch metadata from DLSite network (for new entries or search results)
/// Uses gameta_lib orchestrator for logic.
pub(crate) fn fetch_dlsite_metadata(product_id: &str) -> Option<(serde_json::Value, Option<ScrapedData>)> {
    use archust_plugin_sdk::{fetch_string_blocking, log_network_activity};
    use gameta_lib::providers::dlsite::{DlsiteFetchOptions, plan_fetch, FetchStep, parse_html, parse_api_json};
    
    // Use the orchestrator to plan our fetch
    // We request ALL sources (API + HTML)
    let options = DlsiteFetchOptions::ALL;
    let plan = plan_fetch(product_id, options);
    
    let mut json_data = serde_json::Value::Null;
    let mut scraped_data = None;
    let mut fetch_success = false;

    for step in plan {
        match step {
            FetchStep::FetchJson(url) => {
                let cache_key = gameta_lib::providers::dlsite::cache_keys::json_key(product_id);
                log_network_activity(&format!("Fetching API: {}", url));
                
                match fetch_string_blocking(&cache_key, &url) {
                    Ok(body) => {
                        info(&format!("[DEBUG] JSON fetched: {} bytes", body.len()));
                        log_network_activity(&format!("JSON response ({} bytes): {}...", body.len(), body.chars().take(100).collect::<String>()));
                        if let Ok(_meta) = parse_api_json(product_id, &body) {
                            info("[DEBUG] JSON parsed successfully");
                            // Store the RAW JSON value for the plugin's (legacy) usage
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
                                 if let Some(arr) = val.as_array() {
                                     json_data = arr.first().cloned().unwrap_or(val);
                                 } else {
                                     json_data = val;
                                 }
                                 fetch_success = true;
                                 info("[DEBUG] fetch_success = true (JSON)");
                            } else {
                                // Invalid JSON structure
                                info("[DEBUG] Failed to parse JSON structure");
                                log_network_activity("Failed to parse JSON structure. Invalidating cache.");
                                archust_plugin_sdk::invalidate_cache(&cache_key);
                            }
                        } else {
                            // Parse failed (empty or invalid API response)
                            info("[DEBUG] parse_api_response failed");
                            log_network_activity("Failed to parse API response. Invalidating cache.");
                            archust_plugin_sdk::invalidate_cache(&cache_key);
                        }
                    }
                    Err(e) => {
                        info(&format!("[DEBUG] fetch_string_blocking for JSON FAILED: {}", e));
                    }
                }
            }
            FetchStep::FetchChobitEmbed { .. } | FetchStep::DownloadVideo { .. } => {
                // Video-related steps — not handled by the metadata plugin
                info("[DEBUG] Skipping video-related fetch step");
            }
            FetchStep::FetchHtml(url) => {
                let cache_key = gameta_lib::providers::dlsite::cache_keys::html_key(product_id);
                log_network_activity(&format!("Fetching HTML: {}", url));
                
                if let Ok(body) = fetch_string_blocking(&cache_key, &url) {
                    info(&format!("[DEBUG] HTML fetched: {} bytes", body.len()));
                    log_network_activity(&format!("HTML response ({} bytes)", body.len()));
                    scraped_data = parse_html(&body);
                    if let Some(data) = &scraped_data {
                        info(&format!("[DEBUG] HTML parsed: title={:?}, geo_blocked={}", data.title, data.geo_blocked));
                        log_network_activity(&format!("HTML parsed: title={:?}, circle={:?}, geo_blocked={}", 
                            data.title, data.circle, data.geo_blocked));
                        // If geo-blocked, dump the full HTML for debugging
                        if data.geo_blocked && STATE.with(|s| s.borrow().dump_html_debug) {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let filename = format!("dlsite_blocked_{}_{}.html", product_id, timestamp);
                            match archust_plugin_sdk::create_file(&filename, body.as_bytes()) {
                                Ok(path) => {
                                    archust_plugin_sdk::warn(&format!(
                                        "[DLSite Plugin] BLOCKED CONTENT - Dumped to: {}",
                                        path
                                    ));
                                }
                                Err(e) => {
                                    archust_plugin_sdk::error(&format!(
                                        "[DLSite Plugin] Failed to dump blocked content: {}",
                                        e
                                    ));
                                }
                            }
                        }
                        
                        fetch_success = true;
                    } else {
                        // HTML parsing failed (likely no metadata found)
                        log_network_activity("Failed to scrape HTML metadata. Invalidating cache.");
                        archust_plugin_sdk::invalidate_cache(&cache_key);
                    }
                }
            }
        }
    }

    if !fetch_success {
        return None;
    }

    // Return metadata immediately — image fetching happens separately via
    // fetch_images_with_progress() so the caller can emit metadata first,
    // then download images without blocking the metadata signal.
    Some((json_data, scraped_data))
}

/// Download cover + screenshot images with status bar progress.
/// Call this AFTER emitting metadata so the UI isn't blocked waiting for images.
pub(crate) fn fetch_images_with_progress(product_id: &str, scraped: &ScrapedData) {
    use archust_plugin_sdk::{log_network_activity, ResourceType};

    let cache_images = STATE.with(|s| s.borrow().cache_images);
    if !cache_images || scraped.geo_blocked {
        if scraped.geo_blocked {
            log_network_activity("[DLSite Plugin] Skipping image cache for geo-blocked content");
        }
        return;
    }

    let mut total = scraped.screenshots.len();
    let has_cover = scraped.cover_image.is_some();
    if has_cover {
        total += 1;
    }
    if total == 0 {
        return;
    }

    let mut done = 0;

    // Cover image
    if let Some(ref cover_url) = scraped.cover_image {
        let cover_key = gameta_lib::providers::dlsite::cache_keys::cover_key(product_id);
        archust_plugin_sdk::arclain::plugin::host::set_status_message(
            &format!("[{}] Downloading cover ({}/{})", product_id, done + 1, total),
        );
        log_network_activity(&format!("Fetching cover image: {}", cover_url));

        if let Err(e) = archust_plugin_sdk::fetch_blocking(&cover_key, cover_url, ResourceType::Image) {
            log_network_activity(&format!("Failed to fetch cover image: {}", e));
        }
        done += 1;
    }

    // Screenshots
    for (idx, url) in scraped.screenshots.iter().enumerate() {
        let key = gameta_lib::providers::dlsite::cache_keys::screenshot_key(product_id, idx);
        archust_plugin_sdk::arclain::plugin::host::set_status_message(
            &format!("[{}] Downloading screenshot {}/{}", product_id, done + 1, total),
        );
        log_network_activity(&format!("Fetching screenshot {}: {}", idx, url));

        if let Err(e) = archust_plugin_sdk::fetch_blocking(&key, url, ResourceType::Image) {
            log_network_activity(&format!("Failed to fetch screenshot {}: {}", idx, e));
        }
        done += 1;
    }

    archust_plugin_sdk::arclain::plugin::host::set_status_message(
        &format!("[{}] Downloaded {} images", product_id, done),
    );
}

/// Walk `scraped.description_structure` for chobit-embed videos, fetch
/// each embed page, parse the player data to find video sources, pick
/// the user-selected quality, and download the video.
///
/// Off by default — toggled via the `cache_videos` plugin setting and
/// configured via `video_quality` ("best" / "low" / a numeric height
/// like "720" / "480"). Files land in the host's temp dir as
/// `dlsite_<product_id>_video_<idx>_<resolution>p.mp4`.
pub(crate) fn fetch_videos_with_progress(product_id: &str, scraped: &ScrapedData) {
    use archust_plugin_sdk::{fetch_string_blocking, fetch_to_cache,
        log_network_activity, ResourceType};
    use gameta_lib::parsers::chobit::{parse_chobit_embed, ChobitVideoInfo, VideoSource};
    use gameta_lib::providers::dlsite::cache_keys;
    use gameta_lib::parsers::dlsite::DescriptionSection as Section;

    let (cache_videos, quality_pref) = STATE.with(|s| {
        let st = s.borrow();
        (st.cache_videos, st.video_quality.clone())
    });
    if !cache_videos || scraped.geo_blocked {
        if scraped.geo_blocked {
            log_network_activity("[DLSite Plugin] Skipping video cache for geo-blocked content");
        }
        return;
    }

    // Collect every chobit embed referenced in the description.
    let videos: Vec<(String, String)> = scraped
        .description_structure
        .iter()
        .filter_map(|section| match section {
            Section::Video {
                video_id,
                embed_url: Some(url),
                ..
            } => Some((video_id.clone(), url.clone())),
            _ => None,
        })
        .collect();

    if videos.is_empty() {
        return;
    }

    log_network_activity(&format!(
        "[DLSite Plugin] Found {} video embed(s) for {}",
        videos.len(),
        product_id,
    ));

    let total = videos.len();
    for (idx, (video_id, embed_url)) in videos.iter().enumerate() {
        archust_plugin_sdk::arclain::plugin::host::set_status_message(&format!(
            "[{}] Resolving video {}/{}",
            product_id,
            idx + 1,
            total,
        ));

        let embed_key = cache_keys::chobit_embed_key(product_id, video_id);
        log_network_activity(&format!("Fetching chobit embed: {}", embed_url));

        let embed_html = match fetch_string_blocking(&embed_key, embed_url) {
            Ok(html) => html,
            Err(e) => {
                log_network_activity(&format!("Failed to fetch chobit embed {}: {}", video_id, e));
                continue;
            }
        };

        let info: ChobitVideoInfo = match parse_chobit_embed(&embed_html) {
            Some(info) => info,
            None => {
                log_network_activity(&format!(
                    "No video sources parsed from chobit embed {}",
                    video_id,
                ));
                continue;
            }
        };

        let chosen: &VideoSource = match select_video_source(&info, &quality_pref) {
            Some(s) => s,
            None => {
                log_network_activity(&format!(
                    "No matching quality '{}' for {} (have {})",
                    quality_pref,
                    video_id,
                    info.sources.len(),
                ));
                continue;
            }
        };

        let video_key = cache_keys::video_key(product_id, video_id, chosen.resolution);
        archust_plugin_sdk::arclain::plugin::host::set_status_message(&format!(
            "[{}] Downloading video {}/{} ({})",
            product_id,
            idx + 1,
            total,
            chosen.quality_label,
        ));
        log_network_activity(&format!(
            "Fetching video {} ({}): {}",
            video_id, chosen.quality_label, chosen.url,
        ));

        // The video lives in the host's content cache after this —
        // bytes never enter the WASM heap. Consumers (organizer,
        // future preview widget) read it back via has_data/get_data
        // on `video_key`.
        match fetch_to_cache(&video_key, &chosen.url, ResourceType::Binary) {
            Ok(()) => log_network_activity(&format!(
                "Cached video {} ({}) under key {}",
                video_id, chosen.quality_label, video_key,
            )),
            Err(e) => log_network_activity(&format!(
                "Failed to cache video {}: {}",
                video_id, e,
            )),
        }
    }

    archust_plugin_sdk::arclain::plugin::host::set_status_message(&format!(
        "[{}] Video downloads complete",
        product_id,
    ));
}

/// Pick a `VideoSource` according to the user's `video_quality`
/// preference. Accepts:
/// * "best" / "" — highest available resolution.
/// * "low" / "lowest" — lowest available resolution.
/// * "720" / "720p" — exact resolution match, falls back to nearest
///   below if the requested height isn't available.
fn select_video_source<'a>(
    info: &'a gameta_lib::parsers::chobit::ChobitVideoInfo,
    pref: &str,
) -> Option<&'a gameta_lib::parsers::chobit::VideoSource> {
    if info.sources.is_empty() {
        return None;
    }

    let pref = pref.trim().to_lowercase();
    match pref.as_str() {
        "" | "best" | "high" | "highest" => info.best_quality(),
        "low" | "lowest" => info.sources.iter().min_by_key(|s| s.resolution.unwrap_or(u32::MAX)),
        _ => {
            // Accept "720", "720p", " 720 ".
            let target: Option<u32> = pref
                .trim_end_matches('p')
                .trim()
                .parse::<u32>()
                .ok();
            if let Some(target) = target {
                if let Some(exact) = info.by_resolution(target) {
                    return Some(exact);
                }
                // Fall back to the nearest source ≤ target; if none,
                // pick the lowest available.
                let nearest_below = info
                    .sources
                    .iter()
                    .filter(|s| s.resolution.map(|r| r <= target).unwrap_or(false))
                    .max_by_key(|s| s.resolution.unwrap_or(0));
                nearest_below.or_else(|| {
                    info.sources
                        .iter()
                        .min_by_key(|s| s.resolution.unwrap_or(u32::MAX))
                })
            } else {
                // Unknown preference string — fall back to best.
                info.best_quality()
            }
        }
    }
}

// Re-export ScrapedData from gameta_lib for convenience
use gameta_lib::providers::dlsite::ScrapedData;


pub(crate) fn generate_metadata_json(
    product_id: &str,
    data: Option<&(serde_json::Value, Option<ScrapedData>)>,
) -> String {
    // Delegate to gameta_lib for JSON generation
    // This moves the data transformation logic out of the plugin
    let (api_json, scraped) = if let Some((j, s)) = data {
        (Some(j), s.as_ref())
    } else {
        (None, None)
    };

    // Debug logging
    if let Some(s) = scraped {
        info(&format!(
            "[DLSite Plugin] Generating JSON: screenshots={}, voice_actors={}, genres={}, cover={}",
            s.screenshots.len(),
            s.voice_actors.len(),
            s.genres.len(),
            s.cover_image.is_some()
        ));
    } else {
        info("[DLSite Plugin] No scraped data available");
    }

    gameta_lib::providers::dlsite::build_plugin_json_string(product_id, api_json, scraped)
}

/// Search DLSite for a query and return list of (code, title, maker, thumbnail_url)
pub(crate) fn search_dlsite(query: &str) -> Vec<(String, String, String, Option<String>)> {
    use archust_plugin_sdk::{fetch_string_blocking, log_network_activity};
    use gameta_lib::providers::dlsite::parse_search_response;

    log_network_activity(&format!("Searching DLSite: {}", query));

    // Try maniax (NSFW/adult) first, then home (SFW/all-ages) as fallback
    // Most content on DLSite is on maniax
    let sections = [("maniax", "adult"), ("home", "all-ages")];

    for (section, section_name) in sections {
        // Use AJAX endpoint which returns JSON with search_result HTML
        let url = format!(
            "https://www.dlsite.com/{}/fsr/ajax/=/language/jp/keyword/{}",
            section,
            urlencoding::encode(query)
        );
        // Cache key includes section to avoid conflicts between sections
        let key = format!("dlsite:search:v3:{}:{}", section, urlencoding::encode(query));

        log_network_activity(&format!("Trying {} section ({})", section, section_name));
        log_network_activity(&format!("GET {}", url));

        let response = match fetch_string_blocking(&key, &url) {
            Ok(h) => h,
            Err(e) => {
                log_network_activity(&format!("Search on {} failed: {}", section, e));
                continue; // Try next section
            }
        };

        // AJAX endpoint returns JSON with search_result HTML
        log_network_activity(&format!("Received {} bytes response from {}", response.len(), section));

        // Try to parse as JSON (AJAX endpoint returns JSON with search_result HTML)
        // Fall back to treating response as raw HTML if JSON parsing fails
        let html = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
            // JSON response - extract search_result HTML
            match json.get("search_result").and_then(|v| v.as_str()) {
                Some(h) => {
                    log_network_activity("Extracted HTML from JSON search_result");
                    h.to_string()
                }
                None => {
                    log_network_activity("No search_result field in JSON, using response as HTML");
                    response.clone()
                }
            }
        } else {
            // Not JSON - treat as raw HTML (fallback for old endpoint or errors)
            log_network_activity("Response is not JSON, treating as raw HTML");
            response.clone()
        };

        // Debug: save HTML for inspection
        if let Ok(path) = archust_plugin_sdk::create_file(&format!("dlsite_search_{}_debug.html", section), html.as_bytes()) {
            log_network_activity(&format!("Saved search HTML to: {}", path));
        }

        // Use gameta_lib provider for parsing
        let results = parse_search_response(&html);

        log_network_activity(&format!("Found {} results on {}", results.len(), section));

        // If we found results, return them; otherwise try next section
        if !results.is_empty() {
            return results
                .into_iter()
                .take(10)
                .map(|r| (r.external_id, r.title, r.creator.unwrap_or_else(|| "Unknown".to_string()), r.thumbnail_url))
                .collect();
        }

        log_network_activity(&format!("No results on {}, trying next section...", section));
    }

    log_network_activity("No results found on any DLSite section");
    Vec::new()
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
