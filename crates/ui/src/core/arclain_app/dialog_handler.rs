//! Dialog handler for ArclainApp
//!
//! ## On the `set_if_changed` pattern (audit B3, kept-as-correct)
//!
//! Every dialog renderer here follows the same shape: `signal.get()` →
//! pass `&mut` into the egui widget call → if user interacted, mutate
//! the local copy as a side-effect → write back via `set_if_changed`.
//!
//! That round-trip is intrinsic to egui's immediate-mode idiom — widgets
//! (`TextEdit::singleline(&mut s)`, `ComboBox`, sliders) mutate the
//! reference they're given THIS frame as the user types/clicks. The
//! audit (`docs/audits/2026-05-19-state-signals.md` §4.5) flagged 13
//! `set_if_changed` sites as a structural smell, but trying to extract
//! mutation out of render would mean wrapping every interactive egui
//! widget in an event-emitting variant — ~400-600 LOC of widget shims
//! across 10 dialogs, fighting egui rather than working with it.
//!
//! The B3 reframing instead moved the dialog *state ownership* off
//! AppSignals onto per-tab `TabState` (slices 1-2 / commits `0f667e6`
//! + `bd30a4c`). The `set_if_changed` calls stay because they're the
//! correct pattern for TabState signals; what changed is *where* the
//! signal lives. Closing a tab now cleanly drops its in-flight dialog
//! state with the tab.
//!
//! File editing additionally has worker-published load results. Its renderer
//! writes back only actual UI changes and merges them against the current
//! request state so a stale frame cannot overwrite a newer completion.
//!
//! The 3 `status_bar.set_if_changed` calls in this file fire *after*
//! an action handler runs (post-action mutation), not during render —
//! those are also correct controller-pattern uses.

use super::ArclainApp;
use crate::core::operations;
use crate::features::password_management;
use crate::features::settings::types::DropBehavior;
use crate::shared::components::drop_overlay::DropZone;
use crate::shared::dialogs;
use crate::shared::image_assets::ImageOwner;
use eframe::egui;

/// Where a drop that aimed at no overlay zone goes, given the user's
/// default preference. `None` means "do not route it at all" -- the
/// preference is to ask, so the caller stashes the paths for the modal
/// instead of opening anything.
fn unaimed_drop_zone(preference: DropBehavior) -> Option<DropZone> {
    match preference {
        DropBehavior::NewTab => Some(DropZone::NewTab),
        DropBehavior::Replace => Some(DropZone::ReplaceCurrent),
        DropBehavior::AskEachTime => None,
    }
}

