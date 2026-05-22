use crate::core::tabs::{OpGuard, TabState};
use crate::core::AppState;
use crate::shared::components::status_bar;
use crate::shared::dialogs;
use arclain_core::backends::sevenz_cli::ProgressUpdate;
use parking_lot::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

/// Extract a set of files (named by their basename within the current
/// navigation folder) from the active tab's archive.
///
/// Callers pre-compute the list of names — either by filtering the
/// active tab's `view_entries` against `browser_view_state.selection`
/// (toolbar "Extract" button), or by passing a single name from a
/// per-row action (file_ops_service::extract). This function used to
/// read selection itself, but coupling it to `BrowserViewState.selection`
/// meant a single-row extract had to mutate global selection — ugly.
/// Names-as-parameter keeps the boundary clean.
pub fn extract_selected(
    state: &Arc<Mutex<AppState>>,
    selected_files: &[String],
    extraction_dialog: &mut dialogs::ExtractionProgressDialog,
    extraction_rx: &mut Option<Receiver<ProgressUpdate>>,
    extraction_child_mut: &mut Option<std::process::Child>,
    extraction_minimized: &mut bool,
    extraction_started: &mut Option<Instant>,
    extraction_op_guard: &mut Option<OpGuard>,
    extraction_origin_tab: &mut Option<Arc<TabState>>,
    status_info: &mut status_bar::StatusBarInfo,
) {
    if extraction_child_mut.is_some() {
        status_info.message = "Another extraction is already running".to_string();
        return;
    }

    let st = state.lock();
    let tab = st.signals.tabs.get().active().clone();
    if let Some(archive) = tab.archive_path.get().as_ref() {
        if selected_files.is_empty() {
            status_info.message = "No files selected".to_string();
            return;
        }

        // Build full paths using navigation prefix
        let nav = tab.navigation.get();
        let full_paths: Vec<String> = if !nav.current_path.is_empty() {
            selected_files
                .iter()
                .map(|f| format!("{}/{}", nav.current_path, f))
                .collect()
        } else {
            selected_files.to_vec()
        };

        let archive_clone = archive.clone();
        let backend = st.fallback_backend.clone();
        let archive_name = archive.to_str();
        let auto_pw = arclain_core::utilities::auto_password_for(
            &st.pass_rules,
            archive_name,
            &st.last_entries,
        );
        let signal_pw = tab.current_password.get();
        let pw_opt = signal_pw
            .as_deref()
            .or(auto_pw.as_deref())
            .map(|s| s.to_string());
        drop(st);

        if let Some(dest) = rfd::FileDialog::new().pick_folder() {
            match backend.spawn_extract_files_with_progress(
                &archive_clone,
                &dest,
                &full_paths,
                pw_opt.as_deref(),
            ) {
                Ok(handle) => {
                    *extraction_dialog = dialogs::ExtractionProgressDialog::default();
                    extraction_dialog.show = true;
                    extraction_dialog.title = format!("Extracting to {}", dest.display());
                    extraction_dialog.file_action = "Extracting selected files".to_string();
                    #[cfg(target_os = "windows")]
                    {
                        extraction_dialog.can_pause = true;
                    }
                    *extraction_rx = Some(handle.rx);
                    *extraction_child_mut = Some(handle.child);
                    *extraction_minimized = false;
                    *extraction_started = Some(Instant::now());
                    extraction_dialog.dest_path = Some(dest.clone());
                    // Wire per-tab in_flight_ops counter and cancel origin.
                    *extraction_op_guard = Some(OpGuard::new(&tab));
                    *extraction_origin_tab = Some(tab.clone());
                    status_info.message = "Extraction started".to_string();
                }
                Err(e) => {
                    status_info.message = format!("Failed to start extraction: {}", e);
                }
            }
        }
    }
}

/// Extract all files from the archive
pub fn extract_all(
    state: &Arc<Mutex<AppState>>,
    extraction_dialog: &mut dialogs::ExtractionProgressDialog,
    extraction_rx: &mut Option<Receiver<ProgressUpdate>>,
    extraction_child_mut: &mut Option<std::process::Child>,
    extraction_minimized: &mut bool,
    extraction_started: &mut Option<Instant>,
    extraction_op_guard: &mut Option<OpGuard>,
    extraction_origin_tab: &mut Option<Arc<TabState>>,
    status_info: &mut status_bar::StatusBarInfo,
) {
    if extraction_child_mut.is_some() {
        status_info.message = "Another extraction is already running".to_string();
        return;
    }

    let st = state.lock();
    let tab = st.signals.tabs.get().active().clone();
    if let Some(archive) = tab.archive_path.get().as_ref() {
        let archive_clone = archive.clone();
        let backend = st.fallback_backend.clone();
        let archive_name = archive.to_str();
        let auto_pw = arclain_core::utilities::auto_password_for(
            &st.pass_rules,
            archive_name,
            &st.last_entries,
        );
        let signal_pw = tab.current_password.get();
        let pw_opt = signal_pw
            .as_deref()
            .or(auto_pw.as_deref())
            .map(|s| s.to_string());
        drop(st);

        if let Some(dest) = rfd::FileDialog::new().pick_folder() {
            match backend.spawn_extract_all_with_progress(&archive_clone, &dest, pw_opt.as_deref())
            {
                Ok(handle) => {
                    *extraction_dialog = dialogs::ExtractionProgressDialog::default();
                    extraction_dialog.show = true;
                    extraction_dialog.title = format!("Extracting all to {}", dest.display());
                    extraction_dialog.file_action = "Extracting all files".to_string();
                    #[cfg(target_os = "windows")]
                    {
                        extraction_dialog.can_pause = true;
                    }
                    *extraction_rx = Some(handle.rx);
                    *extraction_child_mut = Some(handle.child);
                    *extraction_minimized = false;
                    *extraction_started = Some(Instant::now());
                    extraction_dialog.dest_path = Some(dest.clone());
                    // Wire per-tab in_flight_ops counter and cancel origin.
                    *extraction_op_guard = Some(OpGuard::new(&tab));
                    *extraction_origin_tab = Some(tab.clone());
                    status_info.message = "Extraction started".to_string();
                }
                Err(e) => {
                    status_info.message = format!("Failed to start extraction: {}", e);
                }
            }
        }
    }
}
