//! Button variant definitions for semantic styling

use crate::ThemeColors;
use egui::{Color32, Stroke};

/// Button styling variants
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Primary action button (filled with primary color)
    #[default]
    Primary,
    /// Secondary action button (subtle background)
    Secondary,
    /// Outline button (transparent with border)
    Outline,
    /// Destructive/danger button (error color)
    Destructive,
    /// Ghost button (text only, no background unless hovered)
    Ghost,
}

impl ButtonVariant {
    /// Get the background fill color for this variant
    pub fn bg_color(&self, colors: &ThemeColors) -> Color32 {
        match self {
            ButtonVariant::Primary => colors.primary,
            ButtonVariant::Secondary => colors.secondary,
            ButtonVariant::Outline => Color32::TRANSPARENT,
            ButtonVariant::Destructive => colors.error,
            ButtonVariant::Ghost => Color32::TRANSPARENT,
        }
    }

    /// Get the text color for this variant
    pub fn text_color(&self, colors: &ThemeColors) -> Color32 {
        match self {
            ButtonVariant::Primary => colors.on_primary,
            ButtonVariant::Secondary => colors.on_secondary,
            ButtonVariant::Outline => colors.primary,
            ButtonVariant::Destructive => colors.on_error,
            ButtonVariant::Ghost => colors.on_surface,
        }
    }

    /// Get the stroke/border for this variant
    pub fn stroke(&self, colors: &ThemeColors) -> Stroke {
        match self {
            ButtonVariant::Outline => Stroke::new(1.0, colors.outline),
            _ => Stroke::NONE,
        }
    }
}
