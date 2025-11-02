use super::theme::AppTheme;
use eframe::egui;

// ================= Password Dialog =================

pub struct PasswordDialog {
    pub show: bool,
    pub password: String,
    pub error: String,
}

impl Default for PasswordDialog {
    fn default() -> Self { Self { show: false, password: String::new(), error: String::new() } }
}

pub enum PasswordDialogResult { Unlock, Cancel }

pub fn render_password_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordDialog,
) -> Option<PasswordDialogResult> {
    if !dialog.show { return None; }
    let mut result = None;

    // Dim overlay on a lower layer so it never covers the dialog
    egui::Area::new(egui::Id::new("password_overlay_dim")).order(egui::Order::Middle).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
        let _ = ui.allocate_rect(screen, egui::Sense::click());
    });

    // Modal dialog on the foreground layer
    egui::Area::new(egui::Id::new("password_modal")).order(egui::Order::Foreground).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        // Slightly larger modal to avoid button overflow
        let width = 520.0;
        let height = if dialog.error.is_empty() { 300.0 } else { 340.0 };
        let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
        let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

        ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, theme.colors.border_color));

        let content = rect.shrink(24.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::LEFT)));
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🔒").size(24.0));
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Archive Password Required").size(16.0).strong().color(theme.colors.text_primary));
            });

            ui.label(egui::RichText::new("This archive is password-protected. Please enter the password to continue.").size(14.0).color(theme.colors.text_secondary));

            let password_response = ui.add_sized([content.width(), 40.0], egui::TextEdit::singleline(&mut dialog.password).password(true).hint_text("Enter password...").font(egui::TextStyle::Body));
            password_response.request_focus();
            // Press Enter to unlock while the field is focused
            if password_response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !dialog.password.is_empty() {
                result = Some(PasswordDialogResult::Unlock);
            }
            // Optional: ESC cancels
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                result = Some(PasswordDialogResult::Cancel);
            }

            if !dialog.error.is_empty() { ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error); }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(content.width() - 212.0);
                let cancel_btn = egui::Button::new(egui::RichText::new("Cancel").size(14.0).color(theme.colors.text_primary)).fill(theme.colors.bg_tertiary).stroke(egui::Stroke::new(1.0, theme.colors.border_color)).rounding(4.0).min_size(egui::vec2(100.0, 36.0));
                if ui.add(cancel_btn).clicked() { result = Some(PasswordDialogResult::Cancel); }
                ui.add_space(12.0);
                let unlock_enabled = !dialog.password.is_empty();
                let unlock_btn = egui::Button::new(egui::RichText::new("Unlock").size(14.0).strong().color(if theme.dark_mode { egui::Color32::BLACK } else { egui::Color32::WHITE })).fill(if theme.dark_mode { egui::Color32::WHITE } else { egui::Color32::BLACK }).rounding(4.0).min_size(egui::vec2(100.0, 36.0));
                if ui.add_enabled(unlock_enabled, unlock_btn).clicked() { result = Some(PasswordDialogResult::Unlock); }
            });
        });
    });

    result
}

// ================= File Edit Dialog =================

pub struct FileEditDialog {
    pub show: bool,
    pub full_path_in_archive: String,
    pub name_input: String,
    pub content: String,
    pub error: String,
}

impl Default for FileEditDialog {
    fn default() -> Self { Self { show: false, full_path_in_archive: String::new(), name_input: String::new(), content: String::new(), error: String::new() } }
}

pub enum FileEditResult { Save { new_name: String, content: String }, Cancel }

