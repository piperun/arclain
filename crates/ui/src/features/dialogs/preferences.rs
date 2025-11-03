use crate::features::theme::AppTheme;
use super::helpers::{show_dimmed_modal, ModalParams};
use eframe::egui;

// ================= Preferences Dialog =================

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum EncryptedCrcPolicy { 
    OnOpen, 
    PromptOnOpen, 
    OnAccess 
}

impl Default for EncryptedCrcPolicy { 
    fn default() -> Self { Self::OnOpen } 
}

impl EncryptedCrcPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            EncryptedCrcPolicy::OnOpen => "on_open",
            EncryptedCrcPolicy::PromptOnOpen => "prompt_on_open",
            EncryptedCrcPolicy::OnAccess => "on_access",
        }
    }
    
    pub fn display_name(&self) -> &'static str {
        match self {
            EncryptedCrcPolicy::OnOpen => "When opening archive",
            EncryptedCrcPolicy::PromptOnOpen => "Prompt when opening archive",
            EncryptedCrcPolicy::OnAccess => "When opening/editing file",
        }
    }
}

pub struct PreferencesDialog {
    pub show: bool,
    pub key_file_path: String,
    pub secrets_db_path: String,
    pub encrypted_crc_policy: EncryptedCrcPolicy,
    pub info: String,
    pub error: String,
}

impl Default for PreferencesDialog {
    fn default() -> Self {
        Self {
            show: false,
            key_file_path: String::new(),
            secrets_db_path: String::new(),
            encrypted_crc_policy: EncryptedCrcPolicy::default(),
            info: String::new(),
            error: String::new(),
        }
    }
}

pub enum PreferencesDialogResult {
    Save { 
        key_file_path: Option<String>, 
        secrets_db_path: Option<String>, 
        encrypted_crc_policy: Option<String> 
    },
    MoveVault { dest_path: String },
    RekeyVault { new_key_file_path: String },
    ManagePasswords,
    Cancel,
}

pub fn render_preferences_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PreferencesDialog,
) -> Option<PreferencesDialogResult> {
    if !dialog.show { return None; }
    let mut result = None;

    let params = ModalParams {
        width_frac: 0.6,
        height_frac: 0.55,
        min: egui::vec2(560.0, 340.0),
        max: egui::vec2(920.0, 640.0),
        padding: egui::vec2(20.0, 16.0),
        bottom_bar_height: 48.0,
        overlay_alpha: 160,
        overlay_order: egui::Order::Middle,
        modal_order: egui::Order::Foreground,
    };

    // Bottom bar click flags to avoid borrowing `dialog` in both closures
    let mut save_clicked = false;
    let mut cancel_clicked = false;
    let mut manage_clicked = false;
    let mut rekey_path: Option<String> = None;
    let mut move_path: Option<String> = None;

    show_dimmed_modal(ctx, theme, "prefs", &params, |ui, content| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

                // Title
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("⚙ Preferences")
                            .size(18.0)
                            .strong()
                            .color(theme.colors.text_primary)
                    );
                    ui.label(
                        egui::RichText::new("— storage & security")
                            .size(12.0)
                            .color(theme.colors.text_secondary)
                    );
                });

                ui.add_space(4.0);

                // Key file picker
                ui.label(
                    egui::RichText::new("Master key file (32-byte raw / hex / base64)")
                        .size(12.0)
                        .color(theme.colors.text_secondary)
                );
                ui.horizontal(|ui| {
                    let te = egui::TextEdit::singleline(&mut dialog.key_file_path)
                        .hint_text("Select a key file...");
                    ui.add_sized([content.width() - 120.0, 28.0], te);
                    if ui.add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0))).clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            dialog.key_file_path = file.to_string_lossy().to_string();
                        }
                    }
                });

                // Secrets DB picker
                ui.label(
                    egui::RichText::new("Secrets database (redb with AES-256-GCM)")
                        .size(12.0)
                        .color(theme.colors.text_secondary)
                );
                ui.horizontal(|ui| {
                    let te = egui::TextEdit::singleline(&mut dialog.secrets_db_path)
                        .hint_text("Path to pass.redb...");
                    ui.add_sized([content.width() - 120.0, 28.0], te);
                    if ui.add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0))).clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            dialog.secrets_db_path = file.to_string_lossy().to_string();
                        }
                    }
                });

                // Encrypted CRC policy
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Encrypted CRC computation")
                        .size(12.0)
                        .color(theme.colors.text_secondary)
                );
                egui::ComboBox::new("crc_policy", "")
                    .selected_text(dialog.encrypted_crc_policy.display_name())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut dialog.encrypted_crc_policy, 
                            EncryptedCrcPolicy::OnOpen, 
                            EncryptedCrcPolicy::OnOpen.display_name()
                        );
                        ui.selectable_value(
                            &mut dialog.encrypted_crc_policy, 
                            EncryptedCrcPolicy::PromptOnOpen, 
                            EncryptedCrcPolicy::PromptOnOpen.display_name()
                        );
                        ui.selectable_value(
                            &mut dialog.encrypted_crc_policy, 
                            EncryptedCrcPolicy::OnAccess, 
                            EncryptedCrcPolicy::OnAccess.display_name()
                        );
                    });

                ui.add_space(6.0);

                // Info / Error
                if !dialog.info.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(56, 142, 60), &dialog.info);
                }
                if !dialog.error.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error);
                }

            }, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save_btn = egui::Button::new(egui::RichText::new("Save").strong())
                        .min_size(egui::vec2(100.0, 32.0));
                    let cancel_btn = egui::Button::new("Cancel")
                        .min_size(egui::vec2(100.0, 32.0));

                    if ui.add(cancel_btn).clicked() { cancel_clicked = true; }
                    if ui.add(save_btn).clicked() { save_clicked = true; }

                    ui.add_space(16.0);

                    let passwords_btn = egui::Button::new("🔐 Manage Passwords…")
                        .min_size(egui::vec2(160.0, 32.0));
                    if ui.add(passwords_btn).clicked() { manage_clicked = true; }

                    let rekey_btn = egui::Button::new("Rekey vault…")
                        .min_size(egui::vec2(132.0, 32.0));
                    if ui.add(rekey_btn).clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() { rekey_path = Some(file.to_string_lossy().to_string()); }
                    }

                    let move_btn = egui::Button::new("Move vault…")
                        .min_size(egui::vec2(132.0, 32.0));
                    if ui.add(move_btn).clicked() {
                        if let Some(path) = rfd::FileDialog::new().set_file_name("pass.redb").save_file() { move_path = Some(path.to_string_lossy().to_string()); }
                    }
                });
            });

    // Apply actions after modal draw to avoid borrow conflicts
    if cancel_clicked { result = Some(PreferencesDialogResult::Cancel); }
    if save_clicked {
        let key_opt = if dialog.key_file_path.trim().is_empty() { None } else { Some(dialog.key_file_path.trim().to_string()) };
        let db_opt = if dialog.secrets_db_path.trim().is_empty() { None } else { Some(dialog.secrets_db_path.trim().to_string()) };
        let policy_opt = Some(dialog.encrypted_crc_policy.as_str().to_string());
        result = Some(PreferencesDialogResult::Save { key_file_path: key_opt, secrets_db_path: db_opt, encrypted_crc_policy: policy_opt });
    }
    if manage_clicked { result = Some(PreferencesDialogResult::ManagePasswords); }
    if let Some(p) = rekey_path { result = Some(PreferencesDialogResult::RekeyVault { new_key_file_path: p }); }
    if let Some(p) = move_path { result = Some(PreferencesDialogResult::MoveVault { dest_path: p }); }

    result
}