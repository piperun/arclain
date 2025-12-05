use crate::features::password_management::dialogs::password_dialog::{
    render_password_dialog, PasswordDialogResult,
};
use crate::features::password_management::operations::{PasswordFeature, PasswordFeatureAction};
use crate::shared::SharedState;
use eframe::egui;

pub fn handle_password_dialogs(
    feature: &mut PasswordFeature,
    ctx: &egui::Context,
    shared: &SharedState,
) -> PasswordFeatureAction {
    let mut action = PasswordFeatureAction::None;

    // Render password dialog if open
    if feature.password_dialog.show {
        if let Some(result) =
            render_password_dialog(ctx, &shared.theme, &mut feature.password_dialog)
        {
            match result {
                PasswordDialogResult::Unlock => {
                    let password = feature.password_dialog.password.clone();
                    if let Some(path) = &feature.password_dialog.target_path {
                        action = PasswordFeatureAction::PasswordUnlocked {
                            path: path.clone(),
                            password,
                        };
                    }
                    feature.password_dialog.show = false;
                }
                PasswordDialogResult::Cancel => {
                    feature.password_dialog.show = false;
                }
            }
        }
    }

    // Render rules dialog if open (this might be modal or not, depending on implementation)
    // Assuming it's a modal for now or handled elsewhere.
    // If it's the full page rules editor, that's handled by render_password_rules_page.

    action
}
