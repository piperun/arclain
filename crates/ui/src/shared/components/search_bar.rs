use eframe::egui;
use egui::Widget;

pub struct SearchBar<'a> {
    query: &'a mut String,
    hint: Option<String>,
    desired_width: Option<f32>,
}

#[allow(dead_code)]
impl<'a> SearchBar<'a> {
    pub fn new(query: &'a mut String) -> Self {
        Self {
            query,
            hint: None,
            desired_width: None,
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
}

impl<'a> Widget for SearchBar<'a> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let hint = self.hint.unwrap_or_else(|| "Search...".to_string());
        let width = self.desired_width.unwrap_or(250.0);

        // Potential future enhancement: Add a search icon inside or next to it
        ui.add(
            egui::TextEdit::singleline(self.query)
                .hint_text(hint)
                .desired_width(width),
        )
    }
}
