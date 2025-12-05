use crate::core::utils::{convert_to_file_entry, format_size};
use crate::core::AppState;
use crate::features::password_management::dialogs;
use crate::shared::components::{file_list, status_bar};
use arclain_core::{ArchiveBackend, ArchiveEntry};
use crc32fast::Hasher;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

/// Handle opening an archive file via file dialog
pub fn open_archive(
    state: &Arc<Mutex<AppState>>,
    current_path: &mut String,
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<file_list::FileEntry>,
    archive_info: &mut ArchiveInfo,
) {
    if let Some(file) = rfd::FileDialog::new()
        .add_filter("Archives", &["zip", "7z", "rar"])
        .pick_file()
    {
        info!("File selected: {}", file.display());
        *current_path = file.to_string_lossy().to_string();

        let mut st = state.lock();
        match st.list_archive(&file) {
            Ok(archive_entries) => {
                let current_archive = st.current_archive.clone();
                drop(st);
                load_archive_data(
                    state,
                    archive_entries,
                    current_archive,
                    password_dialog,
                    pending_archive_path,
                    status_info,
                    entries,
                    archive_info,
                );
            }
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("Wrong password")
                    || err_msg.contains("Cannot open encrypted")
                    || err_msg.contains("Can not open encrypted")
                    || err_msg.contains("Enter password")
                    || err_msg.contains("code Some(2)")
                    || err_msg.contains("code Some(255)")
                {
                    password_dialog.show = true;
                    *pending_archive_path = Some(file);
                    password_dialog.password.clear();
                    password_dialog.error.clear();
                    status_info.message = "Archive is password-protected".to_string();
                } else {
                    error!("Failed to load archive: {}", err_msg);
                    status_info.message = format!("Failed to load archive: {}", err_msg);
                }
            }
        }
    }
}

/// Try to open an archive with a password
pub fn try_open_with_password(
    state: &Arc<Mutex<AppState>>,
    path: &PathBuf,
    password: &str,
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<file_list::FileEntry>,
    archive_info: &mut ArchiveInfo,
) -> bool {
    let mut st = state.lock();
    // Save the current navigation state before re-listing
    let saved_current_path = st.navigation.current_path.clone();
    let saved_path_stack = st.navigation.path_stack.clone();

    match st.list_with_password(path, password) {
        Ok(archive_entries) => {
            // Restore navigation state after re-listing
            st.navigation.current_path = saved_current_path;
            st.navigation.path_stack = saved_path_stack;
            st.navigation.forward_stack.clear();

            // Save successful password rule
            st.save_password_rule_from_archive(path, password).ok();

            let current_archive = st.current_archive.clone();
            drop(st);
            load_archive_data(
                state,
                archive_entries,
                current_archive,
                password_dialog,
                pending_archive_path,
                status_info,
                entries,
                archive_info,
            );
            true
        }
        Err(_) => false,
    }
}

