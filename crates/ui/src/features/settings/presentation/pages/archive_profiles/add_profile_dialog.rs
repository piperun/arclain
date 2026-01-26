//! Add/Edit Profile Dialog for Archive Profiles
//!
//! Dialog UI for creating and editing archive format profiles.

use arclain_core::features::organization::{ArchiveFormat, ArchiveProfile};
use eframe::egui::{self, Align, Layout, Window};

pub struct AddProfileDialog {
    open: bool,
    profile: ArchiveProfile,
    is_edit: bool,
    name_error: Option<String>,
}

impl Default for AddProfileDialog {
    fn default() -> Self {
        Self {
            open: false,
            profile: ArchiveProfile::default(),
            is_edit: false,
            name_error: None,
        }
    }
}

impl AddProfileDialog {
    pub fn open(&mut self) {
        self.open = true;
        self.is_edit = false;
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
    }

    pub fn edit(&mut self, profile: ArchiveProfile) {
        self.open = true;
        self.is_edit = true;
        self.profile = profile;
        self.name_error = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Returns Some(profile) if the user saved
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        theme: &crate::shared::theme::AppTheme,
    ) -> Option<ArchiveProfile> {
        let mut result = None;
        let mut open = self.open;
        let mut close_requested = false;

        Window::new(if self.is_edit {
            "Edit Profile"
        } else {
            "Add Profile"
        })
        .open(&mut open)
        .resize(|r| r.fixed_size((450.0, 500.0)))
        .collapsible(false)
        .show(ctx, |ui| {
            ui.add_space(8.0);

            // Basic Info
            ui.heading("Profile Information");
            ui.add_space(8.0);

            egui::Grid::new("profile_basic_info")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name:");
                    ui.add(egui::TextEdit::singleline(&mut self.profile.name).desired_width(280.0));
                    ui.end_row();

                    if let Some(err) = &self.name_error {
                        ui.label("");
                        ui.colored_label(egui::Color32::RED, err);
                        ui.end_row();
                    }

                    ui.label("Description:");
                    let mut desc = self.profile.description.clone().unwrap_or_default();
                    if ui.add(egui::TextEdit::singleline(&mut desc).desired_width(280.0)).changed() {
                        self.profile.description = if desc.is_empty() { None } else { Some(desc) };
                    }
                    ui.end_row();
                });

            ui.add_space(16.0);
            ui.separator();

            // Format Settings
            ui.heading("Format Settings");
            ui.add_space(8.0);

            egui::Grid::new("profile_format")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Output Format:");
                    egui::ComboBox::from_id_salt("format_combo")
                        .selected_text(self.profile.format.display_name())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for format in ArchiveFormat::all() {
                                let selected = self.profile.format == *format;
                                if ui.selectable_label(selected, format.display_name()).clicked() {
                                    self.profile.format = *format;
                                    // Update compression method to default for new format
                                    self.profile.compression_method = Some(
                                        self.profile.default_compression_method().to_string()
                                    );
                                }
                            }
                        });
                    ui.end_row();

                    ui.label("Compression Method:");
                    egui::ComboBox::from_id_salt("method_combo")
                        .selected_text(
                            self.profile
                                .compression_method
                                .as_deref()
                                .unwrap_or("Default"),
                        )
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for method in self.profile.available_compression_methods() {
                                let selected = self
                                    .profile
                                    .compression_method
                                    .as_deref()
                                    == Some(*method);
                                if ui.selectable_label(selected, *method).clicked() {
                                    self.profile.compression_method = Some(method.to_string());
                                }
                            }
                        });
                    ui.end_row();
                });

            ui.add_space(16.0);
            ui.separator();

            // Compression Settings
            ui.heading("Compression Settings");
            ui.add_space(8.0);

            egui::Grid::new("profile_compression")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Compression Level:");
                    ui.horizontal(|ui| {
                        let mut level = self.profile.compression_level as i32;
                        if ui.add(egui::Slider::new(&mut level, 0..=9).show_value(true)).changed() {
                            self.profile.compression_level = level as u8;
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
            if self.profile.format == ArchiveFormat::SevenZ {
                ui.add_space(8.0);
                ui.checkbox(&mut self.profile.solid_archive, "Create solid archive")
                    .on_hover_text("Solid archives have better compression but slower random access");

                ui.checkbox(&mut self.profile.encrypt_headers, "Encrypt file headers")
                    .on_hover_text("Hide file names when password-protected (7z only)");
            }

            ui.add_space(16.0);
            ui.separator();

            // Default checkbox
            ui.add_space(8.0);
            ui.checkbox(&mut self.profile.is_default, "Set as default profile")
                .on_hover_text("This profile will be pre-selected when organizing archives");

            // Warning for system profiles
            if self.profile.is_system && self.is_edit {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Note: This is a system profile. Changes will be saved but the profile cannot be deleted.")
                        .size(11.0)
                        .color(theme.colors.on_surface_variant),
                );
            }

            ui.add_space(20.0);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let can_save = !self.profile.name.trim().is_empty();
                if ui
                    .add_enabled(
                        can_save,
                        arclain_widgets::button::TextButton::new(
                            "Save",
                            arclain_widgets::button::ButtonSize::Medium,
                        )
                        .variant(arclain_theme::ButtonVariant::Primary)
                        .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    if self.profile.name.trim().is_empty() {
                        self.name_error = Some("Name is required".to_string());
                    } else {
                        result = Some(self.profile.clone());
                        close_requested = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    close_requested = true;
                }
            });
        });

        if close_requested {
            self.open = false;
        } else {
            self.open = open;
        }

        result
    }
}
