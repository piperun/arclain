use crate::core::tabs::{TabId, TabState};
use crate::shared::components::status_bar;
use crate::shared::dialogs::MergeDialogState;
use crate::shared::SharedState;
use arclain_core::archive::MultiPartArchive;
use arclain_core::ArchiveBackend;
use crc32fast::Hasher;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Starts opening `path` into `tab_id` through the application facade,
/// optionally with a password already in hand (the file-open retry flow
/// in `process_extraction_progress` supplies one when a single-file
/// extraction failed for want of a password -- everything else passes
/// `None` and lets the operation raise a `Challenge::Password` itself if
/// one turns out to be needed). Fire-and-forget: this function returns
/// as soon as the operation is dispatched, registering its id with
/// `shared`'s operation-bridge origins so that worker (see
/// `crate::core::operation_bridge`) can route progress/challenges/
/// completion back to `tab_id`. Every open in this crate funnels through
/// here now -- the file dialog, hotkey open, drop, tab restore/
/// duplicate/reopen-closed, and nested-archive-open call sites just
/// resolve a path and a target tab first.
pub fn start_archive_open(
    shared: &SharedState,
    tab_id: TabId,
    path: PathBuf,
    password: Option<String>,
) {
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[archive] start_archive_open: no application facade available");
        return;
    };
    if let Some(tab) = shared.signals().tabs.get().get(tab_id) {
        // Reset for a fresh open (mirrors the pre-facade `list_archive`'s
        // own `tab.current_password.set(None)`); reinstated immediately
        // when `password` is explicitly supplied (the "reopen with a
        // just-typed password" flow, mirroring `list_with_password`'s
        // own `set(Some(password))`) so
        // `crate::core::operation_bridge::relist_for_browser_signals` can
        // reuse it once the operation completes, without needing to
        // reach into the facade's own session for the password it used.
        tab.current_password.set(password.clone());
    }
    let runtime = shared.services.tokio_runtime.clone();
    let shared = shared.clone();
    runtime.spawn(async move {
        match app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: path,
                password: password.map(arclain_app::challenge::SecretInput::new),
            })
            .await
        {
            Ok(operation_id) => {
                // Set before registering (not after): `register_operation`
                // immediately reconciles against the operation's current
                // snapshot, and a fast-failing open could already be
                // terminal by the time that reconciliation runs. Setting
                // this first means a terminal reconciliation's own
                // `tab.pending_open_operation.set(None)` is the value
                // that sticks, rather than this overwriting it back to
                // `Some` afterward for an operation that already finished.
                if let Some(tab) = shared.signals().tabs.get().get(tab_id) {
                    tab.pending_open_operation.set(Some(operation_id));
                }
                crate::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                    .await;
            }
            Err(error) => {
                tracing::error!("[archive] start_open_archive was rejected: {error:?}");
            }
        }
    });
}

/// Cancels the archive-open operation currently in flight for `tab`, if
/// any. Mirrors `crate::core::operations::extraction::cancel_extraction`
/// -- see the close-tab-confirm handler (`dialog_handler.rs`) for why an
/// in-flight operation must be cancelled before its owning tab goes
/// away: the facade has no way to notice a tab closing on its own, so
/// without this the open would keep running orphaned in the background
/// with nowhere left to route its completion once the tab is gone.
pub fn cancel_archive_open(shared: &SharedState, tab: &Arc<TabState>) {
    let Some(operation_id) = tab.pending_open_operation.get() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        let _ = app.cancel_operation(operation_id).await;
    });
}

/// Fire-and-forget `ArclainApp::close_archive` for `session_id`, if any.
///
/// Every call site that is about to overwrite or discard a tab's
/// `archive_session_id` -- closing the tab, replacing its active
/// archive with a different one -- must call this first with whatever
/// the id was beforehand. `close_archive` is the only way to release
/// the facade-side session `ArclainApp::start_open_archive` opened;
/// without a call here at every such site, the facade keeps the
/// session (and its indexed entry data) alive in memory for the rest of
/// the process's life. A no-op if `session_id` is `None` (nothing was
/// ever open) or the facade is unavailable (test fixtures).
pub fn close_archive_session(
    shared: &SharedState,
    session_id: Option<arclain_app::ids::ArchiveSessionId>,
) {
    let Some(session_id) = session_id else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        if let Err(error) = app.close_archive(session_id).await {
            tracing::warn!(
                "[archive] close_archive failed for a discarded session {session_id:?}: {error:?}"
            );
        }
    });
}

