//! Reusable Form Dialog Component
//!
//! A standardized dialog for CRUD operations with support for:
//! - Draggable window mode (free moving)
//! - Fixed center modal mode (dimmed background)
//! - Standard Save/Cancel button bar
//! - Edit/Add title switching

use crate::shared::theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

/// Dialog positioning mode
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum DialogMode {
    /// Draggable window that can be moved freely
    #[default]
    Draggable,
    /// Fixed in center with dimmed background overlay
    FixedCenter,
}

/// Configuration for the form dialog
pub struct FormDialogConfig {
    /// Title shown when adding a new item
    pub add_title: String,
    /// Title shown when editing an existing item
    pub edit_title: String,
    /// Dialog positioning mode
    pub mode: DialogMode,
    /// Dialog width (for Draggable mode, this is fixed size; for FixedCenter, this is the width)
    pub width: f32,
    /// Dialog height
    pub height: f32,
    /// Whether the dialog can be resized (Draggable mode only)
    pub resizable: bool,
    /// Overlay alpha for FixedCenter mode (0-255)
    pub overlay_alpha: u8,
}

impl Default for FormDialogConfig {
    fn default() -> Self {
        Self {
            add_title: "Add Item".to_string(),
            edit_title: "Edit Item".to_string(),
            mode: DialogMode::Draggable,
            width: 500.0,
            height: 600.0,
            resizable: false,
            overlay_alpha: 180,
        }
    }
}

impl FormDialogConfig {
    pub fn new(add_title: impl Into<String>, edit_title: impl Into<String>) -> Self {
        Self {
            add_title: add_title.into(),
            edit_title: edit_title.into(),
            ..Default::default()
        }
    }

    pub fn mode(mut self, mode: DialogMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn overlay_alpha(mut self, alpha: u8) -> Self {
        self.overlay_alpha = alpha;
        self
    }
}

/// Result from the form dialog
pub enum FormDialogResult<T> {
    /// User saved with the provided data
    Save(T),
    /// User cancelled
    Cancel,
    /// Dialog is still open, no action yet
    None,
}

/// A reusable form dialog for CRUD operations
pub struct FormDialog {
    open: bool,
    is_edit: bool,
    config: FormDialogConfig,
}

impl FormDialog {
    pub fn new(config: FormDialogConfig) -> Self {
        Self {
            open: false,
            is_edit: false,
            config,
        }
    }

    /// Open the dialog in "Add" mode
    pub fn open_add(&mut self) {
        self.open = true;
        self.is_edit = false;
    }

    /// Open the dialog in "Edit" mode
    pub fn open_edit(&mut self) {
        self.open = true;
        self.is_edit = true;
    }

    /// Close the dialog
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Check if dialog is open
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Check if dialog is in edit mode
    pub fn is_edit(&self) -> bool {
        self.is_edit
    }

    /// Get the current title based on mode
    pub fn title(&self) -> &str {
        if self.is_edit {
            &self.config.edit_title
        } else {
            &self.config.add_title
        }
    }

    /// Show the dialog and return the result
    ///
    /// # Arguments
    /// * `ctx` - egui context
    /// * `theme` - application theme
    /// * `can_save` - whether the save button should be enabled
    /// * `content` - closure to render the form content
    ///
    /// # Returns
    /// `FormDialogResult` indicating user action
    pub fn show<T, F>(
        &mut self,
        ctx: &egui::Context,
        theme: &AppTheme,
        can_save: bool,
        content: F,
    ) -> FormDialogResult<T>
    where
        F: FnOnce(&mut egui::Ui) -> Option<T>,
    {
        if !self.open {
            return FormDialogResult::None;
        }

        match self.config.mode {
            DialogMode::Draggable => self.show_draggable(ctx, theme, can_save, content),
            DialogMode::FixedCenter => self.show_fixed_center(ctx, theme, can_save, content),
        }
    }

