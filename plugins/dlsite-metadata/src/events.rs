//! UI event handling for the DLsite plugin.
//!
//! Was the body of `Component::on_ui_event` — a series of system-event
//! prefix checks (`__page_init`, `event:archive_opened`,
//! `background_fetch_complete:`, `do_native_fetch:`)
//! followed by a 22-arm match on the UI element id, plus another set
//! of prefix-guarded ids (`view_cache_entry_`, `select_entry_`,
//! `refresh_view_`, `refetch_entry_`, the `RJ`/`VJ`/`BJ`-prefix list
//! item handler, etc.). Lifted out so `lib.rs` is just lifecycle +
//! state + free-fn helpers; this file owns the entire event-dispatch
//! flow.
//!
//! Behaviour preserved exactly — same prefix order, same match arms,
//! same `STATE.with(...)` access. The only change is that bare
//! identifier references (`STATE`, `detect_code_from_archive`, etc.)
//! resolve through `crate::` since we're outside the impl block now.

use crate::{
    detect_code_from_archive, emit_dlsite_metadata, fetch_dlsite_metadata,
    fetch_images_with_progress, fetch_videos_with_progress, generate_metadata_json,
    get_cached_dlsite_metadata, get_total_image_count, perform_scan_cached_only,
    raw_metadata_cache_keys, sanitize_filename, search_dlsite, STATE,
};
use archust_plugin_sdk::info;

const MAX_RETURNED_TOAST_BYTES: usize = 1024;

