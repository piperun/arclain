use crate::core::utils::{convert_to_file_entry, format_size};
use crate::core::AppState;
use crate::features::password_management::dialogs;
use crate::shared::components::status_bar;
use crate::shared::dialogs::MergeDialogState;
use crate::shared::models::file_entry::FileEntry;
use arclain_core::archive::{MultiPartArchive, NavigationState};
use arclain_core::{ArchiveBackend, ArchiveEntry};
use crc32fast::Hasher;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

/// Handle opening an archive file via file dialog
pub fn open_archive(
    state: &Arc<Mutex<AppState>>,
    // current_path removed
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
    archive_info: &mut ArchiveInfo,
    merge_dialog: Option<&mut MergeDialogState>,
) {
    if let Some(file) = rfd::FileDialog::new()
        .add_filter("Archives", &["zip", "7z", "rar"])
        .pick_file()
    {
        info!("File selected: {}", file.display());

        // Check if this is a multi-part archive
        if let Some(multipart) = MultiPartArchive::detect(&file) {
            if let Some(md) = merge_dialog {
                md.open(multipart);
                status_info.message =
                    "Multi-part archive detected. Use the dialog to merge.".to_string();
                return;
            }
        }

        // Reset navigation state entirely for new archive
        state.lock().signals.navigation.set(NavigationState::new());

        let mut st = state.lock();
        match st.list_archive(&file) {
            Ok(archive_entries) => {
                let current_archive = st.signals.archive_path.get();
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

/// Handle opening an archive file from a specific path (for nested archives)
pub fn open_archive_by_path(
    state: &Arc<Mutex<AppState>>,
    path: &std::path::Path,
    // current_path removed - handled via signal reset
    password_dialog: &mut dialogs::PasswordDialog,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
    archive_info: &mut ArchiveInfo,
) {
    info!("Opening archive from path: {}", path.display());
    // Reset navigation state entirely for new archive
    state.lock().signals.navigation.set(NavigationState::new());

    let mut st = state.lock();
    match st.list_archive(path) {
        Ok(archive_entries) => {
            let current_archive = st.signals.archive_path.get();
            drop(st);
            load_archive_data(
                state,
                archive_entries,
                current_archive,
                password_dialog,
                &mut None,
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

/// Try to open an archive with a password
pub fn try_open_with_password(
    state: &Arc<Mutex<AppState>>,
    path: &PathBuf,
    password: &str,
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
    archive_info: &mut ArchiveInfo,
) -> bool {
    let mut st = state.lock();
    // Save the current navigation state before re-listing
    let saved_current_path = st.signals.navigation.get().current_path.clone();
    let saved_path_stack = st.signals.navigation.get().path_stack.clone();

    match st.list_with_password(path, password) {
        Ok(archive_entries) => {
            // Restore navigation state after re-listing
            {
                let mut nav = st.signals.navigation.get();
                nav.current_path = saved_current_path;
                nav.path_stack = saved_path_stack;
                nav.forward_stack.clear();
                st.signals.navigation.set(nav);
            }

            // Save successful password rule
            st.save_password_rule_from_archive(path, password).ok();

            let current_archive = st.signals.archive_path.get();
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
    entries: &mut Vec<FileEntry>,
    archive_info: &mut ArchiveInfo,
) {
    // Optionally compute missing CRC-32 for encrypted entries
    let signals = state.lock().signals.clone();

    let (policy, have_pw, pending_archive) = {
        let st = state.lock();
        (
            st.encrypted_crc_policy.clone(),
            {
                let archive_name = signals
                    .archive_path
                    .get()
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string());
                signals.current_password.get().is_some()
                    || arclain_core::utilities::auto_password_for(
                        &st.pass_rules,
                        archive_name.as_deref(),
                        &st.last_entries,
                    )
                    .is_some()
            },
            signals.archive_path.get(),
        )
    };

    if have_pw && policy != "on_access" {
        let (backend, archive_path, password, paths_to_compute) = {
            let st = state.lock();
            let pw_opt = signals.current_password.get().or_else(|| {
                let archive_name = signals
                    .archive_path
                    .get()
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string());
                arclain_core::utilities::auto_password_for(
                    &st.pass_rules,
                    archive_name.as_deref(),
                    &st.last_entries,
                )
            });
            let arc = signals.archive_path.get();
            let entries_arc = signals.entries.get();
            let paths: Vec<String> = entries_arc
                .iter()
                .filter(|e| {
                    !e.is_dir
                        && (e.encrypted || st.signals.archive_info.get().headers_encrypted)
                        && e.crc32.is_none()
                })
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
                let mut entries_arc = signals.entries.get();
                let st = state.lock();
                for (p, sum) in computed {
                    if let Some(e) = Arc::make_mut(&mut entries_arc)
                        .iter_mut()
                        .find(|e| e.path == p && e.encrypted && e.crc32.is_none())
                    {
                        e.crc32 = Some(sum);
                    }
                }
                // Update signal with modified entries
                st.signals.entries.set(entries_arc.clone());
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

        // Read encryption info from signal
        let current_ai = st.signals.archive_info.get();
        archive_info.archive_encrypted = current_ai.archive_encrypted;
        archive_info.headers_encrypted = current_ai.headers_encrypted;
        archive_info.encryption_method = current_ai.encryption_method.clone();
        // Note: archive_info will be synced to signal at the end of this function
    }

    // Use the latest state entries for totals/CRC aggregation
    let ents: Arc<Vec<ArchiveEntry>> = signals.entries.get();

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

    archive_info.archive_loaded = true;
    status_info.message = "Archive loaded successfully".to_string();
    status_info.file_count = archive_info.file_count;
    status_info.total_size = format_size(archive_info.total_size);
    status_info.compressed_size = format_size(archive_info.compressed_size);
    status_info.archive_format = archive_info.archive_format.clone();

    // Sync to signal only (archive_info removed from AppState)
    {
        let st = state.lock();
        let mut ai = st.signals.archive_info.get();
        ai.total_size = archive_info.total_size;
        ai.compressed_size = archive_info.compressed_size;
        ai.file_count = archive_info.file_count;
        ai.archive_format = archive_info.archive_format.clone();
        ai.total_crc32 = archive_info.total_crc32.clone();
        ai.plugin_metadata = archive_info.plugin_metadata.clone();
        ai.archive_loaded = true;
        st.signals.archive_info.set(ai);

        // Populate view entries for the initial file list display
        crate::core::operations::navigation_view::refresh_view_entries(&st.signals);
    }
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
    options: arclain_core::ConvertOptions,
) {
    let signals = state.lock().signals.clone();
    let current_archive = signals.archive_path.get();
    let current_password = signals.current_password.get();

    let (last_entries, temp_dir) = {
        let st = state.lock();
        (
            st.last_entries.clone(),
            st.user_config.temp_dir.as_ref().map(PathBuf::from),
        )
    };

    if let Some(source_path) = current_archive {
        let target_ext = options.format.extension();

        // Determine default destination filename with chosen format extension
        let mut default_name = source_path.file_stem().unwrap_or_default().to_os_string();
        default_name.push(format!(".{}", target_ext));

        // Open save dialog — or use provided path if options.output_path is set (batch)
        let dest = if let Some(p) = options.output_path.clone() {
            Some(p)
        } else {
            let filter_label = match options.format {
                arclain_core::ConvertFormat::Zip => "ZIP Archive",
                arclain_core::ConvertFormat::SevenZ => "7z Archive",
            };
            rfd::FileDialog::new()
                .set_file_name(default_name.to_string_lossy())
                .add_filter(filter_label, &[target_ext])
                .save_file()
        };

        let Some(dest) = dest else {
            return;
        };

        info!("Converting {} to {}", source_path.display(), dest.display());

        let temp = temp_dir.unwrap_or_else(std::env::temp_dir);

        // Password: explicit option overrides auto-detect
        let password = options.password.clone().or_else(|| {
            let st = state.lock();
            let archive_name = source_path.to_str();
            current_password.or_else(|| {
                arclain_core::utilities::auto_password_for(
                    &st.pass_rules,
                    archive_name,
                    &last_entries,
                )
            })
        });

        let st = state.lock();
        let cli_backend = match arclain_core::backends::SevenZipCli::detect(None) {
            Ok(cli) => cli,
            Err(e) => {
                error!("7z CLI not found: {}", e);
                status_info.message = format!("Conversion failed: 7z not found. {}", e);
                return;
            }
        };

        let source_backend = match st.backend_selector.select(&source_path) {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to select backend: {}", e);
                status_info.message = format!("Conversion failed: {}", e);
                return;
            }
        };
        drop(st);

        let archive = if let Some(pwd) = password.clone() {
            info!("Converting archive with password (length: {})", pwd.len());
            arclain_core::Archive::with_password(source_backend, source_path.clone(), pwd)
        } else {
            arclain_core::Archive::new(source_backend, source_path.clone())
        };

        let extract_dir = temp.join(format!("arclain_convert_{}", std::process::id()));
        std::fs::create_dir_all(&extract_dir).ok();

        info!("Extracting source archive to temp directory...");
        if let Err(e) = archive.extract_all(&extract_dir) {
            error!("Failed to extract source: {}", e);
            status_info.message = format!("Conversion failed during extraction: {}", e);
            return;
        }

        // Flatten nested archives if requested
        if options.flatten_nested {
            info!("Flattening nested archives in extract dir...");
            let state_clone = state.clone();
            let report = arclain_core::features::conversion::flatten::flatten_nested_archives(
                &extract_dir,
                options.strip_common_prefix,
                |archive_path, dest_dir| {
                    let backend = state_clone.lock().backend_selector.select(archive_path)?;
                    backend.extract_all(archive_path, dest_dir, None)
                },
            );
            match report {
                Ok(r) => {
                    info!(
                        "[Convert] Flatten: {} extracted, {} skipped, {} failed",
                        r.extracted.len(),
                        r.skipped.len(),
                        r.failed.len()
                    );
                    for (name, err) in &r.failed {
                        tracing::warn!("[Convert] Flatten failed for {}: {}", name, err);
                    }
                }
                Err(e) => {
                    error!("Flatten operation failed: {}", e);
                    status_info.message = format!("Flatten failed: {}", e);
                    std::fs::remove_dir_all(&extract_dir).ok();
                    return;
                }
            }
        }

        info!("Compressing with 7z CLI ({})...", target_ext);
        match cli_backend.spawn_convert_with_progress(
            &extract_dir,
            &dest,
            options.format.clone(),
            options.compression,
        ) {
            Ok(handle) => {
                *conversion_dialog =
                    crate::shared::dialogs::ExtractionProgressDialog::default();
                conversion_dialog.show = true;
                conversion_dialog.title = format!(
                    "Converting to {}",
                    dest.file_name().unwrap_or_default().to_string_lossy()
                );
                conversion_dialog.file_action =
                    format!("Compressing with 7z ({}, multi-threaded)...", target_ext);
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
                std::fs::remove_dir_all(&extract_dir).ok();
            }
        }
    }
}
