//! Add/Edit Profile Dialog for Archive Profiles
//!
//! Dialog UI for creating and editing archive format profiles.
//!
//! The form edits the request it will submit
//! (`ArclainApp::upsert_organization_profile`) directly, and builds its
//! format and compression-method pickers from
//! `arclain_app::organization::archive_format_options` -- so the values
//! it can produce are exactly the values the facade accepts, and a
//! format added to the application appears here without a change to
//! this file.

use super::format_label;
use crate::shared::components::settings_form::SectionHeader;
use crate::shared::dialogs::{DialogMode, FormDialog, FormDialogConfig, FormDialogResult};
use arclain_app::organization::{
    archive_format_options, OrganizationProfileInput, OrganizationProfileSummary,
};
use arclain_widgets::{TextInput, ThemedDropdown};
use eframe::egui;

pub struct AddProfileDialog {
    dialog: FormDialog,
    profile: OrganizationProfileInput,
    /// Carried purely to render the "this is a system profile" note.
    /// Deliberately not part of the submitted input: the flag is what
    /// makes a profile undeletable, and the facade preserves the stored
    /// value rather than taking one from a caller.
    is_system: bool,
    name_error: Option<String>,
}

impl Default for AddProfileDialog {
    fn default() -> Self {
        let config = FormDialogConfig::new("Add Profile", "Edit Profile")
            .mode(DialogMode::Draggable)
            .size(450.0, 480.0);

        Self {
            dialog: FormDialog::new(config),
            profile: blank_profile(),
            is_system: false,
            name_error: None,
        }
    }
}

/// A new profile: the application's first offered format at its own
/// default compression method, solid, mid compression.
fn blank_profile() -> OrganizationProfileInput {
    let format = archive_format_options()
        .into_iter()
        .next()
        .expect("the application must offer at least one archive format");
    OrganizationProfileInput {
        id: None,
        name: String::new(),
        description: None,
        output_format: format.token,
        compression_level: 5,
        compression_method: Some(format.default_compression_method),
        solid_archive: true,
        encrypt_headers: false,
        is_default: false,
    }
}

impl AddProfileDialog {
    pub fn open(&mut self) {
        self.profile = blank_profile();
        self.is_system = false;
        self.name_error = None;
        self.dialog.open_add();
    }

    pub fn edit(&mut self, profile: &OrganizationProfileSummary) {
        self.profile = OrganizationProfileInput {
            id: Some(profile.id.clone()),
            name: profile.name.clone(),
            description: profile.description.clone(),
            output_format: profile.output_format.clone(),
            compression_level: profile.compression_level,
            compression_method: profile.compression_method.clone(),
            solid_archive: profile.solid_archive,
            encrypt_headers: profile.encrypt_headers,
            is_default: profile.is_default,
        };
        self.is_system = profile.is_system;
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
    ) -> Option<OrganizationProfileInput> {
        let can_save = !self.profile.name.trim().is_empty();

        // Borrow fields separately to avoid borrowing self in the closure
        let profile = &mut self.profile;
        let name_error = &self.name_error;
        let is_system = self.is_system;
        let is_edit = self.dialog.is_edit();

        match self.dialog.show(ctx, theme, can_save, |ui| {
            Self::render_form_content(ui, theme, profile, name_error, is_system, is_edit);
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
        profile: &mut OrganizationProfileInput,
        name_error: &Option<String>,
        is_system: bool,
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

        let formats = archive_format_options();
        let selected_format = formats
            .iter()
            .find(|option| option.token.eq_ignore_ascii_case(&profile.output_format))
            .cloned();

        egui::Grid::new("profile_format")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Output Format:");
                ThemedDropdown::new("format_combo", format_label(&profile.output_format))
                    .with_theme_colors(&theme.colors)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for format in &formats {
                            let selected =
                                format.token.eq_ignore_ascii_case(&profile.output_format);
                            if ui
                                .selectable_label(selected, &format.display_name)
                                .clicked()
                            {
                                profile.output_format = format.token.clone();
                                // Update compression method to default for new format
                                profile.compression_method =
                                    Some(format.default_compression_method.clone());
                            }
                        }
                    });
                ui.end_row();

                ui.label("Compression Method:");
                ThemedDropdown::new(
                    "method_combo",
                    profile.compression_method.as_deref().unwrap_or("Default"),
                )
                .with_theme_colors(&theme.colors)
                .width(150.0)
                .show_ui(ui, |ui| {
                    let methods = selected_format
                        .as_ref()
                        .map(|format| format.compression_methods.as_slice())
                        .unwrap_or_default();
                    for method in methods {
                        let selected = profile.compression_method.as_deref() == Some(method);
                        if ui.selectable_label(selected, method).clicked() {
                            profile.compression_method = Some(method.clone());
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

        // Options only the chosen container can honor.
        if selected_format
            .as_ref()
            .is_some_and(|format| format.supports_solid_archive)
        {
            ui.add_space(8.0);
            ui.checkbox(&mut profile.solid_archive, "Create solid archive")
                .on_hover_text("Solid archives have better compression but slower random access");
        }
        if selected_format
            .as_ref()
            .is_some_and(|format| format.supports_header_encryption)
        {
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
        if is_system && is_edit {
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