/// Handle opening an archive file via file dialog, into the active tab.
///
/// Multi-part detection happens here (before dispatching the open) since
/// it needs to redirect to the merge dialog instead -- the only call
/// site that ever did this pre-facade too.
pub fn open_archive_via_file_dialog(
    shared: &SharedState,
    merge_dialog: Option<&mut MergeDialogState>,
) {
    if let Some(file) = rfd::FileDialog::new()
        .add_filter("Archives", &["zip", "7z", "rar"])
        .pick_file()
    {
        info!("File selected: {}", file.display());

        if let Some(multipart) = MultiPartArchive::detect(&file) {
            if let Some(md) = merge_dialog {
                md.open(multipart);
                shared.signals().status_bar.update(|status| {
                    status.message =
                        "Multi-part archive detected. Use the dialog to merge.".to_string();
                });
                return;
            }
        }

        let active_id = shared.signals().tabs.get().active_id();
        start_archive_open(shared, active_id, file, None);
    }
}

/// Async archive loader for a specific tab -- the entry point for drop /
/// multi-tab loads, tab restore, tab duplicate, and reopen-closed-tab,
/// where the target tab is a specific (possibly non-active) `TabId`
/// rather than "whichever tab is active right now".
pub fn load_archive_into_tab(shared: &SharedState, tab_id: TabId, path: &std::path::Path) {
    start_archive_open(shared, tab_id, path.to_path_buf(), None);
}

/// Finishes an archive load once the facade's `start_open_archive`
/// operation has completed and `crate::core::operation_bridge` has
/// already populated the tab's flat `entries`/`archive_path`/
/// `archive_extras` signals: precomputes CRC-32 for entries whose
/// content is encrypted (unless the CRC policy is `on_access`), and
/// proactively shows the password dialog when the policy is
/// `prompt_on_open` and the archive turned out to hold encrypted
/// content the open itself didn't need a password for (headers
/// unencrypted, but individual files are). Ported from the pre-facade
/// `load_archive_data`, adapted to read/write signals directly instead
/// of threading `&mut` out-parameters through a render frame -- the
/// operation bridge that calls this has no render frame to thread them
/// through.
pub fn finish_archive_load(shared: &SharedState, tab: &crate::core::tabs::TabState) {
    let (policy, pass_rules, last_entries, fallback_backend) = {
        let state = shared.app_state.lock();
        (
            state.encrypted_crc_policy.clone(),
            state.pass_rules.clone(),
            state.last_entries.clone(),
            state.fallback_backend.clone(),
        )
    };

    let archive_path = tab.archive_path.get();
    let archive_name_owned = archive_path
        .as_ref()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string());
    let auto_pw = arclain_core::utilities::auto_password_for(
        &pass_rules,
        archive_name_owned.as_deref(),
        &last_entries,
    );
    let have_pw = tab.current_password.read().is_some() || auto_pw.is_some();

    if have_pw && policy != "on_access" {
        let password = tab.current_password.get().or_else(|| auto_pw.clone());
        if let (Some(pw), Some(arc_path)) = (password, archive_path.clone()) {
            let paths_to_compute: Vec<String> = {
                let entries_arc = tab.entries.get();
                let headers_encrypted = tab.archive_extras.get().headers_encrypted;
                entries_arc
                    .iter()
                    .filter(|e| {
                        !e.is_dir && (e.encrypted || headers_encrypted) && e.crc32.is_none()
                    })
                    .map(|e| e.path.clone())
                    .collect()
            };
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
                if let Ok(sum) = fallback_backend.crc32_of_entry(&arc_path, &p, Some(&pw)) {
                    computed.push((p, sum));
                }
            }
            if !computed.is_empty() {
                let mut entries_arc = tab.entries.get();
                for (p, sum) in computed {
                    if let Some(e) = std::sync::Arc::make_mut(&mut entries_arc)
                        .iter_mut()
                        .find(|e| e.path == p && e.encrypted && e.crc32.is_none())
                    {
                        e.crc32 = Some(sum);
                    }
                }
                tab.entries.set(entries_arc.clone());
            }
        }
    }

    if policy == "prompt_on_open" && !have_pw {
        let any_encrypted = tab.entries.get().iter().any(|e| e.encrypted);
        if any_encrypted {
            let mut dialog = tab.password_dialog.get();
            dialog.show = true;
            dialog.password.clear();
            dialog.error.clear();
            tab.password_dialog.set(dialog);
            shared.signals().status_bar.update(|status| {
                status.message = "Password required to access encrypted content".to_string();
            });
        }
    }
}