pub fn render_file_edit_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut FileEditDialog,
) -> Option<FileEditResult> {
    if !dialog.show { return None; }
    let mut result = None;

    egui::Area::new(egui::Id::new("file_edit_overlay_dim")).order(egui::Order::Middle).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
        let _ = ui.allocate_rect(screen, egui::Sense::click());
    });

    egui::Area::new(egui::Id::new("file_edit_modal")).order(egui::Order::Foreground).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        let width = (screen.width() * 0.6).clamp(520.0, 900.0);
        let height = (screen.height() * 0.7).clamp(420.0, 900.0);
        let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
        let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

        ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, theme.colors.border_color));

        let content_rect = rect.shrink2(egui::vec2(20.0, 16.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content_rect).layout(egui::Layout::top_down_justified(egui::Align::Min)));
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("✏ Edit File").size(18.0).strong());
                ui.label(egui::RichText::new("— inline editor").size(12.0).color(theme.colors.text_secondary));
            });

            ui.label(egui::RichText::new("File name").size(12.0).color(theme.colors.text_secondary));
            ui.add_sized([content_rect.width(), 32.0], egui::TextEdit::singleline(&mut dialog.name_input));

            ui.label(egui::RichText::new("Content").size(12.0).color(theme.colors.text_secondary));
            ui.add_sized([content_rect.width(), content_rect.height() - 140.0], egui::TextEdit::multiline(&mut dialog.content).font(egui::TextStyle::Monospace).code_editor());

            if !dialog.error.is_empty() { ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error); }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = ui.add(egui::Button::new(egui::RichText::new("Save").strong()).min_size(egui::vec2(100.0, 32.0)));
                let cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0)));
                if save.clicked() { result = Some(FileEditResult::Save { new_name: dialog.name_input.clone(), content: dialog.content.clone() }); }
                if cancel.clicked() { result = Some(FileEditResult::Cancel); }
            });
        });
    });

    result
}


// ================= Preferences Dialog =================

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum EncryptedCrcPolicy { OnOpen, PromptOnOpen, OnAccess }

