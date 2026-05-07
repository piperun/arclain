//! Application lifecycle management
//!
//! Handles per-frame lifecycle tasks like signal binding, theme application,
//! metadata updates, and extraction progress handling.

use crate::core::state::AppState;
use crate::features::organization;
use crate::shared::{dialogs, SharedState};
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Process plugin refresh requests
pub fn process_refresh_requests(shared_state: &SharedState, ctx: &egui::Context) {
    let mut requests = shared_state.refresh_requests.lock();
    if !requests.is_empty() {
        tracing::debug!(
            "Processing {} refresh requests: {:?}",
            requests.len(),
            requests
        );
        requests.clear();
        ctx.request_repaint();
    }
}

/// Bind signals to egui context for automatic repaint
pub fn bind_signals_once(app_state: &Arc<Mutex<AppState>>, ctx: &egui::Context, bound: &mut bool) {
    if !*bound {
        let state = app_state.lock();
        state.signals.bind_to_context(ctx);
        drop(state);
        *bound = true;
        tracing::info!("Signals bound to egui context");
    }
}

/// Apply theme to context and set widget theme colors
pub fn apply_theme(shared_state: &SharedState, ctx: &egui::Context) {
    shared_state.theme.apply_to_context(ctx);
    arclain_widgets::set_theme(ctx, shared_state.theme.colors.clone());
}

/// Check for hotkey input and return triggered actions
///
/// This should be called early in the update loop before rendering.
/// Returns a list of triggered actions to be dispatched by the caller.
pub fn process_hotkey_input(
    hotkey_manager: &crate::features::hotkeys::HotkeyManager,
    ctx: &egui::Context,
) -> Vec<crate::features::hotkeys::HotkeyAction> {
    hotkey_manager.check_input(ctx)
}

/// Check for and process metadata updates from plugin signals
pub fn process_metadata_signal(
    shared_state: &SharedState,
    organization_feature: &mut organization::OrganizationFeature,
) {
    let new_metadata = {
        let val = shared_state.signals().metadata.get();
        if val.is_some() {
            shared_state.signals().metadata.set(None); // Consume
            val
        } else {
            None
        }
    };

    if let Some(json_val) = new_metadata {
        let json_str = json_val.to_string();
        match arclain_core::features::organization::GameMetadata::from_json(&json_str) {
            Ok(meta) => {
                tracing::info!(
                    "Received metadata signal update: {} (ID: {})",
                    meta.title,
                    meta.product_id
                );

                // Update global state via signals
                shared_state.signals().game_metadata.set(Some(meta.clone()));
                // Note: metadata already consumed (set to None) on line 63

                // Update active organizer panel
                if let Some(page) = &mut organization_feature.organizer_page {
                    page.panel.session.metadata = Some(meta);
                    page.panel.update_preview();
                    tracing::info!("Updated Organization Panel with new metadata");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to parse metadata from signal: {}", e);
            }
        }
    }
}

/// Handle extraction progress from native backends
pub fn process_extraction_progress(
    shared_state: &SharedState,
    extraction_dialog: &mut dialogs::ExtractionProgressDialog,
    status_message: &mut String,
    ctx: &egui::Context,
) {
    let progress_opt = shared_state.signals().extraction_progress.get();

    if let Some(progress) = progress_opt {
        if progress.complete {
            // Extraction finished - clear the signal
            shared_state.signals().extraction_progress.set(None);

            // Handle file opening
            if let Some(file_path) = progress.file_to_open {
                if file_path.exists() {
                    open_extracted_file(&file_path, status_message);
                } else {
                    tracing::warn!("Extracted file not found: {}", file_path.display());
                    *status_message = "Extraction completed but file not found".to_string();
                }
            } else {
                *status_message = "Extraction completed".to_string();
            }

            // Handle errors
            if let Some(error) = &progress.error {
                tracing::error!("Extraction failed: {}", error);
                if error.contains("cancelled") {
                    *status_message = "Extraction cancelled".to_string();
                } else {
                    *status_message = format!("Extraction failed: {}", error);
                }
            }

            extraction_dialog.show = false;
        } else {
            // Update extraction dialog with current progress
            if !extraction_dialog.show {
                // First progress update - show the dialog
                *extraction_dialog = dialogs::ExtractionProgressDialog {
                    show: true,
                    title: "Extracting files".to_string(),
                    file_action: format!("Extracting {} files...", progress.total),
                    percent: progress.percent,
                    processed_text: format!("{}/{}", progress.current, progress.total),
                    elapsed_text: String::new(),
                    time_left_text: String::new(),
                    status: dialogs::ExtractionStatus::Running,
                    can_minimize: false,
                    can_pause: false,
                    can_cancel: true,
                    error: String::new(),
                    log_lines: vec![progress.current_file.clone()],
                    show_log: false,
                    dest_path: None,
                };
            } else {
                extraction_dialog.percent = progress.percent;
                extraction_dialog.processed_text =
                    format!("{}/{}", progress.current, progress.total);
                extraction_dialog.file_action = progress.current_file.clone();
            }

            ctx.request_repaint();
        }
    }
}

/// Open an extracted file with the system default handler
fn open_extracted_file(file_path: &std::path::Path, status_message: &mut String) {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = std::process::Command::new("explorer")
            .arg(file_path)
            .spawn()
        {
            tracing::warn!("Failed to open file: {}", e);
            *status_message = format!("Failed to open file: {}", e);
        } else {
            tracing::info!("Opened extracted file: {}", file_path.display());
            *status_message = format!(
                "Opened: {}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Err(e) = std::process::Command::new("xdg-open")
            .arg(file_path)
            .spawn()
        {
            tracing::warn!("Failed to open file: {}", e);
            *status_message = format!("Failed to open file: {}", e);
        } else {
            *status_message = format!(
                "Opened: {}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
}

/// Update window title based on current state
pub fn update_window_title(
    shared_state: &SharedState,
    page_navigator: &crate::core::navigation::PageNavigator,
    last_title: &mut Option<String>,
    ctx: &egui::Context,
) {
    let title = {
        if let Some(path) = shared_state.signals().archive_path.get() {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Arclain".to_string())
        } else if let Some(settings_page) = page_navigator.current_settings_page() {
            format!("Settings - {}", settings_page.display_name())
        } else {
            "Arclain".to_string()
        }
    };

    let sanitized = crate::core::operations::window::sanitize_window_title(&title);
    if last_title.as_deref() != Some(&sanitized) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(sanitized.clone()));
        *last_title = Some(sanitized);
    }
}
