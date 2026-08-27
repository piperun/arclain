//! ItemTable Component
//!
//! A standardized table component for rendering interactive lists of items
//! with actions (Edit, Delete, etc.) and status indicators.
//!
//! Features:
//! - Y2K styling (zero corner radius, sharp borders)
//! - Striped rows
//! - Resizable columns
//! - Empty state handling
//! - Deferred action pattern (delete/edit after render)
//! - Theme-aware styling

use crate::shared::theme::AppTheme;
use eframe::egui;
use egui_extras::Column;

/// Column definition for ItemTable
pub struct TableColumn {
    pub label: String,
    pub column: Column,
    pub align_right: bool,
}

impl TableColumn {
    /// Create a column with exact width
    pub fn exact(width: f32, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            column: Column::exact(width),
            align_right: false,
        }
    }

    /// Create a resizable column with initial width
    pub fn resizable(initial_width: f32, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            column: Column::initial(initial_width).resizable(true),
            align_right: false,
        }
    }

    /// Create a column that takes remaining space
    pub fn remainder(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            column: Column::remainder().clip(true),
            align_right: false,
        }
    }

    /// Align column content to the right
    pub fn align_right(mut self) -> Self {
        self.align_right = true;
        self
    }
}

/// Actions that can be performed on table items
pub struct TableActions<T> {
    to_delete: Option<T>,
    to_edit: Option<T>,
}

impl<T> TableActions<T> {
    pub fn new() -> Self {
        Self {
            to_delete: None,
            to_edit: None,
        }
    }

    /// Mark an item for deletion (deferred until after render)
    pub fn delete(&mut self, id: T) {
        self.to_delete = Some(id);
    }

    /// Mark an item for editing (deferred until after render)
    pub fn edit(&mut self, id: T) {
        self.to_edit = Some(id);
    }

    /// Get the item marked for deletion
    pub fn get_delete(&self) -> Option<&T> {
        self.to_delete.as_ref()
    }

    /// Get the item marked for editing
    pub fn get_edit(&self) -> Option<&T> {
        self.to_edit.as_ref()
    }

    /// Take the item marked for deletion
    pub fn take_delete(&mut self) -> Option<T> {
        self.to_delete.take()
    }

    /// Take the item marked for editing
    pub fn take_edit(&mut self) -> Option<T> {
        self.to_edit.take()
    }
}

impl<T> Default for TableActions<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for ItemTable
pub struct ItemTable {
    striped: bool,
    min_scrolled_height: f32,
    header_height: f32,
    row_height: f32,
    empty_message: String,
}

impl ItemTable {
    pub fn new() -> Self {
        Self {
            striped: true,
            min_scrolled_height: 0.0,
            header_height: 24.0,
            row_height: 30.0,
            empty_message: "No items".to_string(),
        }
    }

    /// Enable/disable striped rows (default: true)
    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }

    /// Set minimum scrolled height
    pub fn min_scrolled_height(mut self, height: f32) -> Self {
        self.min_scrolled_height = height;
        self
    }

    /// Set header row height (default: 24.0)
    pub fn header_height(mut self, height: f32) -> Self {
        self.header_height = height;
        self
    }

    /// Set row height (default: 30.0)
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height = height;
        self
    }

    /// Set empty state message
    pub fn empty_message(mut self, message: impl Into<String>) -> Self {
        self.empty_message = message.into();
        self
    }

    /// Render the table with the given data
    pub fn show<T, F>(
        self,
        ui: &mut egui::Ui,
        theme: &AppTheme,
        columns: &[TableColumn],
        items: &[T],
        mut render_row: F,
    ) -> TableActions<usize>
    where
        F: FnMut(&T, usize, &mut egui_extras::TableRow, &mut TableActions<usize>),
    {
        let mut actions = TableActions::new();

        if items.is_empty() {
            // Empty state
            egui::Frame::NONE
                .stroke(egui::Stroke::new(1.0_f32, theme.colors.outline))
                .corner_radius(egui::CornerRadius::ZERO) // Y2K: zero radius
                .inner_margin(20.0)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(&self.empty_message)
                                .size(13.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    });
                });
        } else {
            // Table with data
            egui::Frame::NONE
                .stroke(egui::Stroke::new(1.0_f32, theme.colors.outline))
                .corner_radius(egui::CornerRadius::ZERO) // Y2K: zero radius
                .inner_margin(4.0)
                .show(ui, |ui| {
                    use egui_extras::TableBuilder;

                    let mut table = TableBuilder::new(ui)
                        .striped(self.striped)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .min_scrolled_height(self.min_scrolled_height);

                    // Add columns
                    for col in columns {
                        table = table.column(col.column.clone());
                    }

                    table
                        .header(self.header_height, |mut header| {
                            for col in columns {
                                header.col(|ui| {
                                    let layout = if col.align_right {
                                        egui::Layout::right_to_left(egui::Align::Center)
                                    } else {
                                        egui::Layout::left_to_right(egui::Align::Center)
                                    };

                                    ui.with_layout(layout, |ui| {
                                        ui.label(
                                            egui::RichText::new(&col.label)
                                                .strong()
                                                .color(theme.colors.on_surface),
                                        );
                                    });
                                });
                            }
                        })
                        .body(|mut body| {
                            for (idx, item) in items.iter().enumerate() {
                                body.row(self.row_height, |mut row| {
                                    render_row(item, idx, &mut row, &mut actions);
                                });
                            }
                        });
                });
        }

        actions
    }
}

impl Default for ItemTable {
    fn default() -> Self {
        Self::new()
    }
}