/// Load archive data and update UI state
pub fn load_archive_data(
    state: &Arc<Mutex<AppState>>,
    _archive_entries: Vec<ArchiveEntry>,
    current_archive: Option<PathBuf>,
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<file_list::FileEntry>,
    archive_info: &mut ArchiveInfo,
) {
    // Optionally compute missing CRC-32 for encrypted entries
    let (policy, have_pw, pending_archive) = {
        let st = state.lock();
        (
            st.encrypted_crc_policy.clone(),
            {
                let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                st.current_password.is_some()
                    || st
                        .cfg
                        .auto_password_for(archive_name, &st.last_entries)
                        .is_some()
            },
            st.current_archive.clone(),
        )
    };

    if have_pw && policy != "on_access" {
        let (backend, archive_path, password, paths_to_compute) = {
            let st = state.lock();
            let pw_opt = st.current_password.clone().or_else(|| {
                let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                st.cfg.auto_password_for(archive_name, &st.last_entries)
            });
            let arc = st.current_archive.clone();
            let paths: Vec<String> = st
                .all_entries
                .iter()
                .filter(|e| !e.is_dir && (e.encrypted || st.headers_encrypted) && e.crc32.is_none())
                .map(|e| e.path.clone())
                .collect();

            (st.fallback_backend.clone(), arc, pw_opt, paths)
        };

        if let (Some(pw), Some(arc_path)) = (password, archive_path) {
            let mut computed: Vec<(String, String)> = Vec::new();
            for p in paths_to_compute {
                if let Ok(sum) = backend.crc32_of_entry(&arc_path, &p, Some(&pw)) {
                    computed.push((p, sum));
                }
            }
            if !computed.is_empty() {
                let mut st = state.lock();
                for (p, sum) in computed {
                    if let Some(e) = st
                        .all_entries
                        .iter_mut()
                        .find(|e| e.path == p && e.encrypted && e.crc32.is_none())
                    {
                        e.crc32 = Some(sum);
                    }
                }
            }
        }
    }

    // Build UI rows from potentially updated entries
    {
        let mut st = state.lock();
        *entries = st
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();
        archive_info.archive_encrypted = st.archive_encrypted;
        archive_info.headers_encrypted = st.headers_encrypted;
        archive_info.encryption_method = st.encryption_method.clone();
        
        // Update state's archive info
        st.archive_info.archive_encrypted = st.archive_encrypted;
        st.archive_info.headers_encrypted = st.headers_encrypted;
        st.archive_info.encryption_method = st.encryption_method.clone();
    }

    // Use the latest state entries for totals/CRC aggregation
    let ents: Vec<ArchiveEntry> = {
        let st = state.lock();
        st.all_entries.clone()
    };

    archive_info.total_size = ents.iter().map(|e| e.size).sum();
    archive_info.compressed_size = ents.iter().map(|e| e.packed_size).sum();
    archive_info.file_count = ents.len();

    if let Some(archive_path) = &current_archive {
        archive_info.archive_format = archive_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "Archive".to_string());
    }

    // Compute archive total CRC-32
    let mut pairs: Vec<(String, String)> = ents
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            e.crc32
                .as_ref()
                .map(|c| (e.path.replace('\\', "/"), c.to_uppercase()))
        })
        .collect();

    if pairs.is_empty() {
        archive_info.total_crc32 = None;
    } else {
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Hasher::new();
        for (p, c) in pairs {
            hasher.update(p.as_bytes());
            hasher.update(b":");
            hasher.update(c.as_bytes());
            hasher.update(b"\n");
        }
        let sum = hasher.finalize();
        archive_info.total_crc32 = Some(format!("{:08X}", sum));
    }

    // Auto-prompt if requested
    if policy == "prompt_on_open" && !have_pw {
        let any_encrypted = ents.iter().any(|e| e.encrypted);
        if any_encrypted {
            password_dialog.show = true;
            password_dialog.password.clear();
            password_dialog.error.clear();
            *pending_archive_path = pending_archive;
            status_info.message = "Password required to access encrypted content".to_string();
        }
    }

    // Dispatch event to plugins for metadata enrichment
    if let Some(archive_path) = &current_archive {
        let plugin_metadata = dispatch_metadata_event(state, archive_path);
        archive_info.plugin_metadata = plugin_metadata;
    }

    archive_info.archive_loaded = true;
    status_info.message = "Archive loaded successfully".to_string();
    status_info.file_count = archive_info.file_count;
    status_info.total_size = format_size(archive_info.total_size);
    status_info.compressed_size = format_size(archive_info.compressed_size);
    status_info.archive_format = archive_info.archive_format.clone();

    // Sync back to state
    {
        let mut st = state.lock();
        st.archive_info.total_size = archive_info.total_size;
        st.archive_info.compressed_size = archive_info.compressed_size;
        st.archive_info.file_count = archive_info.file_count;
        st.archive_info.archive_format = archive_info.archive_format.clone();
        st.archive_info.total_crc32 = archive_info.total_crc32.clone();
        st.archive_info.plugin_metadata = archive_info.plugin_metadata.clone();
        st.archive_info.archive_loaded = true;
    }
}

/// Dispatch metadata display event to plugins
fn dispatch_metadata_event(
    state: &Arc<Mutex<AppState>>,
    archive_path: &PathBuf,
) -> Option<serde_json::Value> {
    let st = state.lock();
    let _plugin_manager = st.plugin_manager.as_ref()?;
    drop(st);

    let _event = arclain_plugins::PluginEvent::OnMetadataDisplay {
        archive: archive_path.to_string_lossy().to_string(),
    };

    // Note: dispatch_event requires &mut self, but we have Arc<PluginManager>
    // For now, we'll return None. This will be fixed when we add interior mutability to dispatch_event
    // or restructure to allow mutable access

    tracing::debug!(
        "Would dispatch metadata event for: {}",
        archive_path.display()
    );
    None
}

