use super::theme::AppTheme;
use eframe::egui;

pub struct HeaderState {
    pub search_text: String,
}

impl Default for HeaderState {
    fn default() -> Self {
        Self {
            search_text: String::new(),
        }
    }
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut HeaderState,
    on_theme_toggle: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(16.0, 0.0);

        // Title - matching mockup style
        ui.label(
            egui::RichText::new("ARCHIVE VIEWER")
                .size(14.0)
                .color(theme.colors.text_secondary)
                .strong(),
        );

        ui.add_space(ui.available_width() - 380.0);

        // Search box with proper theming
        let search_frame = egui::Frame::none()
            .fill(theme.colors.bg_primary)
            .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
            .rounding(4.0)
            .inner_margin(egui::Margin::symmetric(8.0, 4.0));

        search_frame.show(ui, |ui| {
            ui.add_sized(
                [284.0, 16.0],
                egui::TextEdit::singleline(&mut state.search_text)
                    .hint_text("Search files...")
                    .frame(false),
            );
        });

        ui.add_space(12.0);

        // Theme toggle - styled like the mockup
        let toggle_size = egui::vec2(48.0, 24.0);
        let (rect, response) = ui.allocate_exact_size(toggle_size, egui::Sense::click());

        if response.clicked() {
            *on_theme_toggle = true;
        }

        if ui.is_rect_visible(rect) {
            let radius = rect.height() / 2.0;

            // Background
            ui.painter()
                .rect_filled(rect, radius, theme.colors.bg_primary);

            // Border
            ui.painter().rect_stroke(
                rect,
                radius,
                egui::Stroke::new(1.0, theme.colors.border_color),
            );

            // Slider circle
            let circle_radius = (rect.height() - 4.0) / 2.0;
            let circle_x = if theme.dark_mode {
                rect.right() - circle_radius - 2.0
            } else {
                rect.left() + circle_radius + 2.0
            };
            let circle_center = egui::pos2(circle_x, rect.center().y);

            ui.painter()
                .circle_filled(circle_center, circle_radius, theme.colors.accent);
        }
    });
}
