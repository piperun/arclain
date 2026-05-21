use crate::core::signals::ServerConnectionStatus;
use arclain_theme::{AppTheme, ButtonVariant};
use arclain_widgets::TextInput;
use eframe::egui;
use egui::Widget;

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
    pub show_logs: bool,
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut HeaderState,
    on_theme_toggle: &mut bool,
    show_nav_buttons: bool,
    can_go_back: bool,
    is_on_settings: bool,
    search_focus_requested: &mut bool,
    server_status: &ServerConnectionStatus,
) -> HeaderActions {
    let mut actions = HeaderActions::default();
    let show_labels = state.show_button_labels;

    let full_rect = ui.available_rect_before_wrap();
    let left_width = if show_nav_buttons { 80.0 } else { 0.0 };
    // Extra 20px when server status dot is visible (Connected or Error).
    let has_server_indicator = !matches!(server_status, ServerConnectionStatus::Offline);
    let right_width = if has_server_indicator { 140.0 } else { 120.0 };
    let center_width = full_rect.width() - left_width - right_width - 24.0;

    // === LEFT SECTION: Navigation ===
    if show_nav_buttons {
        let left_rect =
            egui::Rect::from_min_size(full_rect.min, egui::vec2(left_width, full_rect.height()));
        ui.scope_builder(egui::UiBuilder::new().max_rect(left_rect), |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

                // Back first, then Home — follows the universal
                // convention (browser nav, iOS/Android nav stack,
                // every desktop app with a back+home pair). Arrow
                // anchors the left edge; home sits just to its right.
                if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_LEFT)
                    .with_theme_colors(&theme.colors)
                    .variant(ButtonVariant::Ghost)
                    .enabled(can_go_back)
                    .ui(ui)
                    .on_hover_text("Back")
                    .clicked()
                {
                    actions.navigate_back = true;
                }
                if arclain_widgets::IconButton::new(egui_phosphor::regular::HOUSE)
                    .with_theme_colors(&theme.colors)
                    .variant(ButtonVariant::Ghost)
                    .ui(ui)
                    .on_hover_text("Home")
                    .clicked()
                {
                    actions.navigate_home = true;
                }
            });
        });
    }

    // === CENTER SECTION: Search (centered) ===
    let center_rect = egui::Rect::from_min_size(
        egui::pos2(full_rect.min.x + left_width + 12.0, full_rect.min.y),
        egui::vec2(center_width, full_rect.height()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(center_rect), |ui| {
        ui.with_layout(
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                let search_frame = egui::Frame::NONE
                    .fill(theme.colors.surface_variant)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 6));

                let search_width = center_width.min(400.0).max(200.0);

                search_frame.show(ui, |ui| {
                    ui.set_width(search_width);
                    let response = TextInput::new(&mut state.search_text)
                        .hint(if is_on_settings {
                            "Search settings..."
                        } else {
                            "Search files..."
                        })
                        .prefix_icon(egui_phosphor::regular::MAGNIFYING_GLASS)
                        .width(search_width)
                        .with_theme_colors(&theme.colors)
                        .show(ui);

                    if *search_focus_requested {
                        response.response.request_focus();
                        *search_focus_requested = false;
                    }
                });
            },
        );
    });

    // === RIGHT SECTION: Utilities ===
    let right_rect = egui::Rect::from_min_size(
        egui::pos2(full_rect.max.x - right_width, full_rect.min.y),
        egui::vec2(right_width, full_rect.height()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(right_rect), |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);

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
                    .rect_filled(rect, radius, theme.colors.surface_variant);
                let circle_radius = (rect.height() - 6.0) / 2.0;
                let circle_x = if theme.dark_mode {
                    rect.right() - circle_radius - 3.0
                } else {
                    rect.left() + circle_radius + 3.0
                };
                ui.painter().circle_filled(
                    egui::pos2(circle_x, rect.center().y),
                    circle_radius,
                    theme.colors.on_surface,
                );
            }

            ui.add_space(4.0);

            // Settings
            let settings_btn = if show_labels {
                arclain_widgets::TextButton::new(
                    format!("{} Settings", egui_phosphor::regular::GEAR),
                    arclain_widgets::ButtonSize::Custom {
                        width: 80.0,
                        height: 28.0,
                    },
                )
                .with_theme_colors(&theme.colors)
            } else {
                arclain_widgets::TextButton::new(
                    egui_phosphor::regular::GEAR,
                    arclain_widgets::ButtonSize::Custom {
                        width: 32.0,
                        height: 28.0,
                    },
                )
                .with_theme_colors(&theme.colors)
            }
            .variant(if is_on_settings {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Ghost
            });

            let mut response = settings_btn.ui(ui);
            if !show_labels {
                response = response.on_hover_text("Settings");
            }
            if response.clicked() {
                actions.navigate_settings = true;
            }

            // Logs (replaces dead extension button)
            let logs_btn = arclain_widgets::TextButton::new(
                egui_phosphor::regular::CLIPBOARD_TEXT,
                arclain_widgets::ButtonSize::Custom {
                    width: 32.0,
                    height: 28.0,
                },
            )
            .with_theme_colors(&theme.colors)
            .variant(ButtonVariant::Ghost);

            if logs_btn.ui(ui).on_hover_text("View Logs").clicked() {
                actions.show_logs = true;
            }

            // Server connection status indicator (only when server is configured)
            if has_server_indicator {
                let dot_radius = 5.0;
                let (dot_rect, dot_response) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());

                let (dot_color, hover_text) = match server_status {
                    ServerConnectionStatus::Connected(version) => (
                        theme.colors.success,
                        format!("Gameta server connected ({})", version),
                    ),
                    ServerConnectionStatus::Error(msg) => (
                        theme.colors.error,
                        format!("Gameta server unavailable: {}", msg),
                    ),
                    ServerConnectionStatus::Offline => unreachable!(),
                };

                if ui.is_rect_visible(dot_rect) {
                    ui.painter()
                        .circle_filled(dot_rect.center(), dot_radius, dot_color);
                }
                dot_response.on_hover_text(hover_text);
            }
        });
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