/// Archive information state
#[derive(Default, Clone)]
pub struct ArchiveInfo {
    pub archive_format: String,
    pub total_size: u64,
    pub compressed_size: u64,
    pub file_count: usize,
    pub archive_encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
    pub total_crc32: Option<String>,
    pub archive_loaded: bool,
    pub plugin_metadata: Option<serde_json::Value>,
}
pub fn convert_archive(
    state: &Arc<Mutex<AppState>>,
    status_info: &mut status_bar::StatusBarInfo,
    conversion_dialog: &mut crate::shared::dialogs::ExtractionProgressDialog,
    conversion_rx: &mut Option<
        std::sync::mpsc::Receiver<arclain_core::backends::sevenz_cli::ProgressUpdate>,
    >,
    conversion_child: &mut Option<std::process::Child>,
    conversion_started: &mut Option<std::time::Instant>,
) {
    let (current_archive, current_password, last_entries, temp_dir) = {
        let st = state.lock();
        (
            st.current_archive.clone(),
            st.current_password.clone(),
            st.last_entries.clone(),
            st.cfg.cfg.temp_dir.clone(),
        )
    };

    if let Some(source_path) = current_archive {
        // Determine default destination filename
        let mut default_name = source_path.file_stem().unwrap_or_default().to_os_string();
        default_name.push(".7z");

        // Open save dialog
        if let Some(dest) = rfd::FileDialog::new()
            .set_file_name(default_name.to_string_lossy())
            .add_filter("7z Archive", &["7z"])
            .save_file()
        {
            info!("Converting {} to {}", source_path.display(), dest.display());

            // Determine temp dir
            let temp = temp_dir.unwrap_or_else(std::env::temp_dir);

            // Get password - try current_password first, then auto-detect
            let password = {
                let st = state.lock();
                let archive_name = source_path.to_str();
                current_password.or_else(|| st.cfg.auto_password_for(archive_name, &last_entries))
            };

            // Use 7z CLI for conversion (fast, with progress)
            let st = state.lock();
            let cli_backend = match arclain_core::backends::SevenZipCli::detect(None) {
                Ok(cli) => cli,
                Err(e) => {
                    error!("7z CLI not found: {}", e);
                    status_info.message = format!("Conversion failed: 7z not found. {}", e);
                    return;
                }
            };

            // Select appropriate backend for source extraction
            let source_backend = match st.backend_selector.select(&source_path) {
                Ok(b) => b,
                Err(e) => {
                    error!("Failed to select backend: {}", e);
                    status_info.message = format!("Conversion failed: {}", e);
                    return;
                }
            };
            drop(st);

            // Create Archive handle for extraction
            let archive = if let Some(pwd) = password.clone() {
                info!("Converting archive with password (length: {})", pwd.len());
                arclain_core::Archive::with_password(source_backend, source_path.clone(), pwd)
            } else {
                info!("Converting archive without password");
                arclain_core::Archive::new(source_backend, source_path.clone())
            };

            // Extract to temp directory first
            let extract_dir = temp.join(format!("arclain_convert_{}", std::process::id()));
            std::fs::create_dir_all(&extract_dir).ok();

            info!("Extracting source archive to temp directory...");
            if let Err(e) = archive.extract_all(&extract_dir) {
                error!("Failed to extract source: {}", e);
                status_info.message = format!("Conversion failed during extraction: {}", e);
                return;
            }

            // Now compress with 7z CLI using progress
            info!("Compressing with 7z CLI...");
            match cli_backend.spawn_convert_with_progress(&extract_dir, &dest) {
                Ok(handle) => {
                    *conversion_dialog =
                        crate::shared::dialogs::ExtractionProgressDialog::default();
                    conversion_dialog.show = true;
                    conversion_dialog.title = format!(
                        "Converting to {}",
                        dest.file_name().unwrap_or_default().to_string_lossy()
                    );
                    conversion_dialog.file_action =
                        "Compressing with 7z (fast, multi-threaded)...".to_string();
                    #[cfg(target_os = "windows")]
                    {
                        conversion_dialog.can_pause = true;
                    }
                    conversion_dialog.can_cancel = true;
                    *conversion_rx = Some(handle.rx);
                    *conversion_child = Some(handle.child);
                    *conversion_started = Some(std::time::Instant::now());
                    status_info.message = "Converting archive...".to_string();
                }
                Err(e) => {
                    error!("Failed to start compression: {}", e);
                    status_info.message = format!("Conversion failed: {}", e);
                    // Cleanup temp dir
                    std::fs::remove_dir_all(&extract_dir).ok();
                }
            }
        }
    }
}
