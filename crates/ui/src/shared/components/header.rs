use crate::shared::theme::AppTheme;
use eframe::egui;

#[derive(Default)]
pub struct HeaderState {
    pub search_text: String,
    pub show_button_labels: bool, // UI preference: show text labels on buttons
}

#[derive(Default)]
pub struct HeaderActions {
    pub navigate_home: bool,
    pub navigate_back: bool,
    pub navigate_plugins: bool,
    pub navigate_settings: bool,
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut HeaderState,
    on_theme_toggle: &mut bool,
    show_nav_buttons: bool,
    can_go_back: bool,
    is_on_settings: bool,
) -> HeaderActions {
    let mut actions = HeaderActions::default();
    let show_labels = state.show_button_labels;

    let full_rect = ui.available_rect_before_wrap();
    let left_width = if show_nav_buttons { 80.0 } else { 0.0 };
    let right_width = 120.0;
    let center_width = full_rect.width() - left_width - right_width - 24.0;

    // === LEFT SECTION: Navigation ===
    if show_nav_buttons {
        let left_rect =
            egui::Rect::from_min_size(full_rect.min, egui::vec2(left_width, full_rect.height()));
        ui.allocate_ui_at_rect(left_rect, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

                if header_button(
                    ui,
                    theme,
                    egui_phosphor::regular::HOUSE,
                    "Home",
                    true,
                    show_labels,
                )
                .clicked()
                {
                    actions.navigate_home = true;
                }
                if header_button(
                    ui,
                    theme,
                    egui_phosphor::regular::ARROW_LEFT,
                    "Back",
                    can_go_back,
                    show_labels,
                )
                .clicked()
                {
                    actions.navigate_back = true;
                }
            });
        });
    }

    // === CENTER SECTION: Search (centered) ===
    let center_rect = egui::Rect::from_min_size(
        egui::pos2(full_rect.min.x + left_width + 12.0, full_rect.min.y),
        egui::vec2(center_width, full_rect.height()),
    );
    ui.allocate_ui_at_rect(center_rect, |ui| {
        ui.with_layout(
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                let search_frame = egui::Frame::NONE
                    .fill(theme.colors.bg_tertiary)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 6));

                let search_width = center_width.min(400.0).max(200.0);

                search_frame.show(ui, |ui| {
                    ui.set_width(search_width);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                                .size(14.0)
                                .color(theme.colors.text_muted),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut state.search_text)
                                .hint_text("Search files...")
                                .frame(false)
                                .desired_width(search_width - 32.0),
                        );
                    });
                });
            },
        );
    });

    // === RIGHT SECTION: Utilities ===
    let right_rect = egui::Rect::from_min_size(
        egui::pos2(full_rect.max.x - right_width, full_rect.min.y),
        egui::vec2(right_width, full_rect.height()),
    );
    ui.allocate_ui_at_rect(right_rect, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

            // Theme toggle (rightmost)
            let toggle_size = egui::vec2(40.0, 20.0);
            let (rect, response) = ui.allocate_exact_size(toggle_size, egui::Sense::click());
            if response.clicked() {
                *on_theme_toggle = true;
            }
            response.on_hover_text(if theme.dark_mode {
                "Light mode"
            } else {
                "Dark mode"
            });

            if ui.is_rect_visible(rect) {
                let radius = rect.height() / 2.0;
                ui.painter()
                    .rect_filled(rect, radius, theme.colors.bg_tertiary);
                let circle_radius = (rect.height() - 6.0) / 2.0;
                let circle_x = if theme.dark_mode {
                    rect.right() - circle_radius - 3.0
                } else {
                    rect.left() + circle_radius + 3.0
                };
                ui.painter().circle_filled(
                    egui::pos2(circle_x, rect.center().y),
                    circle_radius,
                    theme.colors.accent,
                );
            }

            ui.add_space(4.0);

            // Settings
            let settings_bg = if is_on_settings {
                theme.colors.bg_hover
            } else {
                theme.colors.bg_tertiary
            };
            if header_button_with_bg(
                ui,
                theme,
                egui_phosphor::regular::GEAR,
                "Settings",
                true,
                show_labels,
                settings_bg,
            )
            .clicked()
            {
                actions.navigate_settings = true;
            }

            // Plugins
            if header_button(
                ui,
                theme,
                egui_phosphor::regular::PUZZLE_PIECE,
                "Plugins",
                true,
                show_labels,
            )
            .clicked()
            {
                actions.navigate_plugins = true;
            }
        });
    });

    actions
}

/// Header button with optional label
fn header_button(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    icon: &str,
    label: &str,
    enabled: bool,
    show_label: bool,
) -> egui::Response {
    header_button_with_bg(
        ui,
        theme,
        icon,
        label,
        enabled,
        show_label,
        theme.colors.bg_tertiary,
    )
}

/// Header button with optional label and custom background
fn header_button_with_bg(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    icon: &str,
    label: &str,
    enabled: bool,
    show_label: bool,
    bg: egui::Color32,
) -> egui::Response {
    let color = if enabled {
        theme.colors.text_primary
    } else {
        theme.colors.text_muted
    };

    let text = if show_label {
        format!("{} {}", icon, label)
    } else {
        icon.to_string()
    };

    let min_width = if show_label { 80.0 } else { 32.0 };

    let button = egui::Button::new(egui::RichText::new(&text).size(14.0).color(color))
        .fill(bg)
        .stroke(egui::Stroke::NONE)
        .corner_radius(4.0)
        .min_size(egui::vec2(min_width, 28.0));

    let response = ui.add_enabled(enabled, button);

    // Show tooltip if label is hidden
    if !show_label {
        response.on_hover_text(label)
    } else {
        response
    }
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
