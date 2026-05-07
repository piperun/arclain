use arclain_theme::ThemeColors;
use arclain_widgets::TextInput;
use eframe::egui;
use egui::Widget;

pub struct SearchBar<'a> {
    query: &'a mut String,
    hint: Option<String>,
    desired_width: Option<f32>,
    theme_colors: Option<&'a ThemeColors>,
}

#[allow(dead_code)]
impl<'a> SearchBar<'a> {
    pub fn new(query: &'a mut String) -> Self {
        Self {
            query,
            hint: None,
            desired_width: None,
            theme_colors: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.desired_width = Some(width);
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.theme_colors = Some(colors);
        self
    }
}

impl<'a> Widget for SearchBar<'a> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let hint = self.hint.unwrap_or_else(|| "Search...".to_string());
        let width = self.desired_width.unwrap_or(250.0);

        let mut input = TextInput::new(self.query).hint(hint).width(width);
        if let Some(colors) = self.theme_colors {
            input = input.with_theme_colors(colors);
        }
        input.show(ui).response
    }
}
