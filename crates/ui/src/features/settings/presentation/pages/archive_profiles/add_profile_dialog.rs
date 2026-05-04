//! Add/Edit Profile Dialog for Archive Profiles
//!
//! Dialog UI for creating and editing archive format profiles.

use arclain_core::features::organization::{ArchiveFormat, ArchiveProfile};
use arclain_widgets::{TextInput, ThemedDropdown};
use crate::shared::components::settings_form::SectionHeader;
use crate::shared::dialogs::{DialogMode, FormDialog, FormDialogConfig, FormDialogResult};
use eframe::egui;

pub struct AddProfileDialog {
    dialog: FormDialog,
    profile: ArchiveProfile,
    name_error: Option<String>,
}

impl Default for AddProfileDialog {
    fn default() -> Self {
        let config = FormDialogConfig::new("Add Profile", "Edit Profile")
            .mode(DialogMode::Draggable)
            .size(450.0, 480.0);

        Self {
            dialog: FormDialog::new(config),
            profile: ArchiveProfile::default(),
            name_error: None,
        }
    }
}

impl AddProfileDialog {
    pub fn open(&mut self) {
        self.profile = ArchiveProfile {
            id: 0,
            name: String::new(),
            description: None,
            format: ArchiveFormat::SevenZ,
            compression_level: 5,
            compression_method: Some("LZMA2".to_string()),
            solid_archive: true,
            encrypt_headers: false,
            is_default: false,
            is_system: false,
        };
        self.name_error = None;
        self.dialog.open_add();
    }

    pub fn edit(&mut self, profile: ArchiveProfile) {
        self.profile = profile;
        self.name_error = None;
        self.dialog.open_edit();
    }

    pub fn is_open(&self) -> bool {
        self.dialog.is_open()
    }

    /// Returns Some(profile) if the user saved
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        theme: &crate::shared::theme::AppTheme,
    ) -> Option<ArchiveProfile> {
        let can_save = !self.profile.name.trim().is_empty();

        // Borrow fields separately to avoid borrowing self in the closure
        let profile = &mut self.profile;
        let name_error = &self.name_error;
        let is_edit = self.dialog.is_edit();

        match self.dialog.show(ctx, theme, can_save, |ui| {
            Self::render_form_content(ui, theme, profile, name_error, is_edit);
            Some(profile.clone())
        }) {
            FormDialogResult::Save(profile) => Some(profile),
            FormDialogResult::Cancel => None,
            FormDialogResult::None => None,
        }
    }

    fn render_form_content(
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        profile: &mut ArchiveProfile,
        name_error: &Option<String>,
        is_edit: bool,
    ) {
        // Basic Info
        SectionHeader::new("Profile Information").show(ui, &theme.colors);
        ui.add_space(8.0);

        egui::Grid::new("profile_basic_info")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name:");
                TextInput::new(&mut profile.name)
                    .width(280.0)
                    .with_theme_colors(&theme.colors)
                    .show(ui);
                ui.end_row();

                if let Some(err) = name_error {
                    ui.label("");
                    crate::shared::components::error_label(ui, theme, err);
                    ui.end_row();
                }

                ui.label("Description:");
                let mut desc = profile.description.clone().unwrap_or_default();
                if TextInput::new(&mut desc)
                    .width(280.0)
                    .with_theme_colors(&theme.colors)
                    .show(ui)
                    .changed()
                {
                    profile.description = if desc.is_empty() { None } else { Some(desc) };
                }
                ui.end_row();
            });

        ui.add_space(16.0);
        ui.separator();

        // Format Settings
        SectionHeader::new("Format Settings").show(ui, &theme.colors);
        ui.add_space(8.0);

        egui::Grid::new("profile_format")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Output Format:");
                ThemedDropdown::new("format_combo", profile.format.display_name())
                    .with_theme_colors(&theme.colors)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for format in ArchiveFormat::all() {
                            let selected = profile.format == *format;
                            if ui.selectable_label(selected, format.display_name()).clicked() {
                                profile.format = *format;
                                // Update compression method to default for new format
                                profile.compression_method =
                                    Some(profile.default_compression_method().to_string());
                            }
                        }
                    });
                ui.end_row();

                ui.label("Compression Method:");
                ThemedDropdown::new("method_combo", profile.compression_method.as_deref().unwrap_or("Default"))
                    .with_theme_colors(&theme.colors)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for method in profile.available_compression_methods() {
                            let selected = profile.compression_method.as_deref() == Some(*method);
                            if ui.selectable_label(selected, *method).clicked() {
                                profile.compression_method = Some(method.to_string());
                            }
                        }
                    });
                ui.end_row();
            });

        ui.add_space(16.0);
        ui.separator();

        // Compression Settings
        SectionHeader::new("Compression Settings").show(ui, &theme.colors);
        ui.add_space(8.0);

        egui::Grid::new("profile_compression")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Compression Level:");
                ui.horizontal(|ui| {
                    let mut level = profile.compression_level as i32;
                    if ui
                        .add(egui::Slider::new(&mut level, 0..=9).show_value(true))
                        .changed()
                    {
                        profile.compression_level = level as u8;
                    }
                    ui.label(match level {
                        0 => "(Store)",
                        1..=3 => "(Fast)",
                        4..=6 => "(Normal)",
                        7..=9 => "(Maximum)",
                        _ => "",
                    });
                });
                ui.end_row();
            });

        // 7z-specific options
        if profile.format == ArchiveFormat::SevenZ {
            ui.add_space(8.0);
            ui.checkbox(&mut profile.solid_archive, "Create solid archive")
                .on_hover_text("Solid archives have better compression but slower random access");

            ui.checkbox(&mut profile.encrypt_headers, "Encrypt file headers")
                .on_hover_text("Hide file names when password-protected (7z only)");
        }

        ui.add_space(16.0);
        ui.separator();

        // Default checkbox
        ui.add_space(8.0);
        ui.checkbox(&mut profile.is_default, "Set as default profile")
            .on_hover_text("This profile will be pre-selected when organizing archives");

        // Warning for system profiles
        if profile.is_system && is_edit {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Note: This is a system profile. Changes will be saved but the profile cannot be deleted.",
                )
                .size(11.0)
                .color(theme.colors.on_surface_variant),
            );
        }
    }
}