    fn show_draggable<T, F>(
        &mut self,
        ctx: &egui::Context,
        theme: &AppTheme,
        can_save: bool,
        content: F,
    ) -> FormDialogResult<T>
    where
        F: FnOnce(&mut egui::Ui) -> Option<T>,
    {
        let mut result = FormDialogResult::None;
        let mut open = self.open;
        let mut close_requested = false;
        let mut save_data: Option<T> = None;

        egui::Window::new(self.title())
            .open(&mut open)
            .resizable(self.config.resizable)
            .default_size([self.config.width, self.config.height])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);

                // Content area with scroll
                egui::ScrollArea::vertical()
                    .max_height(self.config.height - 80.0)
                    .show(ui, |ui| {
                        save_data = content(ui);
                    });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                // Button bar
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            can_save,
                            TextButton::new("Save", ButtonSize::Medium)
                                .variant(arclain_theme::ButtonVariant::Primary)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
                        if let Some(data) = save_data.take() {
                            result = FormDialogResult::Save(data);
                            close_requested = true;
                        }
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            TextButton::new("Cancel", ButtonSize::Medium)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
                        result = FormDialogResult::Cancel;
                        close_requested = true;
                    }
                });
            });

        if close_requested || !open {
            self.open = false;
            if matches!(result, FormDialogResult::None) {
                result = FormDialogResult::Cancel;
            }
        }

        result
    }

    fn show_fixed_center<T, F>(
        &mut self,
        ctx: &egui::Context,
        theme: &AppTheme,
        can_save: bool,
        content: F,
    ) -> FormDialogResult<T>
    where
        F: FnOnce(&mut egui::Ui) -> Option<T>,
    {
        let mut result = FormDialogResult::None;
        let mut close_requested = false;
        let mut save_data: Option<T> = None;

        let id_prefix = format!("form_dialog_{}", self.title().replace(' ', "_"));

        // Dimmed overlay
        egui::Area::new(egui::Id::new(format!("{}_overlay", id_prefix)))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let screen = ctx.viewport_rect();
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    egui::Color32::from_black_alpha(self.config.overlay_alpha),
                );
                // Block interaction with background
                if ui
                    .allocate_rect(screen, egui::Sense::click())
                    .clicked_elsewhere()
                {
                    // Clicking outside doesn't close by default, but could be made optional
                }
            });

        // Dialog
        egui::Area::new(egui::Id::new(format!("{}_dialog", id_prefix)))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(theme.colors.surface)
                    .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                    .corner_radius(8.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.set_width(self.config.width);
                        ui.set_max_height(self.config.height);

                        // Title
                        ui.label(
                            egui::RichText::new(self.title())
                                .size(18.0)
                                .strong()
                                .color(theme.colors.on_surface),
                        );
                        ui.add_space(16.0);

                        // Content area with scroll
                        egui::ScrollArea::vertical()
                            .max_height(self.config.height - 120.0)
                            .show(ui, |ui| {
                                save_data = content(ui);
                            });

                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(12.0);

                        // Button bar
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    can_save,
                                    TextButton::new("Save", ButtonSize::Medium)
                                        .variant(arclain_theme::ButtonVariant::Primary)
                                        .with_theme_colors(&theme.colors),
                                )
                                .clicked()
                            {
                                if let Some(data) = save_data.take() {
                                    result = FormDialogResult::Save(data);
                                    close_requested = true;
                                }
                            }

                            ui.add_space(8.0);

                            if ui
                                .add(
                                    TextButton::new("Cancel", ButtonSize::Medium)
                                        .with_theme_colors(&theme.colors),
                                )
                                .clicked()
                            {
                                result = FormDialogResult::Cancel;
                                close_requested = true;
                            }
                        });
                    });
            });

        // Handle Escape key
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_requested = true;
            result = FormDialogResult::Cancel;
        }

        if close_requested {
            self.open = false;
        }

        result
    }
}
