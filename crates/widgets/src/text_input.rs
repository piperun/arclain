//! Text Input Widget
//!
//! A styled single-line text input with prefix/suffix slots, state variants,
//! and responsive sizing. Based on common design system patterns.

use arclain_theme::ThemeColors;
use egui::{Color32, Response, Sense, TextEdit, Ui, Widget};

/// Height presets for text inputs with proportional text sizing
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TextInputSize {
    /// Small: 28px height, 12px font
    Small,
    /// Medium: 32px height, 13px font (default)
    #[default]
    Medium,
    /// Large: 40px height, 14px font
    Large,
}

impl TextInputSize {
    pub fn height(self) -> f32 {
        match self {
            TextInputSize::Small => 28.0,
            TextInputSize::Medium => 32.0,
            TextInputSize::Large => 40.0,
        }
    }

    pub fn font_size(self) -> f32 {
        match self {
            TextInputSize::Small => 12.0,
            TextInputSize::Medium => 13.0,
            TextInputSize::Large => 14.0,
        }
    }

    pub fn icon_size(self) -> f32 {
        match self {
            TextInputSize::Small => 14.0,
            TextInputSize::Medium => 16.0,
            TextInputSize::Large => 18.0,
        }
    }

    pub fn padding(self) -> f32 {
        match self {
            TextInputSize::Small => 6.0,
            TextInputSize::Medium => 8.0,
            TextInputSize::Large => 10.0,
        }
    }
}

/// Visual state variants for the input
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TextInputState {
    #[default]
    Normal,
    Error,
    Warning,
    Success,
}

/// Content for prefix/suffix slots
#[derive(Clone)]
pub enum SlotContent {
    /// Icon (typically from egui_phosphor)
    Icon(String),
    /// Plain text
    Text(String),
}

/// A styled single-line text input with advanced features
pub struct TextInput<'a> {
    text: &'a mut String,
    hint: Option<String>,
    size: TextInputSize,
    width: Option<f32>,
    theme_colors: Option<&'a ThemeColors>,
    monospace: bool,
    state: TextInputState,
    disabled: bool,
    prefix: Option<SlotContent>,
    suffix: Option<SlotContent>,
    clearable: bool,
    interactive_suffix: bool,
}

impl<'a> TextInput<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self {
            text,
            hint: None,
            size: TextInputSize::Medium,
            width: None,
            theme_colors: None,
            monospace: false,
            state: TextInputState::Normal,
            disabled: false,
            prefix: None,
            suffix: None,
            clearable: false,
            interactive_suffix: false,
        }
    }

    /// Set placeholder/hint text
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Set the input size (Small, Medium, Large)
    pub fn size(mut self, size: TextInputSize) -> Self {
        self.size = size;
        self
    }

    /// Set a specific width
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Use monospace font
    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }

    /// Set theme colors
    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.theme_colors = Some(colors);
        self
    }

    /// Set input state (Normal, Error, Warning, Success)
    pub fn state(mut self, state: TextInputState) -> Self {
        self.state = state;
        self
    }

    /// Mark as error state (shorthand)
    pub fn error(mut self) -> Self {
        self.state = TextInputState::Error;
        self
    }

    /// Mark as warning state (shorthand)
    pub fn warning(mut self) -> Self {
        self.state = TextInputState::Warning;
        self
    }

    /// Mark as success state (shorthand)
    pub fn success(mut self) -> Self {
        self.state = TextInputState::Success;
        self
    }

    /// Disable the input
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Add a prefix icon
    pub fn prefix_icon(mut self, icon: impl Into<String>) -> Self {
        self.prefix = Some(SlotContent::Icon(icon.into()));
        self
    }

    /// Add a prefix text
    pub fn prefix_text(mut self, text: impl Into<String>) -> Self {
        self.prefix = Some(SlotContent::Text(text.into()));
        self
    }

    /// Add a suffix icon
    pub fn suffix_icon(mut self, icon: impl Into<String>) -> Self {
        self.suffix = Some(SlotContent::Icon(icon.into()));
        self
    }

    /// Add a suffix text
    pub fn suffix_text(mut self, text: impl Into<String>) -> Self {
        self.suffix = Some(SlotContent::Text(text.into()));
        self
    }

    /// Mark suffix as interactive (clickable button)
    pub fn interactive_suffix(mut self) -> Self {
        self.interactive_suffix = true;
        self
    }

    /// Add a clear button (X) that appears when text is not empty
    pub fn clearable(mut self) -> Self {
        self.clearable = true;
        self
    }
}

/// Response from TextInput including suffix click status
pub struct TextInputResponse {
    /// The main text edit response
    pub response: Response,
    /// Whether the suffix was clicked (if interactive)
    pub suffix_clicked: bool,
    /// Whether the clear button was clicked
    pub cleared: bool,
}

impl TextInputResponse {
    /// Check if the text changed
    pub fn changed(&self) -> bool {
        self.response.changed() || self.cleared
    }
}

