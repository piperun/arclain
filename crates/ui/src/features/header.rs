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

pub struct HeaderActions {
    pub navigate_home: bool,
    pub navigate_back: bool,
    pub navigate_plugins: bool,
    pub navigate_settings: bool,
}

impl Default for HeaderActions {
    fn default() -> Self {
        Self {
            navigate_home: false,
            navigate_back: false,
            navigate_plugins: false,
            navigate_settings: false,
        }
    }
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut HeaderState,
    on_theme_toggle: &mut bool,
    show_nav_buttons: bool,
    can_go_back: bool,
) -> HeaderActions {
    let mut actions = HeaderActions::default();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Navigation buttons (shown when not on main page)
        if show_nav_buttons {
            // Home button
            let home_btn = egui::Button::new(egui::RichText::new("🏠").size(16.0))
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                .min_size(egui::vec2(32.0, 32.0));

            if ui.add(home_btn).clicked() {
                actions.navigate_home = true;
            }

            // Back button
            let back_btn = egui::Button::new(egui::RichText::new("←").size(16.0))
                .fill(if can_go_back {
                    egui::Color32::TRANSPARENT
                } else {
                    theme.colors.bg_secondary
                })
                .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                .min_size(egui::vec2(32.0, 32.0));

            if ui.add_enabled(can_go_back, back_btn).clicked() {
                actions.navigate_back = true;
            }

            ui.add_space(8.0);
        }

        // Plugins button
        let plugins_btn = egui::Button::new(egui::RichText::new("⬢").size(16.0))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
            .min_size(egui::vec2(32.0, 32.0));
        if ui.add(plugins_btn).clicked() {
            actions.navigate_plugins = true;
        }
        ui.add_space(8.0);

        // Settings button (always at the top row, same style as nav)
        let settings_btn = egui::Button::new(egui::RichText::new("⚙").size(16.0))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
            .min_size(egui::vec2(32.0, 32.0));
        if ui.add(settings_btn).clicked() {
            actions.navigate_settings = true;
        }
        ui.add_space(8.0);

        // Title - matching mockup style
        ui.label(
            egui::RichText::new("ARCLAIN")
                .size(14.0)
                .color(theme.colors.text_secondary)
                .strong(),
        );

        ui.add_space(ui.available_width() - if show_nav_buttons { 460.0 } else { 380.0 });

        // Search box with proper theming
        let search_frame = egui::Frame::NONE
            .fill(theme.colors.bg_primary)
            .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 4));

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
                egui::CornerRadius::same(radius as u8),
                egui::Stroke::new(1.0, theme.colors.border_color),
                egui::StrokeKind::Outside,
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

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_actions_default_is_clean() {
        let a = HeaderActions::default();
        assert!(!a.navigate_home);
        assert!(!a.navigate_back);
        assert!(!a.navigate_settings);
    }
}
