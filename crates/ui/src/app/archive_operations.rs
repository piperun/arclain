use crate::app::state::AppState;
use crate::app::utils::{convert_to_file_entry, format_size};
use crate::features::{dialogs, file_list, status_bar};
use arclain_core::ArchiveEntry;
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
                st.current_password.is_some() || st.cfg.auto_password_for(archive_name, &st.last_entries).is_some()
            },
            st.current_archive.clone(),
        )
    };

    if have_pw && policy != "on_access" {
        let (backend, archive_path, password, paths_to_compute) = {
            let st = state.lock();
            let pw_opt = st
                .current_password
                .clone()
                .or_else(|| {
                    let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                    st.cfg.auto_password_for(archive_name, &st.last_entries)
                });
            let arc = st.current_archive.clone();
            let paths: Vec<String> = st
                .all_entries
                .iter()
                .filter(|e| !e.is_dir && e.encrypted && e.crc32.is_none())
                .map(|e| e.path.clone())
                .collect();
            (st.backend.clone(), arc, pw_opt, paths)
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
        let st = state.lock();
        *entries = st
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();
        archive_info.archive_encrypted = st.archive_encrypted;
        archive_info.headers_encrypted = st.headers_encrypted;
        archive_info.encryption_method = st.encryption_method.clone();
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
    
    tracing::debug!("Would dispatch metadata event for: {}", archive_path.display());
    None
}

/// Archive information state
#[derive(Default)]
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