pub fn render_dialogs(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Password Dialog (now per-tab — read/write to active tab).
    // The pre-2026-05-20 cross-tab switching via `pending_tab_id` is
    // gone: the dialog lives on the tab that triggered the prompt, so
    // the user interacting with the dialog is by definition on the
    // originating tab (it's that tab's dialog they're looking at).
    let shared_state = app.shared_state.clone();
    match password_management::handle_password_dialogs(ctx, &shared_state) {
        password_management::PasswordFeatureAction::PasswordSubmitted {
            operation_id,
            challenge_id,
            password,
        } => {
            // Optimistically remember the password so anything reading
            // the signal mid-open sees the value just typed. Once the
            // open completes, the bridge re-stamps it from the session's
            // own handle regardless (see `crate::core::operation_bridge::
            // relist_for_browser_signals`); if it turns out to be wrong,
            // the operation raises another `Challenge::Password` and
            // this simply gets overwritten by the next submission.
            let tab = shared_state.signals().tabs.get().active().clone();
            tab.current_password.set(Some(password.clone()));
            let facade = shared_state.facade.clone();
            let runtime = shared_state.services.tokio_runtime.clone();
            runtime.spawn(async move {
                if let Some(facade) = facade {
                    let _ = facade
                        .respond_to_challenge(
                            operation_id,
                            arclain_app::challenge::ChallengeResponse::Password {
                                id: challenge_id,
                                value: arclain_app::challenge::SecretInput::new(password),
                            },
                        )
                        .await;
                }
            });
        }
        password_management::PasswordFeatureAction::Cancelled { operation_id } => {
            let facade = shared_state.facade.clone();
            let runtime = shared_state.services.tokio_runtime.clone();
            runtime.spawn(async move {
                if let Some(facade) = facade {
                    let _ = facade.cancel_operation(operation_id).await;
                }
            });
        }
        password_management::PasswordFeatureAction::PasswordSubmittedForReopen {
            tab_id,
            path,
            password,
        } => {
            operations::archive::start_archive_open(&shared_state, tab_id, path, Some(password));
        }
        password_management::PasswordFeatureAction::None => {}
    }

    // Render Password Rules Dialog
    if let Some(result) = password_management::dialogs::zip_pass_rules::render_password_rules_dialog(
        ctx,
        &app.shared_state.theme,
        &mut app.password_management_feature.password_rules_dialog,
    ) {
        match result {
            password_management::dialogs::zip_pass_rules::PasswordRulesResult::Cancel => {
                app.password_management_feature.password_rules_dialog.show = false;
            }
            password_management::dialogs::zip_pass_rules::PasswordRulesResult::Save { rules } => {
                // SavePasswordRules doesn't touch plugins_state, so passing
                // None is safe here (and avoids dragging the PluginsFeature
                // borrow through this dialog path).
                app.settings_feature.handle_action(
                    crate::features::settings::settings_content::SettingsAction::SavePasswordRules { rules },
                    &app.shared_state,
                    None,
                );
                app.password_management_feature.password_rules_dialog.show = false;
            }
        }
    }

    // Render Extraction Progress Dialog (now per-tab — render from active tab).
    // The dialog visualises an op that always originates on a specific tab; the
    // active tab is by definition the one the user is looking at, and only
    // the active tab's dialog can be visible (`show = true`) at render time.
    let active_tab_for_progress = app.shared_state.signals().tabs.get().active().clone();
    let mut ext_dialog = active_tab_for_progress.extraction_dialog().get();
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut ext_dialog,
    ) {
        match result {
            dialogs::progress::ExtractionDialogResult::Cancelled => {
                // Set signal-based cancellation for native backends
                app.shared_state
                    .signals()
                    .extraction_cancel
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                app.shared_state.signals().extraction_progress.set(None);
                // Also cancel the facade-driven CLI extraction, if any --
                // see `crate::core::operations::extraction::cancel_extraction`.
                app.archive_operations.cancel_extraction();
                ext_dialog.show = false;
            }
            // Minimize/Pause/Resume have no facade-level equivalent (the
            // facade exposes cancellation only) -- `start_extraction`
            // disables these buttons for a facade-driven extraction, so
            // these arms are unreached for it; kept as harmless no-ops
            // rather than removed, since the conversion/drag-out dialogs
            // still share this same rendering function and may reach
            // here for their own (unrelated, not-yet-migrated) flows.
            dialogs::progress::ExtractionDialogResult::Minimized => {
                ext_dialog.show = false;
            }
            dialogs::progress::ExtractionDialogResult::Paused
            | dialogs::progress::ExtractionDialogResult::Resumed => {}
            dialogs::progress::ExtractionDialogResult::None => {}
        }
    }
    active_tab_for_progress
        .extraction_dialog()
        .set_if_changed(ext_dialog);

    // No conversion progress dialog: the per-tab `conversion_dialog()`
    // slot had exactly one writer, the pre-facade `convert_archive`
    // bypass, and that is gone. Conversion runs through
    // `ArclainApp::start_convert`/`start_pipeline` now, and a Process
    // page pipeline reports into its own modal further down this
    // function.

    // Render Drag Progress Dialog (now per-tab — read from active tab)
    let mut drag_dialog = active_tab_for_progress.drag_dialog().get();
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut drag_dialog,
    ) {
        if let dialogs::progress::ExtractionDialogResult::Cancelled = result {
            drag_dialog.show = false;
        }
    }
    active_tab_for_progress
        .drag_dialog()
        .set_if_changed(drag_dialog);

    // Render File Edit Dialog (now per-tab — read from active tab)
    let active_tab_for_edit = app.shared_state.signals().tabs.get().active().clone();
    let mut edit_dialog = active_tab_for_edit.file_edit_dialog.get();
    let edit_dialog_before_render = edit_dialog.clone();
    if let Some(result) = crate::features::file_editing::render_file_edit_dialog(
        ctx,
        &app.shared_state.theme,
        &mut edit_dialog,
    ) {
        match result {
            crate::features::file_editing::FileEditResult::Save { new_name, content } => {
                // Saving now goes through the application facade
                // (`start_archive_mutation` with `ReplaceText`), driven
                // by `crate::core::operation_bridge` -- see that
                // module for how the resulting `Completed` event
                // refreshes this tab's entries once the mutation
                // actually lands.
                let active_tab = app.shared_state.signals().tabs.get().active().clone();
                let path_in_archive = edit_dialog.full_path_in_archive.clone();
                if let Some(session_id) = active_tab.archive_session_id.get() {
                    operations::file::start_replace_text(
                        &app.shared_state,
                        active_tab.id,
                        session_id,
                        path_in_archive,
                        content,
                    );
                } else {
                    app.shared_state.signals().status_bar.update(|status| {
                        status.message = "No archive loaded".to_string();
                    });
                }
                // Keeps the dialog open with a durable notice instead of
                // closing it when `new_name` differs from the entry's
                // own path -- see `FileEditDialog::apply_save_outcome`'s
                // own doc comment for why this must not be a status-bar
                // write instead.
                edit_dialog.apply_save_outcome(&new_name);
            }
            crate::features::file_editing::FileEditResult::Cancel => {
                edit_dialog.show = false;
            }
        }
    }
    if edit_dialog != edit_dialog_before_render {
        active_tab_for_edit
            .file_edit_dialog
            .update(|current| current.apply_rendered_snapshot(edit_dialog));
    }

    // Render Close-Tab Confirmation Modal
    {
        let mut confirm = app.shared_state.signals().close_tab_confirm.get();
        let result = crate::shared::dialogs::close_tab_confirm::render_close_tab_confirm(
            ctx,
            &app.shared_state.theme,
            &mut confirm,
        );
        app.shared_state
            .signals()
            .close_tab_confirm
            .set_if_changed(confirm);
        use crate::shared::dialogs::close_tab_confirm::CloseTabConfirmResult;
        if let CloseTabConfirmResult::Confirmed(id) = result {
            // If this tab has a facade-driven extraction in flight,
            // cancel it before the tab goes away -- the facade has no
            // way to notice `tab_cancel` on its own (that flag is a
            // pre-facade cooperative-cancellation convention this
            // operation never polls), so without this the extraction
            // would keep running orphaned in the background with
            // nowhere left to route its progress/completion once the
            // tab (and its `extraction_dialog`) is gone.
            if let Some(tab) = app.shared_state.signals().tabs.get().get(id) {
                if tab.active_extraction_operation.get().is_some() {
                    crate::core::operations::extraction::cancel_extraction(&app.shared_state, tab);
                }
                // An in-flight archive-open must be cancelled the same
                // way -- it is just as much an orphaned background
                // operation once this tab is gone as an extraction is.
                if tab.pending_open_operation.get().is_some() {
                    crate::core::operations::archive::cancel_archive_open(&app.shared_state, tab);
                }
                // Release the facade-side session this tab held open,
                // if any -- otherwise it stays resident in the facade's
                // session store for the rest of the process's life.
                crate::core::operations::archive::close_archive_session(
                    &app.shared_state,
                    tab.archive_session_id.get(),
                );
            }

            // A Process page pipeline run holds this tab's in-flight
            // counter (that is why the confirmation appeared at all), so
            // it is just as orphaned as an extraction once the tab is
            // gone -- and, like every other operation, the facade cannot
            // notice the tab closing. Cancel it through the registry.
            if app.shared_state.signals().process_run.get().origin_tab == Some(id) {
                crate::core::operations::process_runner::cancel_pipeline_run(&app.shared_state);
            }

            let mut col = app.shared_state.signals().tabs.get();
            col.force_close(id);
            app.shared_state.signals().tabs.set(col);
            // ACID best-effort cancellation: force_close fires the tab's
            // `tab_cancel` flag before removing the tab. Background ops that
            // captured Arc<TabState> at spawn observe the flag on their next
            // periodic check and abort.
        }
    }

    // Render Archive-Load Error Modal — surfaces backend failures the
    // user would otherwise diagnose by tail-following the log file.
    // Permission errors get specific chown/chmod commands templated
    // against the failing path; other errors get the raw backend
    // output.
    {
        let mut err_state = app.shared_state.signals().archive_error_dialog.get();
        crate::shared::dialogs::render_archive_error_dialog(
            ctx,
            &app.shared_state.theme,
            &mut err_state,
        );
        app.shared_state
            .signals()
            .archive_error_dialog
            .set_if_changed(err_state);
    }

    // Render Ask-Each-Time Drop Modal
    {
        let mut ask_state = app.shared_state.signals().ask_each_time_drop.get();
        let result = crate::shared::dialogs::ask_each_time_drop::render_ask_each_time_drop_dialog(
            ctx,
            &app.shared_state.theme,
            &mut ask_state,
        );
        use crate::shared::dialogs::ask_each_time_drop::AskEachTimeDropResult;
        // Snapshot pending_paths before we possibly clear them so we can
        // route after setting the cleared state back.
        let pending = std::mem::take(&mut ask_state.pending_paths);
        app.shared_state
            .signals()
            .ask_each_time_drop
            .set_if_changed(ask_state);

        let chosen_zone = match result {
            AskEachTimeDropResult::NewTab => {
                Some(crate::shared::components::drop_overlay::DropZone::NewTab)
            }
            AskEachTimeDropResult::Replace => {
                Some(crate::shared::components::drop_overlay::DropZone::ReplaceCurrent)
            }
            AskEachTimeDropResult::Cancel | AskEachTimeDropResult::None => None,
        };
        if let Some(zone) = chosen_zone {
            use crate::shared::components::drop_overlay::DropZone;
            let mut col = app.shared_state.signals().tabs.get();
            let mut tabs_to_load: Vec<(crate::core::tabs::TabId, std::path::PathBuf)> = Vec::new();
            // First file honors the user's choice; subsequent files
            // always open as new tabs (matching the overlay routing
            // semantics in the drop handler).
            for (idx, path) in pending.iter().enumerate() {
                let effective = if idx == 0 { zone } else { DropZone::NewTab };
                match effective {
                    DropZone::NewTab => {
                        if col.active().archive_path.get().is_none() {
                            crate::core::operations::archive::close_archive_session(
                                &app.shared_state,
                                col.active().archive_session_id.get(),
                            );
                            col.replace_active(path.clone());
                            tabs_to_load.push((col.active_id(), path.clone()));
                        } else {
                            let id = col.open(Some(path.clone()));
                            tabs_to_load.push((id, path.clone()));
                        }
                    }
                    DropZone::ReplaceCurrent => {
                        if col.active().archive_path.get().is_some() {
                            crate::core::operations::archive::close_archive_session(
                                &app.shared_state,
                                col.active().archive_session_id.get(),
                            );
                            col.replace_active(path.clone());
                            tabs_to_load.push((col.active_id(), path.clone()));
                        } else {
                            let id = col.open(Some(path.clone()));
                            tabs_to_load.push((id, path.clone()));
                        }
                    }
                }
            }
            app.shared_state.signals().tabs.set(col);
            for (tab_id, path) in tabs_to_load {
                crate::core::operations::archive::load_archive_into_tab(
                    &app.shared_state,
                    tab_id,
                    &path,
                );
            }
        }
    }

    // Render Merge Dialog (now per-tab — read from active tab)
    let active_tab_for_merge = app.shared_state.signals().tabs.get().active().clone();
    let mut merge_dialog = active_tab_for_merge.merge_dialog.get();
    match dialogs::render_merge_dialog(ctx, &app.shared_state.theme, &mut merge_dialog) {
        dialogs::MergeDialogResult::StartMerge => {
            if let Some(multipart) = merge_dialog.multipart.clone() {
                operations::merge::start_merge(
                    &app.shared_state,
                    &active_tab_for_merge,
                    multipart,
                    merge_dialog.output_format,
                    merge_dialog.compression_level,
                    merge_dialog.delete_originals,
                );
            }
            merge_dialog.close();
        }
        dialogs::MergeDialogResult::Cancel => {
            // Dialog already closed in render function
        }
        dialogs::MergeDialogResult::None => {}
    }
    active_tab_for_merge
        .merge_dialog
        .set_if_changed(merge_dialog);
}