impl Default for EncryptedCrcPolicy { fn default() -> Self { Self::OnOpen } }

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
    Save { key_file_path: Option<String>, secrets_db_path: Option<String>, encrypted_crc_policy: Option<String> },
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

    // Dim overlay
    egui::Area::new(egui::Id::new("prefs_overlay_dim")).order(egui::Order::Middle).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
        let _ = ui.allocate_rect(screen, egui::Sense::click());
    });

    // Modal
    egui::Area::new(egui::Id::new("prefs_modal")).order(egui::Order::Foreground).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        let width = (screen.width() * 0.6).clamp(560.0, 920.0);
        let height = (screen.height() * 0.55).clamp(340.0, 640.0);
        let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
        let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

        ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, theme.colors.border_color));

        let content = rect.shrink2(egui::vec2(20.0, 16.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );

        child.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            // Title
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("⚙ Preferences").size(18.0).strong().color(theme.colors.text_primary));
                ui.label(egui::RichText::new("— storage & security").size(12.0).color(theme.colors.text_secondary));
            });

            ui.add_space(4.0);

            // Key file picker
            ui.label(egui::RichText::new("Master key file (32-byte raw / hex / base64)").size(12.0).color(theme.colors.text_secondary));
            ui.horizontal(|ui| {
                let te = egui::TextEdit::singleline(&mut dialog.key_file_path).hint_text("Select a key file...");
                ui.add_sized([content.width() - 120.0, 28.0], te);
                if ui.add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0))).clicked() {
                    if let Some(file) = rfd::FileDialog::new().pick_file() {
                        dialog.key_file_path = file.to_string_lossy().to_string();
                    }
                }
            });

            // Secrets DB picker
            ui.label(egui::RichText::new("Secrets database (redb with AES-256-GCM)").size(12.0).color(theme.colors.text_secondary));
            ui.horizontal(|ui| {
                let te = egui::TextEdit::singleline(&mut dialog.secrets_db_path).hint_text("Path to pass.redb...");
                ui.add_sized([content.width() - 120.0, 28.0], te);
                if ui.add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0))).clicked() {
                    if let Some(file) = rfd::FileDialog::new().pick_file() {
                        dialog.secrets_db_path = file.to_string_lossy().to_string();
                    }
                }
            });

            // Encrypted CRC policy
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Encrypted CRC computation").size(12.0).color(theme.colors.text_secondary));
            egui::ComboBox::from_id_source("crc_policy")
                .selected_text(dialog.encrypted_crc_policy.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dialog.encrypted_crc_policy, EncryptedCrcPolicy::OnOpen, EncryptedCrcPolicy::OnOpen.display_name());
                    ui.selectable_value(&mut dialog.encrypted_crc_policy, EncryptedCrcPolicy::PromptOnOpen, EncryptedCrcPolicy::PromptOnOpen.display_name());
                    ui.selectable_value(&mut dialog.encrypted_crc_policy, EncryptedCrcPolicy::OnAccess, EncryptedCrcPolicy::OnAccess.display_name());
                });

            ui.add_space(6.0);

            // Info / Error
            if !dialog.info.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(56, 142, 60), &dialog.info);
            }
            if !dialog.error.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error);
            }

            ui.add_space(10.0);

            // Actions
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Primary actions
                let save_btn = egui::Button::new(egui::RichText::new("Save").strong())
                    .min_size(egui::vec2(100.0, 32.0));
                let cancel_btn = egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0));
                if ui.add(cancel_btn).clicked() {
                    result = Some(PreferencesDialogResult::Cancel);
                }
                if ui.add(save_btn).clicked() {
                    let key_opt = if dialog.key_file_path.trim().is_empty() { None } else { Some(dialog.key_file_path.trim().to_string()) };
                    let db_opt  = if dialog.secrets_db_path.trim().is_empty() { None } else { Some(dialog.secrets_db_path.trim().to_string()) };
                    let policy_opt = Some(dialog.encrypted_crc_policy.as_str().to_string());
                    result = Some(PreferencesDialogResult::Save { key_file_path: key_opt, secrets_db_path: db_opt, encrypted_crc_policy: policy_opt });
                }

                ui.add_space(16.0);

                // Maintenance actions
                let passwords_btn = egui::Button::new("🔐 Manage Passwords…")
                    .min_size(egui::vec2(160.0, 32.0));
                if ui.add(passwords_btn).clicked() {
                    result = Some(PreferencesDialogResult::ManagePasswords);
                }

                let rekey_btn = egui::Button::new("Rekey vault…").min_size(egui::vec2(132.0, 32.0));
                if ui.add(rekey_btn).clicked() {
                    if let Some(file) = rfd::FileDialog::new().pick_file() {
                        result = Some(PreferencesDialogResult::RekeyVault { new_key_file_path: file.to_string_lossy().to_string() });
                    }
                }

                let move_btn = egui::Button::new("Move vault…").min_size(egui::vec2(132.0, 32.0));
                if ui.add(move_btn).clicked() {
                    if let Some(path) = rfd::FileDialog::new().set_file_name("pass.redb").save_file() {
                        result = Some(PreferencesDialogResult::MoveVault { dest_path: path.to_string_lossy().to_string() });
                    }
                }
            });
        });
    });

    result
}

// ================= Password Rules Management Dialog =================

#[derive(Clone)]
pub struct PasswordRule {
    pub name: String,
    pub pattern: String,
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
}

pub struct PasswordRulesDialog {
    pub show: bool,
    pub rules: Vec<PasswordRule>,
    pub editing_index: Option<usize>,
    pub edit_name: String,
    pub edit_pattern: String,
    pub edit_password: String,
    pub edit_priority: String,
    pub edit_enabled: bool,
    pub error: String,
}

impl Default for PasswordRulesDialog {
    fn default() -> Self {
        Self {
            show: false,
            rules: Vec::new(),
            editing_index: None,
            edit_name: String::new(),
            edit_pattern: String::new(),
            edit_password: String::new(),
            edit_priority: "10".to_string(),
            edit_enabled: true,
            error: String::new(),
        }
    }
}

