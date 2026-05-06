//! Archive Profiles Settings Page
//!
//! CRUD interface for managing archive format profiles. Part of the Settings feature.

mod add_profile_dialog;

use add_profile_dialog::AddProfileDialog;
use arclain_core::features::organization::ArchiveProfile;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::components::Form;
use crate::shared::SharedState;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;
use std::cell::Cell;

pub struct ProfilesPage {
    profiles: Option<Vec<ArchiveProfile>>,
    dialog: AddProfileDialog,
    error: Option<String>,
}

impl Default for ProfilesPage {
    fn default() -> Self {
        Self {
            profiles: None,
            dialog: AddProfileDialog::default(),
            error: None,
        }
    }
}

impl ProfilesPage {
    pub fn new() -> Self {
        Self::default()
    }

    fn refresh_profiles(&mut self, shared: &SharedState) {
        // Load profiles from database via config pool
        let state = shared.app_state.lock();
        if let Some(dbs) = &state.dbs {
            let pool = &dbs.config_pool;
            match pool.get() {
                Ok(mut conn) => {
                    match arclain_core::list_profiles_diesel(&mut conn) {
                        Ok(db_profiles) => {
                            self.profiles = Some(
                                db_profiles
                                    .iter()
                                    .map(ArchiveProfile::from_db)
                                    .collect(),
                            );
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(format!("Failed to load profiles: {}", e));
                        }
                    }
                }
                Err(e) => {
                    self.error = Some(format!("Database connection error: {}", e));
                }
            }
        } else {
            self.error = Some("Database not available".to_string());
        }
    }

    fn save_profile(&mut self, profile: &ArchiveProfile, shared: &SharedState) -> Result<(), String> {
        let state = shared.app_state.lock();
        if let Some(dbs) = &state.dbs {
            let pool = &dbs.config_pool;
            match pool.get() {
                Ok(mut conn) => {
                    let db_profile = profile.to_db();
                    arclain_core::save_profile_diesel(&mut conn, &db_profile)
                        .map_err(|e| format!("Failed to save profile: {}", e))?;
                    Ok(())
                }
                Err(e) => Err(format!("Database connection error: {}", e)),
            }
        } else {
            Err("Database not available".to_string())
        }
    }

    fn delete_profile(&mut self, id: i64, shared: &SharedState) -> Result<(), String> {
        let state = shared.app_state.lock();
        if let Some(dbs) = &state.dbs {
            let pool = &dbs.config_pool;
            match pool.get() {
                Ok(mut conn) => {
                    arclain_core::delete_profile_diesel(&mut conn, id as i32)
                        .map_err(|e| format!("Failed to delete profile: {}", e))?;
                    Ok(())
                }
                Err(e) => Err(format!("Database connection error: {}", e)),
            }
        } else {
            Err("Database not available".to_string())
        }
    }