pub fn render_overlays(app: &mut ArclainApp, ctx: &egui::Context) {
    // === Render Drop Overlay ===
    // Displayed when files are being dragged over the window. When files
    // land, routes them to tabs (new or replace) and triggers async loads.
    {
        let hovered = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        if hovered || !dropped.is_empty() {
            egui::Area::new(egui::Id::new("drop_overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.allocate_ui(ctx.viewport_rect().size(), |ui| {
                        let drop_pos = ctx.input(|i| i.pointer.hover_pos());
                        let col_snapshot = app.shared_state.signals().tabs.get();
                        let zone = crate::shared::components::drop_overlay::render_drop_overlay(
                            ui,
                            &col_snapshot,
                            drop_pos,
                        );

                        if !dropped.is_empty() {
                            use crate::shared::components::drop_overlay::DropZone;
                            let mut col = app.shared_state.signals().tabs.get();
                            let ctrl_held = ctx.input(|i| i.modifiers.ctrl);
                            // Dedupe before routing — a single drop gesture
                            // must never open the same archive into two tabs
                            // (some platforms repeat the final file in the
                            // drop event). See `file_drop::dedupe_dropped_paths`.
                            let deduped_paths = crate::core::file_drop::dedupe_dropped_paths(
                                dropped.iter().filter_map(|f| f.path.clone()).collect(),
                            );

                            // Partition before any zone/`DropBehavior`
                            // routing: a non-archive file dropped while
                            // an archive is already open in the active
                            // tab has exactly one sensible action (add
                            // it to that archive), not a "which tab"
                            // question the zone overlay/`AskEachTime`
                            // modal below exists to answer. See
                            // `drag_drop::should_add_to_open_archive`'s
                            // own doc comment.
                            let active_session_id =
                                col_snapshot.active().archive_session_id.get();
                            let (to_add, dropped_paths): (Vec<_>, Vec<_>) =
                                deduped_paths.into_iter().partition(|path| {
                                    crate::features::archive_operations::application::drag_drop::should_add_to_open_archive(
                                        path,
                                        active_session_id,
                                    )
                                });
                            if !to_add.is_empty() {
                                match active_session_id {
                                    Some(session_id) => {
                                        crate::core::operations::file::start_add_files(
                                            &app.shared_state,
                                            col_snapshot.active_id(),
                                            session_id,
                                            to_add,
                                        );
                                    }
                                    None => {
                                        // Only reachable if the active session
                                        // closed in the instant between the
                                        // partition above and here -- defensive,
                                        // not a normal path.
                                        app.shared_state.signals().status_bar.update(|status| {
                                            status.message =
                                                "Dropped file(s) are not archives and no archive \
                                                 is open to add them to."
                                                    .to_string();
                                        });
                                    }
                                }
                            }

                            let mut tabs_to_load: Vec<(
                                crate::core::tabs::TabId,
                                std::path::PathBuf,
                            )> = Vec::new();
                            for (idx, path) in dropped_paths.iter().cloned().enumerate() {
                                // First file honors zone/Ctrl; subsequent files always open new tabs.
                                let effective_zone = if ctrl_held && idx == 0 {
                                    DropZone::ReplaceCurrent
                                } else if idx == 0 {
                                    match zone {
                                        Some(z) => z,
                                        None => {
                                            // No zone aim — honor the user's default preference.
                                            let preference = crate::features::settings::types::DropBehavior::from_settings_str(
                                                &app.shared_state
                                                    .signals()
                                                    .general_settings
                                                    .get()
                                                    .drop_behavior,
                                            );
                                            match unaimed_drop_zone(preference) {
                                                Some(zone) => zone,
                                                None => {
                                                    // Defer routing. Stash all dropped paths in
                                                    // the ask_each_time_drop signal and skip the
                                                    // immediate path-routing for this drop. The
                                                    // modal will render in the dialog pass and
                                                    // route on user click.
                                                    let mut state = app
                                                        .shared_state
                                                        .signals()
                                                        .ask_each_time_drop
                                                        .get();
                                                    state.show = true;
                                                    // Deduped paths (see above) so the modal
                                                    // doesn't route the same archive twice either.
                                                    state.pending_paths = dropped_paths.clone();
                                                    app.shared_state
                                                        .signals()
                                                        .ask_each_time_drop
                                                        .set(state);
                                                    // Bail out of the per-file loop entirely —
                                                    // the modal handles all paths together.
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    DropZone::NewTab
                                };
                                match effective_zone {
                                    DropZone::NewTab => {
                                        // Smart routing: if the active tab is an empty
                                        // placeholder (no archive_path), reuse it
                                        // instead of appending. This keeps a single
                                        // initial "New tab" from accumulating on the
                                        // left when the user drops their first archive.
                                        if col.active().archive_path.get().is_none() {
                                            crate::core::operations::archive::close_archive_session(
                                                &app.shared_state,
                                                col.active().archive_session_id.get(),
                                            );
                                            col.replace_active(path.clone());
                                            tabs_to_load.push((col.active_id(), path));
                                        } else {
                                            let id = col.open(Some(path.clone()));
                                            tabs_to_load.push((id, path));
                                        }
                                    }
                                    DropZone::ReplaceCurrent => {
                                        if col.active().archive_path.get().is_some() {
                                            crate::core::operations::archive::close_archive_session(
                                                &app.shared_state,
                                                col.active().archive_session_id.get(),
                                            );
                                            col.replace_active(path.clone());
                                            tabs_to_load.push((col.active_id(), path));
                                        } else {
                                            let id = col.open(Some(path.clone()));
                                            tabs_to_load.push((id, path));
                                        }
                                    }
                                }
                            }
                            app.shared_state.signals().tabs.set(col);
                            for (tab_id, path) in tabs_to_load {
                                crate::core::operations::archive::load_archive_into_tab(
                                    &app.shared_state,
                                    tab_id,
                                    &path,
                                );
                            }
                        }
                    });
                });
        }
    }

    // Render toast notifications (always on top)
    app.shared_state.toaster.lock().show(ctx);

    // Render plugin dialog if open
    crate::features::plugins::presentation::views::rendering::render_dialog(ctx, &app.shared_state);

    // Process page progress dialog (when a pipeline is running or just completed)
    {
        use crate::features::process::progress_dialog::ProcessProgressResult;
        let run = app.shared_state.signals().process_run.get();
        match crate::features::process::progress_dialog::render(ctx, &app.shared_state.theme, &run)
        {
            Some(ProcessProgressResult::Cancel) => {
                crate::core::operations::process_runner::cancel_pipeline_run(&app.shared_state);
            }
            Some(ProcessProgressResult::Close) => {
                app.shared_state.signals().process_run.update(|state| {
                    state.completed = false;
                    state.summary = None;
                });
            }
            None => {}
        }
    }

    // Render lightbox if open (now per-tab — read from active tab)
    let active_tab_for_lightbox = app.shared_state.signals().tabs.get().active().clone();
    let mut lightbox_state = active_tab_for_lightbox.lightbox_state.get();
    // The owner carries whichever plugin opened this lightbox, so the
    // image store applies the same cross-plugin key check here as for any
    // other plugin-scoped surface -- see `ImageOwner::Lightbox`.
    let lightbox_owner = ImageOwner::Lightbox {
        tab: active_tab_for_lightbox.id,
        plugin_id: lightbox_state.source_plugin.clone(),
    };
    if lightbox_state.show {
        let result = dialogs::render_lightbox(
            ctx,
            &app.shared_state.theme,
            &mut lightbox_state,
            &app.shared_state.image_assets,
            &lightbox_owner,
        );
        match result {
            dialogs::LightboxResult::Closed => {
                app.shared_state.image_assets.release_owner(&lightbox_owner);
            }
            dialogs::LightboxResult::ImageChanged(_index) => {
                // Could notify plugin if needed
            }
            dialogs::LightboxResult::None => {}
        }
        active_tab_for_lightbox
            .lightbox_state
            .set_if_changed(lightbox_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routing this handler applies when a drop aimed at no overlay
    /// zone. Pinned per preference so the "ask" case stays the only one
    /// that routes nothing -- routing it anywhere would open an archive
    /// the user has not yet chosen a destination for.
    #[test]
    fn an_unaimed_drop_follows_the_stored_preference() {
        assert_eq!(
            unaimed_drop_zone(DropBehavior::NewTab),
            Some(DropZone::NewTab)
        );
        assert_eq!(
            unaimed_drop_zone(DropBehavior::Replace),
            Some(DropZone::ReplaceCurrent)
        );
        assert_eq!(unaimed_drop_zone(DropBehavior::AskEachTime), None);
    }

    /// The same routing, reached the way the handler reaches it: from
    /// the stored token rather than from an already-parsed value. An
    /// unrecognized token opens a new tab, never nothing.
    #[test]
    fn an_unaimed_drop_routes_every_stored_token() {
        for (token, expected) in [
            ("new_tab", Some(DropZone::NewTab)),
            ("replace", Some(DropZone::ReplaceCurrent)),
            ("ask_each_time", None),
            ("unrecognized", Some(DropZone::NewTab)),
        ] {
            assert_eq!(
                unaimed_drop_zone(DropBehavior::from_settings_str(token)),
                expected,
                "stored drop preference {token:?} routed unexpectedly"
            );
        }
    }
}
