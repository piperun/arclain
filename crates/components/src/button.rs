use arclain_theme::{ButtonVariant, ThemeColors};
use egui::{self, Color32, Response, Ui, Widget, WidgetText};

/// Button size presets for responsive design
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ButtonSize {
    /// Small button: 60x28
    Small,
    /// Medium button: 80x32 (default)
    #[default]
    Medium,
    /// Large button: 100x40
    Large,
    /// Extra large button: 120x48
    XLarge,
    /// Custom dimensions
    Custom { width: f32, height: f32 },
}

impl ButtonSize {
    /// Convert to egui Vec2 for min_size
    pub fn to_vec2(self) -> egui::Vec2 {
        match self {
            ButtonSize::Small => egui::vec2(60.0, 28.0),
            ButtonSize::Medium => egui::vec2(80.0, 32.0),
            ButtonSize::Large => egui::vec2(100.0, 40.0),
            ButtonSize::XLarge => egui::vec2(120.0, 48.0),
            ButtonSize::Custom { width, height } => egui::vec2(width, height),
        }
    }

    /// Create custom size from width and height
    pub fn custom(width: f32, height: f32) -> Self {
        ButtonSize::Custom { width, height }
    }
}

/// A standardized text button with semantic styling and size options.
pub struct TextButton<'a> {
    text: WidgetText,
    icon: Option<egui::WidgetText>,
    variant: ButtonVariant,
    size: ButtonSize,
    colors: Option<&'a ThemeColors>,
    fill: Option<Color32>,
}

impl<'a> TextButton<'a> {
    /// Create a new TextButton with text and size
    pub fn new(text: impl Into<WidgetText>, size: ButtonSize) -> Self {
        Self {
            text: text.into(),
            icon: None,
            variant: ButtonVariant::Primary,
            size,
            colors: None,
            fill: None,
        }
    }

    /// Set the button variant (Primary, Secondary, Outline, etc.)
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Add an icon (text/emoji/font icon)
    pub fn icon(mut self, icon: impl Into<WidgetText>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set theme colors for semantic styling
    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Override the button size
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set custom width (creates Custom size, preserves height)
    pub fn width(mut self, width: f32) -> Self {
        let current = self.size.to_vec2();
        self.size = ButtonSize::Custom {
            width,
            height: current.y,
        };
        self
    }

    /// Set custom height (creates Custom size, preserves width)
    pub fn height(mut self, height: f32) -> Self {
        let current = self.size.to_vec2();
        self.size = ButtonSize::Custom {
            width: current.x,
            height,
        };
        self
    }

    /// Deprecated: Use size() or width()/height() instead
    #[deprecated(since = "0.2.0", note = "Use size() or width()/height() instead")]
    pub fn min_size(mut self, size: egui::Vec2) -> Self {
        self.size = ButtonSize::Custom {
            width: size.x,
            height: size.y,
        };
        self
    }
}

impl<'a> Widget for TextButton<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (fill, text_col, stroke) = if let Some(colors) = self.colors {
            (
                self.fill.unwrap_or(self.variant.bg_color(colors)),
                self.variant.text_color(colors),
                self.variant.stroke(colors),
            )
        } else {
            // Fallback to UI visuals
            (
                self.fill.unwrap_or(ui.visuals().widgets.inactive.bg_fill),
                ui.visuals().widgets.inactive.fg_stroke.color,
                egui::Stroke::NONE,
            )
        };

        let button = egui::Button::new(self.text.strong().color(text_col))
            .fill(fill)
            .stroke(stroke)
            .min_size(self.size.to_vec2());

        ui.add(button)
    }
}
