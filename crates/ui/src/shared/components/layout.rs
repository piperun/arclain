//! Flutter-style Layout Components
//!
//! Declarative layout helpers inspired by Flutter's widget system.
//! These wrap egui's layout primitives with a more ergonomic API.

use eframe::egui::{self, Ui};

/// Main axis alignment for Row/Column
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum MainAxisAlignment {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Cross axis alignment for Row/Column
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum CrossAxisAlignment {
    #[default]
    Start,
    Center,
    End,
}

// ============================================================================
// Row - Horizontal layout
// ============================================================================

/// Horizontal layout container (like Flutter's Row)
pub struct Row {
    spacing: f32,
    main_axis: MainAxisAlignment,
    cross_axis: CrossAxisAlignment,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            spacing: 8.0,
            main_axis: MainAxisAlignment::Start,
            cross_axis: CrossAxisAlignment::Center,
        }
    }
}

impl Row {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn main_axis(mut self, alignment: MainAxisAlignment) -> Self {
        self.main_axis = alignment;
        self
    }

    pub fn cross_axis(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_axis = alignment;
        self
    }

    pub fn show(self, ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
        let align = match self.cross_axis {
            CrossAxisAlignment::Start => egui::Align::Min,
            CrossAxisAlignment::Center => egui::Align::Center,
            CrossAxisAlignment::End => egui::Align::Max,
        };

        let layout = match self.main_axis {
            MainAxisAlignment::Start => egui::Layout::left_to_right(align),
            MainAxisAlignment::Center => egui::Layout::left_to_right(align),
            MainAxisAlignment::End => egui::Layout::right_to_left(align),
            MainAxisAlignment::SpaceBetween => egui::Layout::left_to_right(align),
        };

        ui.with_layout(layout, |ui| {
            ui.spacing_mut().item_spacing.x = self.spacing;

            if self.main_axis == MainAxisAlignment::Center {
                // Add flexible space before content to center it
                ui.add_space(ui.available_width() / 2.0);
            }

            content(ui);
        });
    }
}

// ============================================================================
// Column - Vertical layout
// ============================================================================

/// Vertical layout container (like Flutter's Column)
pub struct Column {
    spacing: f32,
    cross_axis: CrossAxisAlignment,
}

impl Default for Column {
    fn default() -> Self {
        Self {
            spacing: 8.0,
            cross_axis: CrossAxisAlignment::Start,
        }
    }
}

impl Column {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn cross_axis(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_axis = alignment;
        self
    }

    pub fn show(self, ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
        let align = match self.cross_axis {
            CrossAxisAlignment::Start => egui::Align::Min,
            CrossAxisAlignment::Center => egui::Align::Center,
            CrossAxisAlignment::End => egui::Align::Max,
        };

        ui.with_layout(egui::Layout::top_down(align), |ui| {
            ui.spacing_mut().item_spacing.y = self.spacing;
            content(ui);
        });
    }
}

// ============================================================================
// Center - Centers content
// ============================================================================

/// Centers content both horizontally and vertically
pub struct Center;

impl Center {
    pub fn horizontal(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), content);
    }

    pub fn vertical(ui: &mut Ui, add_content: impl FnOnce(&mut Ui)) {
        ui.with_layout(
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
            add_content,
        );
    }
}

// ============================================================================
// Spacer - Flexible space
// ============================================================================

/// Adds flexible or fixed space
pub struct Spacer;

impl Spacer {
    /// Add a fixed amount of space
    pub fn fixed(ui: &mut Ui, size: f32) {
        ui.add_space(size);
    }

    /// Add flexible space that expands to fill available room
    pub fn flex(ui: &mut Ui) {
        ui.add_space(ui.available_width().max(ui.available_height()));
    }
}

// ============================================================================
// Padding - Adds padding around content
// ============================================================================

/// Adds padding around content
pub struct Padding {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

impl Default for Padding {
    fn default() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
        }
    }
}

impl Padding {
    pub fn new() -> Self {
        Self::default()
    }

    /// All sides equal padding
    pub fn all(size: f32) -> Self {
        Self {
            left: size,
            right: size,
            top: size,
            bottom: size,
        }
    }

    /// Symmetric padding (horizontal, vertical)
    pub fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }

    /// Only horizontal padding
    pub fn horizontal(size: f32) -> Self {
        Self {
            left: size,
            right: size,
            ..Default::default()
        }
    }

    /// Only vertical padding
    pub fn vertical(size: f32) -> Self {
        Self {
            top: size,
            bottom: size,
            ..Default::default()
        }
    }

    /// Only left padding
    pub fn left(size: f32) -> Self {
        Self {
            left: size,
            ..Default::default()
        }
    }

    pub fn show(self, ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
        if self.top > 0.0 {
            ui.add_space(self.top);
        }

        ui.horizontal(|ui| {
            if self.left > 0.0 {
                ui.add_space(self.left);
            }

            ui.vertical(|ui| {
                content(ui);
            });

            if self.right > 0.0 {
                ui.add_space(self.right);
            }
        });

        if self.bottom > 0.0 {
            ui.add_space(self.bottom);
        }
    }
}

// ============================================================================
// SizedBox - Fixed size container
// ============================================================================

/// A box with fixed dimensions
pub struct SizedBox {
    width: Option<f32>,
    height: Option<f32>,
}

impl SizedBox {
    pub fn new() -> Self {
        Self {
            width: None,
            height: None,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    pub fn square(size: f32) -> Self {
        Self {
            width: Some(size),
            height: Some(size),
        }
    }

    pub fn show(self, ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
        let size = egui::vec2(
            self.width.unwrap_or(ui.available_width()),
            self.height.unwrap_or(ui.available_height()),
        );

        ui.allocate_ui(size, |ui| {
            content(ui);
        });
    }

    /// Just reserve space without content
    pub fn empty(self, ui: &mut Ui) {
        if let (Some(w), Some(h)) = (self.width, self.height) {
            ui.allocate_space(egui::vec2(w, h));
        }
    }
}

impl Default for SizedBox {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// FormField - Label + Field aligned row
// ============================================================================

/// A form field with label and input aligned
pub struct FormField {
    label: String,
    label_width: f32,
}

impl FormField {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            label_width: 120.0,
        }
    }

    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = width;
        self
    }

    pub fn show(self, ui: &mut Ui, field: impl FnOnce(&mut Ui)) {
        ui.horizontal(|ui| {
            ui.add_sized([self.label_width, 18.0], egui::Label::new(&self.label));
            field(ui);
        });
    }
}

// ============================================================================
// Section - A titled section with content
// ============================================================================

/// A section with a title header
pub struct Section {
    title: String,
    spacing: f32,
}

impl Section {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            spacing: 8.0,
        }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn show(
        self,
        ui: &mut Ui,
        theme: &crate::shared::theme::AppTheme,
        content: impl FnOnce(&mut Ui),
    ) {
        ui.label(
            egui::RichText::new(&self.title)
                .size(13.0)
                .strong()
                .color(theme.colors.on_surface),
        );
        ui.add_space(self.spacing);
        content(ui);
    }
}
