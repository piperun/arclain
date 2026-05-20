use crate::core::signals::AppSignals;
use crate::core::tabs::{OpGuard, TabId, TabState};
use crate::core::utils::convert_to_file_entry;
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
///
/// Post 2026-05-20 Tier 2 (item 6) audit: dropped the `archive_info`
/// mutable parameter — `tab.archive_info` is now a `Computed<ArchiveInfo>`
/// derived from `entries` + `archive_path` + `archive_extras`, so the
/// caller no longer needs to maintain a local mirror.
pub fn open_archive(
    state: &Arc<Mutex<AppState>>,
    // current_path removed
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
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
        state.lock().signals.tabs.get().active().navigation.set(NavigationState::new());

        let mut st = state.lock();
        match st.list_archive(&file) {
            Ok(archive_entries) => {
                let current_archive = st.signals.tabs.get().active().archive_path.get();
                drop(st);
                load_archive_data(
                    state,
                    archive_entries,
                    current_archive,
                    password_dialog,
                    pending_archive_path,
                    status_info,
                    entries,
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
///
/// See [`open_archive`] for the post-Tier-2 (item 6) parameter trim.
pub fn open_archive_by_path(
    state: &Arc<Mutex<AppState>>,
    path: &std::path::Path,
    // current_path removed - handled via signal reset
    password_dialog: &mut dialogs::PasswordDialog,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
) {
    info!("Opening archive from path: {}", path.display());
    // Reset navigation state entirely for new archive
    state.lock().signals.tabs.get().active().navigation.set(NavigationState::new());

    let mut st = state.lock();
    match st.list_archive(path) {
        Ok(archive_entries) => {
            let current_archive = st.signals.tabs.get().active().archive_path.get();
            drop(st);
            load_archive_data(
                state,
                archive_entries,
                current_archive,
                password_dialog,
                &mut None,
                status_info,
                entries,
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
///
/// See [`open_archive`] for the post-Tier-2 (item 6) parameter trim.
pub fn try_open_with_password(
    state: &Arc<Mutex<AppState>>,
    path: &PathBuf,
    password: &str,
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
) -> bool {
    let mut st = state.lock();
    // Save the current navigation state before re-listing
    let saved_current_path = st.signals.tabs.get().active().navigation.get().current_path.clone();
    let saved_path_stack = st.signals.tabs.get().active().navigation.get().path_stack.clone();

    match st.list_with_password(path, password) {
        Ok(archive_entries) => {
            // Restore navigation state after re-listing
            {
                let tab = st.signals.tabs.get().active().clone();
                let mut nav = tab.navigation.get();
                nav.current_path = saved_current_path;
                nav.path_stack = saved_path_stack;
                nav.forward_stack.clear();
                tab.navigation.set(nav);
            }

            // Save successful password rule. Audit finding H3:
            // previously the failure was silently dropped via `.ok()`,
            // so the next archive open would re-prompt without
            // explanation. Log so the failure is visible to support.
            if let Err(e) = st.save_password_rule_from_archive(path, password) {
                tracing::warn!(
                    "Failed to persist password rule for {}: {}; user will be \
                     re-prompted next time they open this archive",
                    path.display(),
                    e
                );
            }

            let current_archive = st.signals.tabs.get().active().archive_path.get();
            drop(st);
            load_archive_data(
                state,
                archive_entries,
                current_archive,
                password_dialog,
                pending_archive_path,
                status_info,
                entries,
            );
            true
        }
        Err(_) => false,
    }
}

/// Load archive data and update UI state
///
/// Post 2026-05-20 Tier 2 (item 6) audit: no longer takes / mutates a
/// local `ArchiveInfo` — that struct is now a `Computed<ArchiveInfo>`
/// derived from `entries` + `archive_path` + `archive_extras`. The CRC
/// recompute below still mutates `tab.entries` (since per-entry CRC
/// values get filled in from a backend subprocess); the derivation
/// picks up the new values on the next `archive_info.get()`.
pub fn load_archive_data(
    state: &Arc<Mutex<AppState>>,
    _archive_entries: Vec<ArchiveEntry>,
    _current_archive: Option<PathBuf>,
    password_dialog: &mut dialogs::PasswordDialog,
    pending_archive_path: &mut Option<PathBuf>,
    status_info: &mut status_bar::StatusBarInfo,
    entries: &mut Vec<FileEntry>,
) {
    // Audit finding R3: the previous version opened `state.lock()` 6-7
    // times within this single logical operation, with the gaps in
    // between holding none of the values it had just read. Concurrent
    // renders could observe a half-updated AppState. Snapshot everything
    // we need from `AppState` up front, drop the lock, then do all the
    // work (CRC computation, signal writes, status updates) lock-free.
    let snapshot = {
        let st = state.lock();
        ArchiveSnapshot {
            signals: st.signals.clone(),
            policy: st.encrypted_crc_policy.clone(),
            pass_rules: st.pass_rules.clone(),
            last_entries: st.last_entries.clone(),
            fallback_backend: st.fallback_backend.clone(),
            ui_entries: st
                .get_current_entries()
                .iter()
                .map(convert_to_file_entry)
                .collect(),
        }
    };
    let signals = snapshot.signals;
    let tab = signals.tabs.get().active().clone();

    let policy = snapshot.policy;
    let pending_archive = tab.archive_path.get();
    let archive_name_owned = pending_archive
        .as_ref()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string());

    let auto_pw = arclain_core::utilities::auto_password_for(
        &snapshot.pass_rules,
        archive_name_owned.as_deref(),
        &snapshot.last_entries,
    );
    let have_pw = tab.current_password.read().is_some() || auto_pw.is_some();

    if have_pw && policy != "on_access" {
        let password = tab.current_password.get().or_else(|| auto_pw.clone());
        let archive_path = pending_archive.clone();
        let backend = snapshot.fallback_backend.clone();
        let paths_to_compute: Vec<String> = {
            let entries_arc = tab.entries.get();
            let headers_encrypted = tab.archive_extras.get().headers_encrypted;
            entries_arc
                .iter()
                .filter(|e| {
                    !e.is_dir
                        && (e.encrypted || headers_encrypted)
                        && e.crc32.is_none()
                })
                .map(|e| e.path.clone())
                .collect()
        };

        if let (Some(pw), Some(arc_path)) = (password, archive_path) {
            // Each entry triggers a 7z subprocess via crc32_of_entry, so
            // surface the cost up-front rather than letting the user stare
            // at a frozen UI for minutes on a multi-thousand-entry archive.
            if !paths_to_compute.is_empty() {
                tracing::info!(
                    "[archive] Computing CRC-32 for {} encrypted entries (policy={}). \
                     Switch to 'on_access' in Settings if this hangs.",
                    paths_to_compute.len(),
                    policy
                );
            }
            let mut computed: Vec<(String, String)> = Vec::new();
            for p in paths_to_compute {
                if let Ok(sum) = backend.crc32_of_entry(&arc_path, &p, Some(&pw)) {
                    computed.push((p, sum));
                }
            }
            if !computed.is_empty() {
                let mut entries_arc = tab.entries.get();
                for (p, sum) in computed {
                    if let Some(e) = Arc::make_mut(&mut entries_arc)
                        .iter_mut()
                        .find(|e| e.path == p && e.encrypted && e.crc32.is_none())
                    {
                        e.crc32 = Some(sum);
                    }
                }
                // Update signal with modified entries. The
                // archive_info Computed picks up the new CRC values
                // automatically next time anyone calls .get().
                tab.entries.set(entries_arc.clone());
            }
        }
    }

    // Build UI rows from the snapshot we took up front. The previous
    // version re-locked `state` here to call `get_current_entries`,
    // which by this point could have changed beneath us. Using the
    // snapshot is correct: callers compute against the entry list as
    // it existed when load_archive_data was invoked.
    *entries = snapshot.ui_entries;

    // Auto-prompt if requested
    if policy == "prompt_on_open" && !have_pw {
        let any_encrypted = tab.entries.get().iter().any(|e| e.encrypted);
        if any_encrypted {
            password_dialog.show = true;
            password_dialog.password.clear();
            password_dialog.error.clear();
            *pending_archive_path = pending_archive;
            status_info.message = "Password required to access encrypted content".to_string();
        }
    }

    // Status bar pulls counts/sizes/format from the Computed archive_info
    // each render — no manual mirror writes (the mirror fields are gone
    // from StatusBarInfo as of Tier 2 item 7).
    status_info.message = "Archive loaded successfully".to_string();

    // Populate view entries for the initial file list display
    crate::core::operations::navigation_view::refresh_view_entries(&signals);
}

/// Snapshot of AppState fields needed by `load_archive_data`. Built
/// once under a single `state.lock()` to avoid the relock-window
/// pattern flagged by audit finding R3.
struct ArchiveSnapshot {
    signals: crate::core::signals::AppSignals,
    policy: String,
    pass_rules: Vec<arclain_core::utilities::PassRule>,
    last_entries: Vec<String>,
    fallback_backend: arclain_core::backends::sevenz_cli::SevenZipCli,
    ui_entries: Vec<FileEntry>,
}

/// Archive information state — output shape of the per-tab
/// `Computed<ArchiveInfo>` (see `TabState::archive_info`).
///
/// Post 2026-05-20 Tier 2 (item 6): this struct is derived data, no
/// longer a `Signal<T>`. The derivation reads `entries` + `archive_path`
/// (for counts / sizes / format / total_crc32) and `archive_extras`
/// (for the encryption fields the backend reports on `list`). The
/// `archive_loaded` field is gone — callers use `TabState::archive_loaded`
/// directly. `plugin_metadata` is retained for the properties-panel
/// reader but is never written today (plugin metadata flows through
/// `TabState::metadata` instead); it's emitted as `None`.
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
    pub plugin_metadata: Option<serde_json::Value>,
}

/// Non-derivable inputs to `ArchiveInfo` — fields the backend's `list`
/// call surfaces that can't be re-computed from `entries`/`archive_path`.
/// Written by `AppState::list_archive` / `list_with_password` and read
/// by the `Computed<ArchiveInfo>` derivation on `TabState`.
#[derive(Default, Clone)]
pub struct ArchiveExtras {
    pub archive_encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
}

/// Derive an [`ArchiveInfo`] from the given inputs. Pure function — the
/// Computed closure on `TabState` is this with the signals' `.get()`
/// results plugged in.
pub fn derive_archive_info(
    entries: &[arclain_core::ArchiveEntry],
    archive_path: Option<&std::path::Path>,
    extras: &ArchiveExtras,
) -> ArchiveInfo {
    let archive_format = archive_path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(|s| s.to_uppercase())
        .unwrap_or_default();

    let total_size = entries.iter().map(|e| e.size).sum();
    let compressed_size = entries.iter().map(|e| e.packed_size).sum();
    let file_count = entries.len();

    // Aggregate per-entry CRC-32 into a single archive checksum.
    // Sorted path:crc pairs keep the result order-independent across
    // backends that list entries in different orders. Skipped for
    // empty / fully-encrypted archives (no entry CRCs available yet).
    let mut pairs: Vec<(String, String)> = entries
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            e.crc32
                .as_ref()
                .map(|c| (e.path.replace('\\', "/"), c.to_uppercase()))
        })
        .collect();
    let total_crc32 = if pairs.is_empty() {
        None
    } else {
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Hasher::new();
        for (p, c) in pairs {
            hasher.update(p.as_bytes());
            hasher.update(b":");
            hasher.update(c.as_bytes());
            hasher.update(b"\n");
        }
        Some(format!("{:08X}", hasher.finalize()))
    };

    ArchiveInfo {
        archive_format,
        total_size,
        compressed_size,
        file_count,
        archive_encrypted: extras.archive_encrypted,
        headers_encrypted: extras.headers_encrypted,
        encryption_method: extras.encryption_method.clone(),
        total_crc32,
        plugin_metadata: None,
    }
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
    conversion_op_guard: &mut Option<OpGuard>,
    conversion_origin_tab: &mut Option<Arc<TabState>>,
    options: arclain_core::ConvertOptions,
) {
    let signals = state.lock().signals.clone();
    let tab = signals.tabs.get().active().clone();
    let current_archive = tab.archive_path.get();
    let current_password = tab.current_password.get();

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
                // Wire per-tab in_flight_ops counter and cancel origin.
                *conversion_op_guard = Some(OpGuard::new(&tab));
                *conversion_origin_tab = Some(tab.clone());
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

/// Async archive loader for a specific tab.
///
/// Spawns a background thread that lists the archive and writes results
/// into `tab_id`'s signals. This is the preferred entry point for drop /
/// multi-tab loads — it targets a known TabId regardless of which tab is
/// active at completion time.
pub fn load_archive_into_tab(
    state: Arc<Mutex<AppState>>,
    signals: AppSignals,
    tab_id: TabId,
    path: &std::path::Path,
) {
    let Some(tab) = signals.tabs.get().get(tab_id).cloned() else {
        tracing::warn!("[tabs] load_archive_into_tab: tab {:?} not found", tab_id);
        return;
    };

    let path_owned = path.to_path_buf();
    std::thread::spawn(move || {
        let mut st = state.lock();
        match st.list_archive(&path_owned) {
            Ok(archive_entries) => {
                drop(st);
                tab.entries.set(std::sync::Arc::new(archive_entries));
                tab.archive_path.set(Some(path_owned.clone()));
                tab.navigation.set(arclain_core::archive::NavigationState::new());
                // Populate this tab's view_entries so the file list
                // renders immediately. Without this, the UI shows an
                // empty list at root until the user navigates into
                // a folder (which triggers refresh elsewhere).
                crate::core::operations::navigation_view::refresh_view_entries_for_tab(
                    &signals, tab_id,
                );
            }
            Err(e) => {
                drop(st);
                let err_msg = e.to_string();
                if is_password_error(&err_msg) {
                    // password_dialog is per-tab now (post 2026-05-20 B3
                    // reframed slice). Write to the originating tab's
                    // signal — multi-drop scenarios where each encrypted
                    // archive queues a prompt now each land on the
                    // correct tab without overwriting each other.
                    let mut pwd = tab.password_dialog.get();
                    pwd.show = true;
                    pwd.password.clear();
                    pwd.error.clear();
                    pwd.target_path = Some(path_owned.clone());
                    tab.password_dialog.set(pwd);
                } else {
                    tracing::error!("[tabs] load_archive_into_tab failed: {}", err_msg);
                    let mut bar = signals.status_bar.get();
                    bar.message = format!("Failed to load archive: {}", err_msg);
                    signals.status_bar.set(bar);
                }
            }
        }
    });
}

fn is_password_error(err_msg: &str) -> bool {
    err_msg.contains("Wrong password")
        || err_msg.contains("Cannot open encrypted")
        || err_msg.contains("Can not open encrypted")
        || err_msg.contains("Enter password")
        || err_msg.contains("code Some(2)")
        || err_msg.contains("code Some(255)")
}
