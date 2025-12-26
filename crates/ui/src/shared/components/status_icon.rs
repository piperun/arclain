//! StatusIcon component - A small status indicator with icon and optional count
//!
//! Used in the status bar and other places to show plugin status, progress, etc.

use crate::shared::theme::AppTheme;
use eframe::egui;

/// Configuration for a status icon
pub struct StatusIcon<'a> {
    icon: &'a str,
    label: Option<&'a str>,
    count: Option<(usize, usize)>, // current / total
    color: Option<egui::Color32>,
    tooltip: Option<&'a str>,
}

impl<'a> StatusIcon<'a> {
    /// Create a new status icon with the given Phosphor icon
    pub fn new(icon: &'a str) -> Self {
        Self {
            icon,
            label: None,
            count: None,
            color: None,
            tooltip: None,
        }
    }

    /// Add a text label next to the icon
    #[allow(dead_code)]
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }

    /// Add a count display (e.g., "3/5")
    pub fn count(mut self, current: usize, total: usize) -> Self {
        self.count = Some((current, total));
        self
    }

    /// Set a custom color for the icon
    pub fn color(mut self, color: egui::Color32) -> Self {
        self.color = Some(color);
        self
    }

    /// Add a tooltip on hover
    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    /// Render the status icon
    pub fn show(self, ui: &mut egui::Ui, theme: &AppTheme) -> egui::Response {
        let icon_color = self.color.unwrap_or(theme.colors.on_surface_variant);

        let response = ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            // Icon
            ui.label(egui::RichText::new(self.icon).size(14.0).color(icon_color));

            // Count (if provided)
            if let Some((current, total)) = self.count {
                arclain_widgets::Text::new(&format!("{}/{}", current, total))
                    .size(12.0)
                    .muted()
                    .show(ui);
            }

            // Label (if provided)
            if let Some(label) = self.label {
                arclain_widgets::Text::new(label)
                    .size(12.0)
                    .muted()
                    .show(ui);
            }
        });

        let response = response.response;

        // Tooltip
        if let Some(tooltip_text) = self.tooltip {
            response.clone().on_hover_text(tooltip_text);
        }

        response
    }
}