/// Archive information state — output shape of the per-tab
/// `Computed<ArchiveInfo>` (see `TabState::archive_info`).
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
}

/// Non-derivable inputs to `ArchiveInfo` — fields the backend's `list`
/// call surfaces that can't be re-computed from `entries`/`archive_path`.
/// Written by `crate::core::operation_bridge`'s `relist_for_browser_signals`
/// and read by the `Computed<ArchiveInfo>` derivation on `TabState`.
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
    }
}

pub fn convert_archive(
    state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    status_info: &mut status_bar::StatusBarInfo,
    conversion_dialog: &mut crate::shared::dialogs::ExtractionProgressDialog,
    conversion_rx: &mut Option<
        std::sync::mpsc::Receiver<arclain_core::backends::sevenz_cli::ProgressUpdate>,
    >,
    conversion_child: &mut Option<std::process::Child>,
    conversion_started: &mut Option<std::time::Instant>,
    conversion_op_guard: &mut Option<crate::core::tabs::OpGuard>,
    conversion_origin_tab: &mut Option<std::sync::Arc<crate::core::tabs::TabState>>,
    options: arclain_core::ConvertOptions,
) {
    use tracing::error;

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
            // R4: hoist backend_selector clone out of per-file closure so we
            // don't relock AppState once per nested archive. BackendSelector
            // is a single-String clone — cheap.
            let backend_selector = state.lock().backend_selector.clone();
            let report = arclain_core::features::conversion::flatten::flatten_nested_archives(
                &extract_dir,
                options.strip_common_prefix,
                |archive_path, dest_dir| {
                    let backend = backend_selector.select(archive_path)?;
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
                *conversion_dialog = crate::shared::dialogs::ExtractionProgressDialog::default();
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
                *conversion_op_guard = Some(crate::core::tabs::OpGuard::new(&tab));
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

/// Detect whether a backend error message indicates a password
/// failure. Patterns cover the three backends arclain shells out to:
///
/// - 7-Zip CLI / native:  `Wrong password`, `code Some(2)`
/// - UnRAR CLI:           `Incorrect password`, `code Some(11)`
/// - UnRAR native:        `Password for encrypted archive not specified`
/// - Various:             `Cannot/Can not open encrypted`, `Enter password`,
///                        `code Some(255)`
///
/// Pub(crate) so the extraction-progress handler in
/// `app_lifecycle::process_extraction_progress` can reuse it for the
/// "show password dialog on extract failure" path. Keep this list
/// growing as new backends surface new patterns.
pub(crate) fn is_password_error(err_msg: &str) -> bool {
    err_msg.contains("Wrong password")
        || err_msg.contains("Incorrect password")
        || err_msg.contains("Password for encrypted archive not specified")
        || err_msg.contains("Cannot open encrypted")
        || err_msg.contains("Can not open encrypted")
        || err_msg.contains("Enter password")
        || err_msg.contains("code Some(2)")
        || err_msg.contains("code Some(11)")
        || err_msg.contains("code Some(255)")
}