    fn set_default_profile(&mut self, id: i64, shared: &SharedState) -> Result<(), String> {
        let state = shared.app_state.lock();
        if let Some(dbs) = &state.dbs {
            let pool = &dbs.config_pool;
            match pool.get() {
                Ok(mut conn) => {
                    arclain_core::set_default_profile_diesel(&mut conn, id as i32)
                        .map_err(|e| format!("Failed to set default: {}", e))?;
                    Ok(())
                }
                Err(e) => Err(format!("Database connection error: {}", e)),
            }
        } else {
            Err("Database not available".to_string())
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        shared: &SharedState,
    ) {
        if self.profiles.is_none() {
            self.refresh_profiles(shared);
        }

        // Use Cell to track "set default" clicks from inside the closure
        let set_default_idx: Cell<Option<usize>> = Cell::new(None);

        Form::new()
            .id("archive_profiles")
            .show(ui, theme, |ui| {
                // Page header
                ui.label(
                    egui::RichText::new("Archive Profiles")
                        .size(18.0)
                        .strong()
                        .color(theme.colors.on_surface),
                );
                ui.label(
                    egui::RichText::new("Manage compression profiles for archive organization. Select a profile when organizing to control output format and compression settings.")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Header with count and Add button
                ui.horizontal(|ui| {
                    if ui.add(TextButton::new(format!("{} Add New Profile", egui_phosphor::regular::PLUS), ButtonSize::Medium).with_theme_colors(&theme.colors)).clicked() {
                        self.dialog.open();
                    }

                    if let Some(profiles) = &self.profiles {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{} profiles", profiles.len()))
                                    .size(12.0)
                                    .color(theme.colors.on_surface_variant),
                            );
                        });
                    }
                });

                // Error display
                if let Some(err) = &self.error {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::RED, err);
                }

                ui.add_space(12.0);

                // Table
                let actions = if let Some(profiles) = &self.profiles {
                    let columns = vec![
                        TableColumn::exact(50.0, "Default"),
                        TableColumn::resizable(200.0, "Name"),
                        TableColumn::exact(60.0, "Format"),
                        TableColumn::exact(50.0, "Level"),
                        TableColumn::remainder("Description"),
                        TableColumn::exact(120.0, "Actions").align_right(),
                    ];

                    ItemTable::new()
                        .empty_message("No archive profiles configured yet.")
                        .show(ui, theme, &columns, profiles, |profile, idx, row, actions| {
                            // Default column
                            row.col(|ui| {
                                if profile.is_default {
                                    ui.label(
                                        egui::RichText::new(egui_phosphor::regular::STAR)
                                            .color(theme.colors.primary),
                                    );
                                }
                            });

                            // Name column
                            row.col(|ui| {
                                let color = if profile.is_system {
                                    theme.colors.on_surface_variant
                                } else {
                                    theme.colors.on_surface
                                };
                                ui.label(egui::RichText::new(&profile.name).color(color));
                            });

                            // Format column
                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(profile.format.display_name())
                                        .family(egui::FontFamily::Monospace)
                                        .color(theme.colors.on_surface_variant),
                                );
                            });

                            // Level column
                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{}", profile.compression_level))
                                        .family(egui::FontFamily::Monospace)
                                        .color(theme.colors.on_surface_variant),
                                );
                            });

                            // Description column
                            row.col(|ui| {
                                if let Some(desc) = &profile.description {
                                    ui.label(
                                        egui::RichText::new(desc)
                                            .size(11.0)
                                            .color(theme.colors.on_surface_variant),
                                    );
                                }
                            });

                            // Actions column
                            row.col(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Delete button (only for non-system profiles)
                                    if !profile.is_system {
                                        if ui
                                            .add(TextButton::new(format!("{}", egui_phosphor::regular::TRASH), ButtonSize::Small).with_theme_colors(&theme.colors))
                                            .on_hover_text("Delete profile")
                                            .clicked()
                                        {
                                            actions.delete(idx);
                                        }
                                        ui.add_space(4.0);
                                    }

                                    // Edit button
                                    if ui
                                        .add(TextButton::new(format!("{}", egui_phosphor::regular::PENCIL), ButtonSize::Small).with_theme_colors(&theme.colors))
                                        .on_hover_text("Edit profile")
                                        .clicked()
                                    {
                                        actions.edit(idx);
                                    }

                                    ui.add_space(4.0);

                                    // Set default button (only if not already default)
                                    if !profile.is_default {
                                        if ui
                                            .add(TextButton::new(format!("{}", egui_phosphor::regular::STAR), ButtonSize::Small).with_theme_colors(&theme.colors))
                                            .on_hover_text("Set as default")
                                            .clicked()
                                        {
                                            set_default_idx.set(Some(idx));
                                        }
                                    }
                                });
                            });
                        })
                } else {
                    let empty_profiles: Vec<ArchiveProfile> = Vec::new();
                    ItemTable::new().show(ui, theme, &[], &empty_profiles, |_, _, _, _| {})
                };

                // Handle deferred actions
                if let Some(edit_idx) = actions.get_edit() {
                    if let Some(profiles) = &self.profiles {
                        if let Some(profile) = profiles.get(*edit_idx) {
                            self.dialog.edit(profile.clone());
                        }
                    }
                }

                // Handle delete action
                if let Some(delete_idx) = actions.get_delete() {
                    if let Some(profiles) = &self.profiles {
                        if let Some(profile) = profiles.get(*delete_idx) {
                            if !profile.is_system {
                                if let Err(e) = self.delete_profile(profile.id, shared) {
                                    self.error = Some(e);
                                } else {
                                    self.profiles = None; // Trigger refresh
                                }
                            }
                        }
                    }
                }
            });

        // Handle set default action (deferred from closure)
        if let Some(idx) = set_default_idx.get() {
            if let Some(profiles) = &self.profiles {
                if let Some(profile) = profiles.get(idx) {
                    if let Err(e) = self.set_default_profile(profile.id, shared) {
                        self.error = Some(e);
                    } else {
                        self.profiles = None; // Trigger refresh
                    }
                }
            }
        }

        // Handle Dialog
        if self.dialog.is_open() {
            if let Some(new_profile) = self.dialog.show(ui.ctx(), theme) {
                if let Err(e) = self.save_profile(&new_profile, shared) {
                    self.error = Some(e);
                } else {
                    self.profiles = None; // Trigger refresh
                }
            }
        }
    }
}
