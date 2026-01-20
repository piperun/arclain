use crate::features::password_management::dialogs::password_dialog::{
    render_password_dialog, PasswordDialogResult,
};
use crate::features::password_management::operations::PasswordFeatureAction;
use crate::shared::SharedState;
use eframe::egui;

pub fn handle_password_dialogs(ctx: &egui::Context, shared: &SharedState) -> PasswordFeatureAction {
    let mut action = PasswordFeatureAction::None;
    let mut dialog = shared.signals().password_dialog.get();

    // Render password dialog if open
    if dialog.show {
        if let Some(result) = render_password_dialog(ctx, &shared.theme, &mut dialog) {
            match result {
                PasswordDialogResult::Unlock => {
                    let password = dialog.password.clone();
                    if let Some(path) = &dialog.target_path {
                        action = PasswordFeatureAction::PasswordUnlocked {
                            path: path.clone(),
                            password,
                        };
                    }
                    dialog.show = false;
                }
                PasswordDialogResult::Cancel => {
                    dialog.show = false;
                }
            }
        }
    }

    shared.signals().password_dialog.set(dialog);

    // Render rules dialog if open (this might be modal or not, depending on implementation)
    // Assuming it's a modal for now or handled elsewhere.
    // If it's the full page rules editor, that's handled by render_password_rules_page.

    action
}
