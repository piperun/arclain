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
    /// Label displayed above the input
    label: Option<String>,
    /// Helper text displayed below the input
    helper_text: Option<String>,
    /// Error text displayed below the input (overrides helper_text when in Error state)
    error_text: Option<String>,
    /// Whether label is required (shows asterisk)
    required: bool,
    /// Whether to mask input as a password
    password: bool,
    debug_lines: bool,
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
            label: None,
            helper_text: None,
            error_text: None,
            required: false,
            password: false,
            debug_lines: false,
        }
    }

    /// Mask input as a password field
    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
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

    /// Set the label displayed above the input
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set helper text displayed below the input
    pub fn helper_text(mut self, text: impl Into<String>) -> Self {
        self.helper_text = Some(text.into());
        self
    }

    /// Set error text (displayed below input when in Error state)
    pub fn error_text(mut self, text: impl Into<String>) -> Self {
        self.error_text = Some(text.into());
        self.state = TextInputState::Error;
        self
    }

    /// Mark the field as required (shows asterisk after label)
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Force the debug overlay on for the input's frame rect (not the
    /// surrounding label/helper text). ORs with
    /// `EGUI_UI_DEBUG_GUIDELINES`. Stripped in release builds.
    pub fn debug_lines(mut self, on: bool) -> Self {
        self.debug_lines = on;
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
        self.show_impl(ui, None::<fn(&mut Ui) -> Option<Response>>)
    }

    /// Show the input with a custom suffix widget
    /// The closure receives a Ui and should return an optional Response if the suffix is interactive
    pub fn show_with_suffix<F>(self, ui: &mut Ui, suffix_fn: F) -> TextInputResponse
    where
        F: FnOnce(&mut Ui) -> Option<Response>,
    {
        self.show_impl(ui, Some(suffix_fn))
    }

    fn show_impl<F>(self, ui: &mut Ui, custom_suffix: Option<F>) -> TextInputResponse
    where
        F: FnOnce(&mut Ui) -> Option<Response>,
    {
        let height = self.size.height();
        let font_size = self.size.font_size();
        let icon_size = self.size.icon_size();
        let padding = self.size.padding();
        let label_size = 12.0;
        let helper_size = 11.0;

        // Resolve colors
        let (bg_fill, text_color, hint_color, border_color, prefix_color) =
            self.resolve_colors(ui);

        // Calculate total width
        let total_width = self.width.unwrap_or(ui.available_width());

        // Track suffix/clear clicks
        let mut suffix_clicked = false;
        let mut cleared = false;
        let mut input_response: Option<Response> = None;

        // Wrap everything in a vertical layout
        ui.vertical(|ui| {
            ui.set_width(total_width);

            // Render label if present
            if let Some(label) = &self.label {
                ui.horizontal(|ui| {
                    let label_color = if let Some(colors) = self.theme_colors {
                        colors.on_surface
                    } else {
                        ui.visuals().text_color()
                    };

                    ui.label(
                        egui::RichText::new(label)
                            .size(label_size)
                            .color(label_color),
                    );

                    if self.required {
                        let required_color = if let Some(colors) = self.theme_colors {
                            colors.error
                        } else {
                            Color32::RED
                        };
                        ui.label(
                            egui::RichText::new("*")
                                .size(label_size)
                                .color(required_color),
                        );
                    }
                });
                ui.add_space(4.0);
            }

            // Calculate slot widths
            let prefix_width = self.prefix.as_ref().map(|_| icon_size + padding * 2.0).unwrap_or(0.0);

            // Suffix: either custom suffix, or clear button if clearable and has text
            let show_clear = self.clearable && !self.text.is_empty();
            let has_builtin_suffix = self.suffix.is_some() || show_clear;
            let has_custom_suffix = custom_suffix.is_some();
            let suffix_width = if has_builtin_suffix || has_custom_suffix {
                icon_size + padding * 2.0
            } else {
                0.0
            };

            let input_width = total_width - prefix_width - suffix_width - padding * 2.0;

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
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    let mut edit = TextEdit::singleline(self.text)
                        .vertical_align(egui::Align::Center)
                        .desired_width(input_width - padding)
                        .text_color(text_color)
                        .frame(false)
                        .clip_text(true)
                        .interactive(!self.disabled)
                        .margin(egui::Margin::symmetric(padding as i8, 0));

                    if let Some(hint) = &self.hint {
                        edit = edit.hint_text(egui::RichText::new(hint).color(hint_color));
                    }

                    if self.password {
                        edit = edit.password(true);
                    }

                    if self.monospace {
                        edit = edit.font(egui::FontSelection::FontId(egui::FontId::monospace(font_size)));
                    } else {
                        edit = edit.font(egui::FontSelection::FontId(egui::FontId::proportional(font_size)));
                    }

                    ui.add(edit)
                },
            ).inner;
            input_response = Some(response);

            // Render suffix (custom or built-in)
            if has_custom_suffix || has_builtin_suffix {
                child_ui.allocate_ui_with_layout(
                    egui::vec2(suffix_width, height - 2.0),
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        if let Some(suffix_fn) = custom_suffix {
                            if let Some(resp) = suffix_fn(ui) {
                                if resp.clicked() {
                                    suffix_clicked = true;
                                }
                            }
                        } else {
                            let (icon, is_clear) = if show_clear {
                                (egui_phosphor::regular::X.to_string(), true)
                            } else if let Some(SlotContent::Icon(ref icon)) = self.suffix {
                                (icon.clone(), false)
                            } else if let Some(SlotContent::Text(ref text)) = self.suffix {
                                (text.clone(), false)
                            } else {
                                return;
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
                            } else {
                                ui.label(
                                    egui::RichText::new(&icon)
                                        .size(icon_size)
                                        .color(hint_color),
                                );
                            }
                        }
                    },
                );
            }

            #[cfg(debug_assertions)]
            crate::debug::paint_widget_rect_debug(
                ui.painter(),
                full_rect,
                "text-input",
                self.debug_lines || crate::debug::ui_debug_guidelines_enabled(),
            );

            // Render helper text or error text below input
            let show_error = self.state == TextInputState::Error && self.error_text.is_some();
            let bottom_text = if show_error {
                self.error_text.as_deref()
            } else {
                self.helper_text.as_deref()
            };

            if let Some(text) = bottom_text {
                ui.add_space(4.0);
                let text_color = if show_error {
                    if let Some(colors) = self.theme_colors {
                        colors.error
                    } else {
                        Color32::from_rgb(220, 50, 50)
                    }
                } else if let Some(colors) = self.theme_colors {
                    colors.on_surface_variant
                } else {
                    ui.visuals().weak_text_color()
                };

                ui.label(
                    egui::RichText::new(text)
                        .size(helper_size)
                        .color(text_color),
                );
            }
        });

        // Handle clear action
        if cleared {
            self.text.clear();
        }

        TextInputResponse {
            response: input_response.unwrap_or_else(|| ui.label("")),
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
