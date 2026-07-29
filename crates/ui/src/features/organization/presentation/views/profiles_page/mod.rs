//! Archive Profiles Page
//!
//! CRUD interface for managing archive format profiles. Owned by the Organization
//! feature (profiles are organization-domain configuration).
//!
//! Architecture: render emits `Option<ProfilesAction>` and never
//! touches the DB. The sibling `handle_profiles_action` function
//! owns all persistence side effects, so adding logging, retry, or
//! audit-trail concerns only requires editing one place.

mod add_profile_dialog;

use crate::features::organization::application::facade;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::components::Form;
use crate::shared::SharedState;
use add_profile_dialog::AddProfileDialog;
use arclain_app::organization::{
    archive_format_options, ArchiveFormatOptionDto, OrganizationProfileInput,
    OrganizationProfileSummary,
};
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;
use std::cell::Cell;

/// How an output-format token is labelled wherever a profile is shown.
/// Falls back to the token itself for a stored format the application no
/// longer offers -- rendering the raw value is more honest than hiding
/// a row's real format behind a guess.
pub fn format_label(token: &str) -> String {
    format_option(token)
        .map(|option| option.display_name)
        .unwrap_or_else(|| token.to_string())
}

/// The extension an output packed with `token` gets, with the same
/// fallback [`format_label`] makes.
pub fn format_extension(token: &str) -> String {
    format_option(token)
        .map(|option| option.extension)
        .unwrap_or_else(|| token.to_string())
}

/// The offered format matching `token`, if the application still offers
/// it.
fn format_option(token: &str) -> Option<ArchiveFormatOptionDto> {
    archive_format_options()
        .into_iter()
        .find(|option| option.token.eq_ignore_ascii_case(token))
}

/// Intents emitted by `ProfilesPage::render`. The dispatcher
/// (`handle_profiles_action`) owns all persistence; render is pure
/// intent-emission.
#[derive(Debug, Clone)]
pub enum ProfilesAction {
    /// First-time load or refresh after a mutation. Fired automatically
    /// from render when `page.profiles` is `None`.
    LoadProfiles,
    /// Persist a new or edited profile. The dispatcher upserts and
    /// re-fetches the list.
    SaveProfile(OrganizationProfileInput),
    /// Delete the profile with the given id.
    DeleteProfile(String),
    /// Mark the profile with the given id as default.
    SetDefaultProfile(String),
}

pub struct ProfilesPage {
    profiles: Option<Vec<OrganizationProfileSummary>>,
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

    /// Currently surfaced error message, if any. Used by integration
    /// tests to assert dispatcher behavior; the render path reads
    /// `self.error` directly.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Borrow the cached profiles list. `None` until the dispatcher
    /// has run `LoadProfiles` at least once.
    pub fn profiles(&self) -> Option<&[OrganizationProfileSummary]> {
        self.profiles.as_deref()
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
    ) -> Option<ProfilesAction> {
        // First render (or after a mutation that invalidated the cache):
        // emit a Load action and show a placeholder. The dispatcher
        // populates `self.profiles` synchronously after render returns;
        // the next frame shows real data.
        if self.profiles.is_none() {
            ui.label(
                egui::RichText::new("Loading profiles…")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            return Some(ProfilesAction::LoadProfiles);
        }

        let mut emitted: Option<ProfilesAction> = None;

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
                                    egui::RichText::new(format_label(&profile.output_format))
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
                                            .add(TextButton::new(egui_phosphor::regular::TRASH, ButtonSize::Small).with_theme_colors(&theme.colors))
                                            .on_hover_text("Delete profile")
                                            .clicked()
                                        {
                                            actions.delete(idx);
                                        }
                                        ui.add_space(4.0);
                                    }

                                    // Edit button
                                    if ui
                                        .add(TextButton::new(egui_phosphor::regular::PENCIL, ButtonSize::Small).with_theme_colors(&theme.colors))
                                        .on_hover_text("Edit profile")
                                        .clicked()
                                    {
                                        actions.edit(idx);
                                    }

                                    ui.add_space(4.0);

                                    // Set default button (only if not already default)
                                    if !profile.is_default {
                                        if ui
                                            .add(TextButton::new(egui_phosphor::regular::STAR, ButtonSize::Small).with_theme_colors(&theme.colors))
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
                    let empty_profiles: Vec<OrganizationProfileSummary> = Vec::new();
                    ItemTable::new().show(ui, theme, &[], &empty_profiles, |_, _, _, _| {})
                };

                // Handle edit click → open dialog locally; no action emitted
                // until the user saves.
                if let Some(edit_idx) = actions.get_edit() {
                    if let Some(profiles) = &self.profiles {
                        if let Some(profile) = profiles.get(*edit_idx) {
                            self.dialog.edit(profile);
                        }
                    }
                }

                // Handle delete click → emit DeleteProfile action.
                if let Some(delete_idx) = actions.get_delete() {
                    if let Some(profiles) = &self.profiles {
                        if let Some(profile) = profiles.get(*delete_idx) {
                            if !profile.is_system {
                                emitted = Some(ProfilesAction::DeleteProfile(profile.id.clone()));
                            }
                        }
                    }
                }
            });

        // Handle set-default click (deferred out of the table closure).
        if emitted.is_none() {
            if let Some(idx) = set_default_idx.get() {
                if let Some(profiles) = &self.profiles {
                    if let Some(profile) = profiles.get(idx) {
                        emitted = Some(ProfilesAction::SetDefaultProfile(profile.id.clone()));
                    }
                }
            }
        }

        // Dialog: open dialog runs its own local rendering loop; if the
        // user saves, we emit a SaveProfile action.
        if self.dialog.is_open() {
            if let Some(new_profile) = self.dialog.show(ui.ctx(), theme) {
                if emitted.is_none() {
                    emitted = Some(ProfilesAction::SaveProfile(new_profile));
                }
            }
        }

        emitted
    }
}

/// Dispatch a `ProfilesAction` against the application facade and
/// update the page's cached state. Called by the parent view
/// (`settings_content.rs`) after `render` returns an action. All side
/// effects live here, so `ProfilesPage::render` itself stays a pure
/// intent-emitter.
///
/// Every mutating facade call answers with the full post-write list, so
/// a mutation and the refresh that follows it are one round trip and
/// the page cannot render a list that a concurrent write has already
/// invalidated.
pub fn handle_profiles_action(
    page: &mut ProfilesPage,
    action: ProfilesAction,
    shared: &SharedState,
) {
    let Some((app, runtime)) = facade::handles(shared) else {
        page.error = Some(facade::unavailable());
        return;
    };

    let (context, result) = match action {
        ProfilesAction::LoadProfiles => (
            "Failed to load profiles",
            runtime.block_on(app.organization_profiles()),
        ),
        ProfilesAction::SaveProfile(profile) => (
            "Failed to save profile",
            runtime.block_on(app.upsert_organization_profile(profile)),
        ),
        ProfilesAction::DeleteProfile(id) => (
            "Failed to delete profile",
            runtime.block_on(app.delete_organization_profile(id)),
        ),
        ProfilesAction::SetDefaultProfile(id) => (
            "Failed to set default",
            runtime.block_on(app.set_default_organization_profile(id)),
        ),
    };

    match result {
        Ok(profiles) => {
            page.profiles = Some(profiles);
            page.error = None;
        }
        Err(error) => {
            page.error = Some(facade::describe(context, &error));
        }
    }
}