fn toast(
    message: impl Into<String>,
    level: archust_plugin_sdk::arclain::plugin::ui::ToastLevel,
) -> archust_plugin_sdk::arclain::plugin::ui::PluginAction {
    use archust_plugin_sdk::arclain::plugin::ui::{PluginAction, ToastConfig};
    let mut message = message.into();
    if message.len() > MAX_RETURNED_TOAST_BYTES {
        let mut end = MAX_RETURNED_TOAST_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    message.shrink_to_fit();
    PluginAction::ShowToast(ToastConfig { message, level })
}

pub(crate) fn dispatch(
    id: String,
    value: Option<String>,
) -> Vec<archust_plugin_sdk::arclain::plugin::ui::PluginAction> {
    use archust_plugin_sdk::arclain::plugin::ui::{OpenLightboxConfig, PluginAction};

    // Handle page initialization - set display name for breadcrumb
    if id == "__page_init" {
        if let Some(page_id) = &value {
            if page_id.starts_with("dlsite_browser") {
                return vec![PluginAction::SetPageDisplayName(
                    "DLSite Browser".to_string(),
                )];
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
        info(&format!(
            "[DLSite Plugin] Background fetch complete: {}",
            key
        ));
        STATE.with(|s| s.borrow_mut().fetch_in_progress = false);
        if let Some(code) = key.strip_prefix("dlsite:") {
            if let Some((json, scraped)) = get_cached_dlsite_metadata(code) {
                info(&format!(
                    "[DLSite Plugin] Loaded {} from cache after fetch",
                    code
                ));
                let metadata_json =
                    generate_metadata_json(code, Some(&(json.clone(), scraped.clone())));
                emit_dlsite_metadata(&metadata_json);
                STATE.with(|s| {
                    s.borrow_mut().found_metadata = Some((code.to_string(), json, scraped));
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

    // The host asks the plugin to perform its own DLsite fetch when no
    // gameta server is available (or the gameta fetch returned nothing).
    // We hold the plugin lock for the duration of the HTTP call, but
    // since this only fires when the user has explicitly clicked Fetch
    // or auto-fetch is enabled, the cost is contained to occasional
    // archive opens / button clicks rather than every UI frame.
    if let Some(key) = id.strip_prefix("do_native_fetch:") {
        info(&format!(
            "[DLSite Plugin] Native fetch requested for {}",
            key
        ));
        let code = key.strip_prefix("dlsite:").unwrap_or(key).to_string();

        match fetch_dlsite_metadata(&code) {
            Some((json, scraped)) => {
                info(&format!(
                    "[DLSite Plugin] Native fetch succeeded for {}",
                    code
                ));
                let metadata_json =
                    generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
                emit_dlsite_metadata(&metadata_json);

                // Mirror the select_result_ / refetch_entry_ paths: kick off
                // image + video downloads after metadata so the new entry
                // shows up complete in the DLSite browser. Without this,
                // auto-fetched archives ended up in the DB but rendered
                // without cover/screenshots, and `cache_videos = true`
                // never produced any video downloads.
                if let Some(ref s) = scraped {
                    fetch_images_with_progress(&code, s);
                    fetch_videos_with_progress(&code, s);
                }

                STATE.with(|s| {
                    let mut state = s.borrow_mut();
                    state.found_metadata = Some((code.clone(), json, scraped));
                    state.fetch_in_progress = false;
                    // Drop the per-filter summary memo so the newly
                    // cached entry shows up on the DLSite tab without
                    // a manual refresh. The "known entries" list is
                    // now host-cached and invalidated automatically
                    // (LibraryService::save_metadata clears its cache),
                    // so we no longer need a plugin-side drop here.
                    state.cached_summaries = None;
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
            let metadata_json =
                generate_metadata_json(&code, Some(&(json.clone(), scraped.clone())));
            emit_dlsite_metadata(&metadata_json);

            // Download images + videos with progress
            if let Some(ref s) = scraped {
                fetch_images_with_progress(&code, s);
                fetch_videos_with_progress(&code, s);
            }

            // Rename archive if option is checked
            if should_rename {
                if let Some(archive_info) = archust_plugin_sdk::current_archive_info() {
                    // Get title from metadata
                    let title = json["work_name"]
                        .as_str()
                        .or_else(|| json["title"].as_str())
                        .map(sanitize_filename);

                    // Get current extension
                    let current_name = &archive_info.filename;
                    let extension = current_name.rsplit('.').next().unwrap_or("7z");

                    // Build new filename: [CODE] Title.ext or [CODE] original.ext
                    let new_name = match title {
                        Some(t) if !t.is_empty() => format!("[{}] {}.{}", code, t, extension),
                        _ => format!("[{}] {}", code, current_name),
                    };

                    info(&format!(
                        "[DLSite Plugin] Renaming archive to: {}",
                        new_name
                    ));
                    match archust_plugin_sdk::rename_archive(&new_name) {
                        Ok(new_path) => {
                            info(&format!(
                                "[DLSite Plugin] Archive renamed successfully to: {}",
                                new_path
                            ));
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
                return vec![PluginAction::RefreshPanel(
                    "Page:dlsite_browser".to_string(),
                )];
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
                return vec![PluginAction::RefreshPanel(
                    "Page:dlsite_browser".to_string(),
                )];
            }

            if action_part.starts_with("select_") {
                // Direct select by carousel index
                if let Ok(carousel_idx) = action_part.trim_start_matches("select_").parse::<usize>()
                {
                    STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        // Carousel index 0 = cover (-1), 1+ = sample (carousel_idx - 1)
                        state.current_image_index = if carousel_idx == 0 {
                            -1
                        } else {
                            (carousel_idx - 1) as i32
                        };
                    });
                    return vec![PluginAction::RefreshPanel(
                        "Page:dlsite_browser".to_string(),
                    )];
                }
            }

            if action_part == "open_lightbox" {
                // Open lightbox with all images (respecting cached tab filter)
                return STATE.with(|s| {
                    let state = s.borrow();
                    let is_cached_tab = state.browser_tab == "cached";

                    if let Some((product_id, _json, scraped)) = &state.browser_detail_cache {
                        let mut images: Vec<(String, Option<String>)> = Vec::new();
                        let mut seen_urls: std::collections::HashSet<String> =
                            std::collections::HashSet::new();

                        // Add cover (only if cached when on cached tab)
                        let cover_key =
                            gameta_lib::providers::dlsite::cache_keys::cover_key(product_id);
                        let cover_url = scraped.as_ref().and_then(|s| s.cover_image.clone());
                        let cover_is_cached =
                            archust_plugin_sdk::arclain::plugin::host::has_data(&cover_key);

                        let show_cover = if is_cached_tab {
                            cover_is_cached
                        } else {
                            cover_url.is_some() || cover_is_cached
                        };

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
                                    let key =
                                        gameta_lib::providers::dlsite::cache_keys::screenshot_key(
                                            product_id, i,
                                        );
                                    let is_cached =
                                        archust_plugin_sdk::arclain::plugin::host::has_data(&key);

                                    let should_include =
                                        if is_cached_tab { is_cached } else { true };

                                    if should_include {
                                        seen_urls.insert(url.clone());
                                        images.push((key, Some(url.clone())));
                                    }
                                }
                            }
                        }

                        // Convert current_image_index to carousel start index
                        let start_idx = if state.current_image_index < 0 {
                            0u32
                        } else {
                            (state.current_image_index + 1) as u32
                        };
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
                info(&format!(
                    "[DLSite Plugin] Auto-fetch setting changed to: {}",
                    enabled
                ));
            }
        }
        "enable_cache" => {
            if let Some(val) = value {
                let enabled = val == "true";
                STATE.with(|state| {
                    state.borrow_mut().enable_cache = enabled;
                });
                archust_plugin_sdk::arclain::plugin::host::set_setting("enable_cache", &val);
                info(&format!(
                    "[DLSite Plugin] Cache setting changed to: {}",
                    enabled
                ));
            }
        }
        "rename_with_code" => {
            if let Some(val) = value {
                let enabled = val == "true";
                STATE.with(|state| {
                    state.borrow_mut().rename_with_code = enabled;
                });
                info(&format!(
                    "[DLSite Plugin] Rename with code setting changed to: {}",
                    enabled
                ));
            }
        }
        "dump_html_debug" => {
            if let Some(val) = value {
                let enabled = val == "true";
                STATE.with(|state| {
                    state.borrow_mut().dump_html_debug = enabled;
                });
                // We don't persist this one to disk as it's a temp debug setting, or we could if needed
                info(&format!(
                    "[DLSite Plugin] Dump HTML debug setting changed to: {}",
                    enabled
                ));
            }
        }
        "cache_videos" => {
            if let Some(val) = value {
                let enabled = val == "true";
                STATE.with(|state| {
                    state.borrow_mut().cache_videos = enabled;
                });
                archust_plugin_sdk::arclain::plugin::host::set_setting("cache_videos", &val);
                info(&format!(
                    "[DLSite Plugin] Cache videos setting changed to: {}",
                    enabled
                ));
            }
        }
        "video_quality" => {
            if let Some(val) = value {
                STATE.with(|state| {
                    state.borrow_mut().video_quality = val.clone();
                });
                archust_plugin_sdk::arclain::plugin::host::set_setting("video_quality", &val);
                info(&format!(
                    "[DLSite Plugin] Video quality setting changed to: {}",
                    val
                ));
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
            use archust_plugin_sdk::arclain::plugin::ui::ToastLevel;
            info("[DLSite Plugin] Clearing plugin-owned DLSite cached files...");
            let cleared = archust_plugin_sdk::invalidate_cache("dlsite:*");
            return vec![if cleared {
                toast(
                    "DLSite cached files were removed; saved product metadata remains.",
                    ToastLevel::Success,
                )
            } else {
                toast(
                    "DLSite cached files could not be cleared.",
                    ToastLevel::Error,
                )
            }];
        }
        "prune_cache" => {
            use archust_plugin_sdk::arclain::plugin::host::get_data;
            use archust_plugin_sdk::arclain::plugin::ui::ToastLevel;
            use archust_plugin_sdk::{invalidate_cache, list_cached_metadata};

            const PRUNE_PAGE_SIZE: u32 = 128;
            const MAX_PRUNE_METADATA_ENTRIES: u32 = 1024;

            info("[DLSite Plugin] Starting cache prune...");
            let mut scanned = 0;
            let mut removed = 0;
            let mut offset = 0;
            while offset < MAX_PRUNE_METADATA_ENTRIES {
                let Ok(entries) = list_cached_metadata("dlsite", offset, PRUNE_PAGE_SIZE) else {
                    break;
                };
                let page_len = entries.len();
                for product_id in entries {
                    for key in raw_metadata_cache_keys(&product_id) {
                        let Some(data_bytes) = get_data(&key) else {
                            continue;
                        };
                        scanned += 1;
                        let data_str = String::from_utf8_lossy(&data_bytes);
                        let is_valid = if key.contains(":json:") {
                            gameta_lib::providers::dlsite::parse_api_response(
                                &product_id,
                                &data_str,
                            )
                            .is_ok()
                        } else {
                            gameta_lib::providers::dlsite::parse_html_response(&data_str).is_some()
                        };
                        if !is_valid && invalidate_cache(&key) {
                            removed += 1;
                        }
                    }
                }
                if page_len < PRUNE_PAGE_SIZE as usize {
                    break;
                }
                offset = offset.saturating_add(PRUNE_PAGE_SIZE);
            }

            info(&format!(
                "[DLSite Plugin] Prune finished. Scanned {}, Removed {}.",
                scanned, removed
            ));
            return vec![toast(
                format!("Scanned {scanned} cached files; removed {removed} invalid files."),
                ToastLevel::Success,
            )];
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
                    let metadata_json =
                        generate_metadata_json(&product_id, Some(&(json.clone(), scraped.clone())));
                    emit_dlsite_metadata(&metadata_json);

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
                        info(&format!("[DLSite Plugin] Fetching metadata for {code}..."));
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
            use archust_plugin_sdk::arclain::plugin::ui::ToastLevel;
            let message = STATE.with(|state| {
                if let Some((id, json, scraped)) = &state.borrow().found_metadata {
                    let title = json["title"].as_str().unwrap_or("Unknown");
                    let maker = json["creator"].as_str().unwrap_or("Unknown");
                    let price = json["dlsite"]["price"].as_u64().unwrap_or(0);

                    let desc_len = scraped
                        .as_ref()
                        .and_then(|s| s.description.as_ref())
                        .map(|s| s.len())
                        .unwrap_or(0);
                    let screenshots_count =
                        scraped.as_ref().map(|s| s.screenshots.len()).unwrap_or(0);

                    let msg = format!(
                        "Title: {}\nCircle: {}\nPrice: {} JPY\nCode: {}\nDescription Length: {}\nScreenshots: {}",
                        title, maker, price, id, desc_len, screenshots_count
                    );
                    Some(msg)
                } else {
                    None
                }
            });
            if let Some(message) = message {
                return vec![toast(message, ToastLevel::Info)];
            }
        }
        "refresh_cache" => {
            // The per-filter summary memo stays in WASM heap (it's
            // user-filter-specific, not cross-tab state). The "known
            // entries" list lives in LibraryService's cache now and
            // is invalidated automatically on writes — no plugin-side
            // drop needed here.
            STATE.with(|state| {
                state.borrow_mut().cached_summaries = None;
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
            return vec![PluginAction::RefreshPanel(
                "Page:dlsite_browser".to_string(),
            )];
        }
        id if id.starts_with("load_details_") => {
            // One-time fetch of details for the selected entry
            let entry_id = id.trim_start_matches("load_details_").to_string();
            info(&format!(
                "[DLSite Plugin] Loading details for: {}",
                entry_id
            ));

            if let Some((json, scraped)) = fetch_dlsite_metadata(&entry_id) {
                STATE.with(|state| {
                    state.borrow_mut().browser_detail_cache =
                        Some((entry_id.clone(), json, scraped));
                });
                info(&format!(
                    "[DLSite Plugin] Details loaded and cached for: {}",
                    entry_id
                ));
            } else {
                info(&format!(
                    "[DLSite Plugin] Failed to load details for: {}",
                    entry_id
                ));
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
                use archust_plugin_sdk::arclain::plugin::ui::ToastLevel;
                emit_dlsite_metadata(&json);

                return vec![toast(
                    format!("Selected for Use: {entry_id}"),
                    ToastLevel::Success,
                )];
            } else {
                use archust_plugin_sdk::arclain::plugin::ui::ToastLevel;
                return vec![toast("Could not find cached details", ToastLevel::Error)];
            }
        }

        // Browser UI handlers
        "browser_tabs" => {
            if let Some(tab) = value {
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    s.browser_tab = if tab == "Search" {
                        "search".to_string()
                    } else {
                        "cached".to_string()
                    };
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
                let is_product_code = (trimmed.starts_with("RJ")
                    || trimmed.starts_with("VJ")
                    || trimmed.starts_with("BJ"))
                    && trimmed.len() >= 4
                    && trimmed[2..].chars().all(|c| c.is_ascii_digit());

                if is_product_code {
                    // Direct lookup - skip search and go straight to details
                    info(&format!(
                        "[DLSite Plugin] Direct lookup for product code: {}",
                        trimmed
                    ));

                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.selected_cache_entry = Some(trimmed.clone());
                        s.current_image_index = -1;
                        s.browser_loading = true;
                        // Note: Keep cached_carousel_images HashMap intact for instant back-navigation
                    });

                    // Try cache first, then network
                    let (json, scraped) = if let Some(cached) = get_cached_dlsite_metadata(&trimmed)
                    {
                        info(&format!("[DLSite Plugin] Found {} in cache", trimmed));
                        cached
                    } else {
                        info(&format!(
                            "[DLSite Plugin] Fetching {} from network",
                            trimmed
                        ));
                        match fetch_dlsite_metadata(&trimmed) {
                            Some(data) => data,
                            None => {
                                STATE.with(|s| s.borrow_mut().browser_loading = false);
                                return vec![toast(
                                    format!("Could not find product: {trimmed}"),
                                    archust_plugin_sdk::arclain::plugin::ui::ToastLevel::Error,
                                )];
                            }
                        }
                    };

                    STATE.with(|state| {
                        let mut s = state.borrow_mut();
                        s.browser_detail_cache = Some((trimmed.clone(), json, scraped));
                        s.browser_loading = false;
                    });

                    return vec![PluginAction::RefreshPanel(
                        "Page:dlsite_browser".to_string(),
                    )];
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
            use archust_plugin_sdk::arclain::plugin::ui::ToastLevel;
            if let Some(archive_info) = archust_plugin_sdk::current_archive_info() {
                // Copy the filename without extension to clipboard
                let filename = &archive_info.filename;
                // Remove extension for easier searching
                let name_without_ext = filename
                    .rsplit('.')
                    .skip(1)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(".");
                let copy_name = if name_without_ext.is_empty() {
                    filename.clone()
                } else {
                    name_without_ext
                };
                return vec![
                    PluginAction::CopyToClipboard(copy_name),
                    toast("Filename copied to clipboard", ToastLevel::Success),
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
                        state.borrow_mut().browser_detail_cache =
                            Some((id.to_string(), json, scraped));
                    });
                }
            }

            // Refresh to show details
            return vec![PluginAction::RefreshPanel(
                "Page:dlsite_browser".to_string(),
            )];
        }
        id if id.starts_with("apply_metadata_") => {
            let code = id.trim_start_matches("apply_metadata_").to_string();
            // Emit metadata for this code
            if let Some((json, scraped)) = fetch_dlsite_metadata(&code) {
                STATE.with(|state| {
                    state.borrow_mut().found_metadata =
                        Some((code.clone(), json.clone(), scraped.clone()));
                });

                let metadata_json = generate_metadata_json(&code, Some(&(json, scraped)));
                emit_dlsite_metadata(&metadata_json);
                return vec![toast(
                    format!("Applied metadata for {code}"),
                    archust_plugin_sdk::arclain::plugin::ui::ToastLevel::Success,
                )];
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
                    state.borrow_mut().browser_detail_cache =
                        Some((entry_id.clone(), json, scraped));
                });
                info(&format!("[DLSite Plugin] Cache re-read for: {}", entry_id));
            }

            return vec![PluginAction::RefreshPanel(
                "Page:dlsite_browser".to_string(),
            )];
        }
        // Refetch from network
        id if id.starts_with("refetch_entry_") => {
            let entry_id = id.trim_start_matches("refetch_entry_").to_string();
            info(&format!(
                "[DLSite Plugin] Refetching from network: {}",
                entry_id
            ));

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
            info(&format!(
                "[DLSite Plugin] Invalidated cache for: {}, {}",
                json_key, html_key
            ));

            // Fetch from network (updates cache)
            let notice = match fetch_dlsite_metadata(&entry_id) {
                Some((json, scraped)) => {
                    // Emit metadata immediately
                    let metadata_json =
                        generate_metadata_json(&entry_id, Some(&(json.clone(), scraped.clone())));
                    emit_dlsite_metadata(&metadata_json);

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
                    info(&format!(
                        "[DLSite Plugin] Refetched and cached: {}",
                        entry_id
                    ));
                    toast(
                        format!("Refetched {entry_id}"),
                        archust_plugin_sdk::arclain::plugin::ui::ToastLevel::Success,
                    )
                }
                None => {
                    info(&format!("[DLSite Plugin] Refetch FAILED for: {}", entry_id));

                    // Restore from backup if we had data before
                    if let Some((json, scraped)) = backup_data {
                        info(&format!(
                            "[DLSite Plugin] Restoring backup data for: {}",
                            entry_id
                        ));
                        // Re-emit the old metadata to re-persist it
                        let metadata_json = generate_metadata_json(
                            &entry_id,
                            Some(&(json.clone(), scraped.clone())),
                        );
                        emit_dlsite_metadata(&metadata_json);

                        STATE.with(|state| {
                            state.borrow_mut().browser_detail_cache =
                                Some((entry_id.clone(), json, scraped));
                        });
                        toast(
                            format!("Refetch failed for {entry_id}; restored previous data."),
                            archust_plugin_sdk::arclain::plugin::ui::ToastLevel::Warning,
                        )
                    } else {
                        toast(
                            format!("Failed to refetch {entry_id}; no backup was available."),
                            archust_plugin_sdk::arclain::plugin::ui::ToastLevel::Error,
                        )
                    }
                }
            };

            return vec![
                PluginAction::RefreshPanel("Page:dlsite_browser".to_string()),
                notice,
            ];
        }
        _ => {}
    }

    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returned_toast_messages_are_utf8_safely_bounded() {
        use archust_plugin_sdk::arclain::plugin::ui::{PluginAction, ToastLevel};

        let PluginAction::ShowToast(config) =
            toast("é".repeat(MAX_RETURNED_TOAST_BYTES), ToastLevel::Info)
        else {
            panic!("toast helper returned the wrong action")
        };

        assert!(config.message.len() <= MAX_RETURNED_TOAST_BYTES);
        assert!(config.message.is_char_boundary(config.message.len()));
    }
}
