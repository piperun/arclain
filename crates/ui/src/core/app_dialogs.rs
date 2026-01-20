//! Application dialogs module
//!
//! Handles rendering and result processing for all dialogs:
//! - Password dialogs
//! - Extraction progress dialogs
//! - Conversion dialogs
//! - File edit dialogs
//!
//! Note: This module provides infrastructure for future dialog extraction.
//! Functions are not yet wired into arclain_app.rs.

#![allow(dead_code)]

use crate::features::password_management;
use crate::shared::{dialogs, SharedState};
use eframe::egui;

/// Result from password dialog handling
pub enum PasswordDialogResult {
    None,
    PasswordUnlocked {
        path: std::path::PathBuf,
        password: String,
    },
}

/// Handle password dialogs
pub fn handle_password_dialogs(
    ctx: &egui::Context,
    shared_state: &SharedState,
) -> PasswordDialogResult {
    match password_management::handle_password_dialogs(ctx, shared_state) {
        password_management::PasswordFeatureAction::PasswordUnlocked { path, password } => {
            PasswordDialogResult::PasswordUnlocked { path, password }
        }
        password_management::PasswordFeatureAction::None => PasswordDialogResult::None,
    }
}

/// Result from password rules dialog
pub enum PasswordRulesResult {
    None,
    Cancel,
    Save {
        rules: Vec<password_management::dialogs::zip_pass_rules::PasswordRule>,
    },
}

/// Handle password rules dialog
pub fn handle_password_rules_dialog(
    ctx: &egui::Context,
    shared_state: &SharedState,
    dialog: &mut password_management::dialogs::zip_pass_rules::PasswordRulesDialog,
) -> PasswordRulesResult {
    if let Some(result) = password_management::dialogs::zip_pass_rules::render_password_rules_dialog(
        ctx,
        &shared_state.theme,
        dialog,
    ) {
        match result {
            password_management::dialogs::zip_pass_rules::PasswordRulesResult::Cancel => {
                PasswordRulesResult::Cancel
            }
            password_management::dialogs::zip_pass_rules::PasswordRulesResult::Save { rules } => {
                PasswordRulesResult::Save { rules }
            }
        }
    } else {
        PasswordRulesResult::None
    }
}

/// Result from extraction progress dialog
pub enum ExtractionDialogResult {
    None,
    Cancelled,
    Minimized,
    Paused,
    Resumed,
}

/// Handle extraction progress dialog
pub fn handle_extraction_dialog(
    ctx: &egui::Context,
    shared_state: &SharedState,
    dialog: &mut dialogs::ExtractionProgressDialog,
) -> ExtractionDialogResult {
    if let Some(result) =
        dialogs::progress::render_extraction_progress_dialog(ctx, &shared_state.theme, dialog)
    {
        match result {
            dialogs::progress::ExtractionDialogResult::Cancelled => {
                ExtractionDialogResult::Cancelled
            }
            dialogs::progress::ExtractionDialogResult::Minimized => {
                ExtractionDialogResult::Minimized
            }
            dialogs::progress::ExtractionDialogResult::Paused => ExtractionDialogResult::Paused,
            dialogs::progress::ExtractionDialogResult::Resumed => ExtractionDialogResult::Resumed,
            dialogs::progress::ExtractionDialogResult::None => ExtractionDialogResult::None,
        }
    } else {
        ExtractionDialogResult::None
    }
}

/// Result from file edit dialog
pub enum FileEditResult {
    None,
    Save { new_name: String, content: String },
    Cancel,
}

/// Handle file edit dialog
pub fn handle_file_edit_dialog(
    ctx: &egui::Context,
    shared_state: &SharedState,
    dialog: &mut crate::features::file_editing::FileEditDialog,
) -> FileEditResult {
    if let Some(result) = crate::features::file_editing::file_edit_dialog::render_file_edit_dialog(
        ctx,
        &shared_state.theme,
        dialog,
    ) {
        match result {
            crate::features::file_editing::file_edit_dialog::FileEditResult::Save {
                new_name,
                content,
            } => FileEditResult::Save { new_name, content },
            crate::features::file_editing::file_edit_dialog::FileEditResult::Cancel => {
                FileEditResult::Cancel
            }
        }
    } else {
        FileEditResult::None
    }
}
