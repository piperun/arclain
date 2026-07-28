use super::actions::PasswordFeatureAction;
use super::views::dialogs::password_dialog::{render_password_dialog, PasswordDialogResult};

use crate::shared::SharedState;
use eframe::egui;

pub fn handle_password_dialogs(ctx: &egui::Context, shared: &SharedState) -> PasswordFeatureAction {
    let mut action = PasswordFeatureAction::None;
    // password_dialog is per-tab now (post 2026-05-20 B3 reframed slice).
    // Render the active tab's dialog — the originating tab is implicit
    // because each tab owns its own dialog state.
    let active_tab = shared.signals().tabs.get().active().clone();
    let mut dialog = active_tab.password_dialog.get();

    // Render password dialog if open
    if dialog.show {
        if let Some(result) = render_password_dialog(ctx, &shared.theme, &mut dialog) {
            // The front of the queue is always the challenge this
            // dialog is currently displaying -- see
            // `TabState::pending_challenge`'s own doc comment.
            let pending = active_tab.pending_challenge.get().first().cloned();
            match result {
                PasswordDialogResult::Unlock => {
                    let password = dialog.password.clone();
                    match &pending {
                        Some(pending) => {
                            if let arclain_app::challenge::Challenge::Password { id, .. } =
                                pending.challenge
                            {
                                action = PasswordFeatureAction::PasswordSubmitted {
                                    operation_id: pending.operation_id,
                                    challenge_id: id,
                                    password,
                                };
                            }
                        }
                        None => {
                            if let Some(path) = dialog.target_path.clone() {
                                // No in-flight operation challenge -- this
                                // prompt was raised by the older
                                // single-file-extraction "needs a
                                // password" trigger instead (see
                                // `PasswordFeatureAction::
                                // PasswordSubmittedForReopen`'s own doc
                                // comment).
                                action = PasswordFeatureAction::PasswordSubmittedForReopen {
                                    tab_id: active_tab.id,
                                    path,
                                    password,
                                };
                            }
                        }
                    }
                    dialog.show = false;
                }
                PasswordDialogResult::Cancel => {
                    if let Some(pending) = &pending {
                        action = PasswordFeatureAction::Cancelled {
                            operation_id: pending.operation_id,
                        };
                    }
                    dialog.show = false;
                }
            }
            if let Some(pending) = pending {
                // Remove the just-answered/cancelled entry and present
                // whichever challenge (if any) is next in line, rather
                // than leaving the dialog unconditionally hidden and
                // silently forgetting a second in-flight operation's
                // still-queued challenge (see `dequeue_and_present_next`'s
                // own doc comment). This writes `active_tab.password_dialog`
                // directly, so re-sync the local snapshot afterward --
                // otherwise the unconditional `set_if_changed` below
                // would clobber it back with the stale pre-dequeue value.
                crate::core::operation_bridge::dequeue_and_present_next(
                    &active_tab,
                    pending.operation_id,
                );
                dialog = active_tab.password_dialog.get();
            }
        }
    }

    active_tab.password_dialog.set_if_changed(dialog);

    // Render rules dialog if open (this might be modal or not, depending on implementation)
    // Assuming it's a modal for now or handled elsewhere.
    // If it's the full page rules editor, that's handled by render_password_rules_page.

    action
}