impl<'a> TextInput<'a> {
    /// Show the input and return extended response
    pub fn show(self, ui: &mut Ui) -> TextInputResponse {
        let height = self.size.height();
        let font_size = self.size.font_size();
        let icon_size = self.size.icon_size();
        let padding = self.size.padding();

        // Resolve colors
        let (bg_fill, text_color, hint_color, border_color, prefix_color) =
            self.resolve_colors(ui);

        // Calculate slot widths
        let prefix_width = self.prefix.as_ref().map(|_| icon_size + padding * 2.0).unwrap_or(0.0);

        // Suffix: either custom suffix, or clear button if clearable and has text
        let show_clear = self.clearable && !self.text.is_empty();
        let has_suffix = self.suffix.is_some() || show_clear;
        let suffix_width = if has_suffix { icon_size + padding * 2.0 } else { 0.0 };

        // Calculate total width
        let total_width = self.width.unwrap_or(ui.available_width());
        let input_width = total_width - prefix_width - suffix_width - padding * 2.0;

        // Track suffix/clear clicks
        let mut suffix_clicked = false;
        let mut cleared = false;

        // Allocate the full rect first for the frame
        let (full_rect, _) = ui.allocate_exact_size(
            egui::vec2(total_width, height),
            Sense::hover(),
        );

        // Draw background frame
        let corner_radius = 4.0;
        ui.painter().rect(
            full_rect,
            corner_radius,
            bg_fill,
            egui::Stroke::new(1.0, border_color),
            egui::StrokeKind::Inside,
        );

        // Create child UI for content
        let mut child_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(full_rect.shrink(1.0))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );

        // Render prefix
        if let Some(prefix) = &self.prefix {
            child_ui.allocate_ui_with_layout(
                egui::vec2(prefix_width, height - 2.0),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    match prefix {
                        SlotContent::Icon(icon) => {
                            ui.label(
                                egui::RichText::new(icon)
                                    .size(icon_size)
                                    .color(prefix_color),
                            );
                        }
                        SlotContent::Text(text) => {
                            ui.label(
                                egui::RichText::new(text)
                                    .size(font_size)
                                    .color(prefix_color),
                            );
                        }
                    }
                },
            );
        }

        // Render text input
        let response = child_ui.allocate_ui_with_layout(
            egui::vec2(input_width, height - 2.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let mut edit = TextEdit::singleline(self.text)
                    .vertical_align(egui::Align::Center)
                    .min_size(egui::vec2(input_width - padding, height - 2.0))
                    .text_color(text_color)
                    .frame(false)
                    .interactive(!self.disabled);

                if let Some(hint) = &self.hint {
                    edit = edit.hint_text(egui::RichText::new(hint).color(hint_color));
                }

                if self.monospace {
                    edit = edit.font(egui::FontSelection::FontId(egui::FontId::monospace(font_size)));
                } else {
                    edit = edit.font(egui::FontSelection::FontId(egui::FontId::proportional(font_size)));
                }

                ui.add(edit)
            },
        ).inner;

        // Render suffix or clear button
        if has_suffix {
            let suffix_response = child_ui.allocate_ui_with_layout(
                egui::vec2(suffix_width, height - 2.0),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    let (icon, is_clear) = if show_clear {
                        (egui_phosphor::regular::X.to_string(), true)
                    } else if let Some(SlotContent::Icon(ref icon)) = self.suffix {
                        (icon.clone(), false)
                    } else if let Some(SlotContent::Text(ref text)) = self.suffix {
                        (text.clone(), false)
                    } else {
                        return ui.label(""); // Empty
                    };

                    // Interactive suffix as button
                    if self.interactive_suffix || is_clear {
                        let btn_response = ui.add(
                            egui::Button::new(
                                egui::RichText::new(&icon)
                                    .size(icon_size)
                                    .color(hint_color),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(corner_radius),
                        );

                        if btn_response.clicked() {
                            if is_clear {
                                cleared = true;
                            } else {
                                suffix_clicked = true;
                            }
                        }
                        btn_response
                    } else {
                        ui.label(
                            egui::RichText::new(&icon)
                                .size(icon_size)
                                .color(hint_color),
                        )
                    }
                },
            );
            let _ = suffix_response;
        }

        // Handle clear action
        if cleared {
            self.text.clear();
        }

        TextInputResponse {
            response,
            suffix_clicked,
            cleared,
        }
    }

    fn resolve_colors(&self, ui: &Ui) -> (Color32, Color32, Color32, Color32, Color32) {
        if let Some(colors) = self.theme_colors {
            let (border_color, prefix_bg) = match self.state {
                TextInputState::Normal => (colors.outline_variant, colors.on_surface_variant),
                TextInputState::Error => (colors.error, colors.error),
                TextInputState::Warning => (colors.warning, colors.warning),
                TextInputState::Success => (colors.success, colors.success),
            };

            let (bg, text, hint) = if self.disabled {
                (
                    colors.surface_variant.gamma_multiply(0.5),
                    colors.on_surface_variant,
                    colors.on_surface_variant.gamma_multiply(0.5),
                )
            } else {
                (
                    colors.surface_variant,
                    colors.on_surface,
                    colors.on_surface_variant,
                )
            };

            (bg, text, hint, border_color, prefix_bg)
        } else {
            let visuals = &ui.visuals().widgets.inactive;
            (
                visuals.bg_fill,
                visuals.fg_stroke.color,
                ui.visuals().weak_text_color(),
                visuals.bg_stroke.color,
                ui.visuals().weak_text_color(),
            )
        }
    }
}

impl<'a> Widget for TextInput<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui).response
    }
}