pub enum PasswordRulesResult {
    Save { rules: Vec<PasswordRule> },
    Cancel,
}

pub fn render_password_rules_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
) -> Option<PasswordRulesResult> {
    if !dialog.show {
        return None;
    }
    let mut result = None;

    // Dim overlay
    egui::Area::new(egui::Id::new("pass_rules_overlay_dim"))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
            let _ = ui.allocate_rect(screen, egui::Sense::click());
        });

    // Modal
    egui::Area::new(egui::Id::new("pass_rules_modal"))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            let width = (screen.width() * 0.7).clamp(680.0, 1000.0);
            let height = (screen.height() * 0.7).clamp(500.0, 800.0);
            let pos = egui::pos2(
                (screen.width() - width) / 2.0,
                (screen.height() - height) / 2.0,
            );
            let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

            ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
            ui.painter().rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(1.0, theme.colors.border_color),
            );

            let content = rect.shrink2(egui::vec2(20.0, 16.0));
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );

            child.vertical(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

                // Title
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🔐 Password Rules")
                            .size(18.0)
                            .strong()
                            .color(theme.colors.text_primary),
                    );
                    ui.label(
                        egui::RichText::new("— manage encrypted archive passwords")
                            .size(12.0)
                            .color(theme.colors.text_secondary),
                    );
                });

                ui.add_space(8.0);

                // Rules list
                ui.label(
                    egui::RichText::new("Saved password rules")
                        .size(13.0)
                        .color(theme.colors.text_secondary),
                );

                let mut to_delete: Option<usize> = None;
                let mut to_edit: Option<usize> = None;
                let mut enable_toggles: Vec<(usize, bool)> = Vec::new();

                egui::ScrollArea::vertical()
                    .max_height(content.height() - 360.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 6.0);

                        if dialog.rules.is_empty() {
                            ui.label(
                                egui::RichText::new("No password rules configured yet")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary)
                                    .italics(),
                            );
                        } else {
                            for (idx, rule) in dialog.rules.iter().enumerate() {
                                let bg_color = if idx % 2 == 0 {
                                    theme.colors.bg_secondary
                                } else {
                                    theme.colors.bg_tertiary
                                };

                                egui::Frame::none()
                                    .fill(bg_color)
                                    .inner_margin(8.0)
                                    .rounding(4.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Enabled checkbox
                                            let mut enabled = rule.enabled;
                                            if ui.checkbox(&mut enabled, "").changed() {
                                                enable_toggles.push((idx, enabled));
                                            }

                                            ui.vertical(|ui| {
                                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
                                                ui.label(
                                                    egui::RichText::new(&rule.name)
                                                        .size(13.0)
                                                        .strong()
                                                        .color(if rule.enabled {
                                                            theme.colors.text_primary
                                                        } else {
                                                            theme.colors.text_secondary
                                                        }),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Pattern: {} • Priority: {}",
                                                        rule.pattern, rule.priority
                                                    ))
                                                    .size(11.0)
                                                    .color(theme.colors.text_secondary),
                                                );
                                            });

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui
                                                        .button(
                                                            egui::RichText::new("🗑")
                                                                .size(14.0),
                                                        )
                                                        .on_hover_text("Delete rule")
                                                        .clicked()
                                                    {
                                                        to_delete = Some(idx);
                                                    }

                                                    if ui
                                                        .button(
                                                            egui::RichText::new("✏")
                                                                .size(14.0),
                                                        )
                                                        .on_hover_text("Edit rule")
                                                        .clicked()
                                                    {
                                                        to_edit = Some(idx);
                                                    }
                                                },
                                            );
                                        });
                                    });
                            }
                        }
                    });

                // Apply actions after immutable borrow ends
                if let Some(idx) = to_delete {
                    dialog.rules.remove(idx);
                }
                if let Some(idx) = to_edit {
                    if let Some(rule) = dialog.rules.get(idx) {
                        dialog.editing_index = Some(idx);
                        dialog.edit_name = rule.name.clone();
                        dialog.edit_pattern = rule.pattern.clone();
                        dialog.edit_password = rule.password.clone();
                        dialog.edit_priority = rule.priority.to_string();
                        dialog.edit_enabled = rule.enabled;
                    }
                }
                for (idx, enabled) in enable_toggles {
                    if let Some(r) = dialog.rules.get_mut(idx) {
                        r.enabled = enabled;
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Edit form
                let form_title = if dialog.editing_index.is_some() {
                    "Edit password rule"
                } else {
                    "Add new password rule"
                };
                ui.label(
                    egui::RichText::new(form_title)
                        .size(13.0)
                        .color(theme.colors.text_secondary),
                );

                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.add_sized(
                        [200.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.edit_name)
                            .hint_text("e.g., Work archives"),
                    );

                    ui.add_space(12.0);
                    ui.label("Pattern:");
                    ui.add_sized(
                        [200.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.edit_pattern)
                            .hint_text("e.g., work/*.7z"),
                    );
                });

                ui.horizontal(|ui| {
                    ui.label("Password:");
                    ui.add_sized(
                        [200.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.edit_password)
                            .password(true)
                            .hint_text("Archive password"),
                    );

                    ui.add_space(12.0);
                    ui.label("Priority:");
                    ui.add_sized(
                        [60.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.edit_priority)
                            .hint_text("10"),
                    );

                    ui.add_space(12.0);
                    ui.checkbox(&mut dialog.edit_enabled, "Enabled");
                });

                ui.horizontal(|ui| {
                    let can_save = !dialog.edit_pattern.trim().is_empty()
                        && !dialog.edit_password.is_empty();

                    if ui
                        .add_enabled(
                            can_save,
                            egui::Button::new(if dialog.editing_index.is_some() {
                                "Update"
                            } else {
                                "Add"
                            })
                            .min_size(egui::vec2(80.0, 28.0)),
                        )
                        .clicked()
                    {
                        let priority = dialog.edit_priority.parse::<u32>().unwrap_or(10);
                        let new_rule = PasswordRule {
                            name: if dialog.edit_name.trim().is_empty() {
                                dialog.edit_pattern.clone()
                            } else {
                                dialog.edit_name.clone()
                            },
                            pattern: dialog.edit_pattern.clone(),
                            password: dialog.edit_password.clone(),
                            priority,
                            enabled: dialog.edit_enabled,
                        };

                        if let Some(idx) = dialog.editing_index {
                            dialog.rules[idx] = new_rule;
                            dialog.editing_index = None;
                        } else {
                            dialog.rules.push(new_rule);
                        }

                        // Clear form
                        dialog.edit_name.clear();
                        dialog.edit_pattern.clear();
                        dialog.edit_password.clear();
                        dialog.edit_priority = "10".to_string();
                        dialog.edit_enabled = true;
                    }

                    if dialog.editing_index.is_some() {
                        if ui.button("Cancel Edit").clicked() {
                            dialog.editing_index = None;
                            dialog.edit_name.clear();
                            dialog.edit_pattern.clear();
                            dialog.edit_password.clear();
                            dialog.edit_priority = "10".to_string();
                            dialog.edit_enabled = true;
                        }
                    }
                });

                if !dialog.error.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error);
                }

                ui.add_space(10.0);

                // Bottom actions
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save_btn = egui::Button::new(egui::RichText::new("Save All").strong())
                        .min_size(egui::vec2(100.0, 32.0));
                    let cancel_btn =
                        egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0));

                    if ui.add(cancel_btn).clicked() {
                        result = Some(PasswordRulesResult::Cancel);
                    }
                    if ui.add(save_btn).clicked() {
                        result = Some(PasswordRulesResult::Save {
                            rules: dialog.rules.clone(),
                        });
                    }
                });
            });
        });

    result
}
