//! Top Tab Bar Component
//!
//! Renders the main application tab bar with host tabs and plugin-registered tabs.
//! Supports badge rendering (numeric count and dot indicators).

use arclain_plugins::BadgeConfig;
use arclain_theme::ThemeColors;
use egui::{Color32, RichText, Ui, Vec2};

/// A top-level tab definition (host or plugin)
#[derive(Debug, Clone)]
pub struct TopTab {
    /// Tab identifier
    pub id: String,
    /// Display label
    pub label: String,
    /// Icon (emoji or text)
    pub icon: String,
    /// Optional badge
    pub badge: Option<BadgeConfig>,
    /// Source: None for host, Some(plugin_id) for plugin
    pub source: Option<String>,
}

/// State for the top tab bar
#[derive(Debug, Clone, Default)]
pub struct TopTabBarState {
    /// Currently selected tab ID
    pub selected_tab: String,
}

impl TopTabBarState {
    pub fn new(default_tab: &str) -> Self {
        Self {
            selected_tab: default_tab.to_string(),
        }
    }
}

/// Actions returned from tab bar interactions
#[derive(Debug, Clone)]
pub enum TopTabAction {
    /// User clicked a host tab
    SelectHostTab(String),
    /// User clicked a plugin tab
    SelectPluginTab { plugin_id: String, tab_id: String },
}

/// Render the top tab bar
pub fn render(
    ui: &mut Ui,
    colors: &ThemeColors,
    state: &mut TopTabBarState,
    tabs: &[TopTab],
) -> Option<TopTabAction> {
    let mut action = None;

    // Use horizontal_centered to align all tabs vertically in the center of the bar
    ui.horizontal_centered(|ui| {
        ui.add_space(8.0);

        for tab in tabs {
            let is_selected = state.selected_tab == tab.id;

            // Tab styling
            let bg_color = if is_selected {
                colors.surface_variant
            } else {
                Color32::TRANSPARENT
            };

            let text_color = if is_selected {
                colors.on_surface
            } else {
                colors.on_surface_variant
            };

            // Render tab button
            // We use a Frame to handle padding and background
            let frame_response = egui::Frame::NONE
                .fill(bg_color)
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 12,
                    top: 8,
                    bottom: 8,
                })
                .corner_radius(4.0)
                .show(ui, |ui| {
                    // Use horizontal_centered to align icon and text vertically
                    ui.horizontal_centered(|ui| {
                        // Icon
                        let icon_glyph = icon_to_phosphor(&tab.icon);
                        ui.label(RichText::new(icon_glyph).color(text_color).size(14.0));

                        // Label - Using Text widget for consistency if possible, or just standardized RichText
                        ui.label(RichText::new(&tab.label).color(text_color).size(13.0));

                        // Badge
                        if let Some(badge) = &tab.badge {
                            render_badge(ui, badge, colors);
                        }
                    });
                });

            let response = frame_response.response;

            // Handle click
            if response.interact(egui::Sense::click()).clicked() {
                state.selected_tab = tab.id.clone();
                action = Some(if let Some(plugin_id) = &tab.source {
                    TopTabAction::SelectPluginTab {
                        plugin_id: plugin_id.clone(),
                        tab_id: tab.id.clone(),
                    }
                } else {
                    TopTabAction::SelectHostTab(tab.id.clone())
                });
            }

            // Hover effect
            if response.hovered() && !is_selected {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
    });

    action
}

/// Render a badge (count or dot)
fn render_badge(ui: &mut Ui, badge: &BadgeConfig, colors: &ThemeColors) {
    let color = badge_color(&badge.color, colors);

    if let Some(count) = badge.count {
        if count > 0 {
            // Numeric badge
            ui.label(
                RichText::new(format!("{}", count))
                    .size(10.0)
                    .color(Color32::WHITE)
                    .background_color(color),
            );
        }
    } else if badge.dot {
        // Dot badge
        let (_, rect) = ui.allocate_space(Vec2::splat(8.0));
        ui.painter().circle_filled(rect.center(), 4.0, color);
    }
}

/// Map a plugin-emitted badge color name to a theme token.
///
/// Plugins describe badge color semantically ("red" for failure,
/// "green" for success, etc.); this routes through `ThemeColors` so
/// the actual hex follows the active theme rather than being baked
/// into the renderer.
fn badge_color(color: &str, colors: &ThemeColors) -> Color32 {
    match color {
        "red" => colors.error,
        "green" => colors.success,
        "blue" => colors.primary,
        "orange" => colors.warning,
        _ => colors.primary,
    }
}

/// Convert icon name to Phosphor icon glyph
/// If the icon is already a glyph (single char or Phosphor char), return as-is
fn icon_to_phosphor(icon: &str) -> &str {
    match icon {
        "FOLDER_OPEN" => egui_phosphor::regular::FOLDER_OPEN,
        "MAGNIFYING_GLASS" => egui_phosphor::regular::MAGNIFYING_GLASS,
        "GLOBE" => egui_phosphor::regular::GLOBE,
        "GEAR" => egui_phosphor::regular::GEAR,
        "PUZZLE_PIECE" => egui_phosphor::regular::PUZZLE_PIECE,
        "HOUSE" => egui_phosphor::regular::HOUSE,
        "FILE" => egui_phosphor::regular::FILE,
        "INFO" => egui_phosphor::regular::INFO,
        "DATABASE" => egui_phosphor::regular::DATABASE,
        // If not a recognized name, return as-is (could be a Phosphor glyph already)
        _ => icon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // TopTabBarState
    // =========================================================================

    #[test]
    fn tab_bar_state_new() {
        let state = TopTabBarState::new("browser");
        assert_eq!(state.selected_tab, "browser");
    }

    #[test]
    fn tab_bar_state_default_is_empty() {
        let state = TopTabBarState::default();
        assert_eq!(state.selected_tab, "");
    }

    // =========================================================================
    // badge_color
    // =========================================================================

    fn test_colors() -> ThemeColors {
        ThemeColors::default()
    }

    #[test]
    fn badge_color_red_maps_to_theme_error() {
        let colors = test_colors();
        assert_eq!(badge_color("red", &colors), colors.error);
    }

    #[test]
    fn badge_color_green_maps_to_theme_success() {
        let colors = test_colors();
        assert_eq!(badge_color("green", &colors), colors.success);
    }

    #[test]
    fn badge_color_blue_maps_to_theme_primary() {
        let colors = test_colors();
        assert_eq!(badge_color("blue", &colors), colors.primary);
    }

    #[test]
    fn badge_color_orange_maps_to_theme_warning() {
        let colors = test_colors();
        assert_eq!(badge_color("orange", &colors), colors.warning);
    }

    #[test]
    fn badge_color_unknown_defaults_to_primary() {
        let colors = test_colors();
        assert_eq!(badge_color("purple", &colors), colors.primary);
        assert_eq!(badge_color("", &colors), colors.primary);
    }

    // =========================================================================
    // icon_to_phosphor
    // =========================================================================

    #[test]
    fn icon_to_phosphor_known_names() {
        assert_eq!(
            icon_to_phosphor("FOLDER_OPEN"),
            egui_phosphor::regular::FOLDER_OPEN
        );
        assert_eq!(icon_to_phosphor("GEAR"), egui_phosphor::regular::GEAR);
        assert_eq!(icon_to_phosphor("HOUSE"), egui_phosphor::regular::HOUSE);
        assert_eq!(
            icon_to_phosphor("DATABASE"),
            egui_phosphor::regular::DATABASE
        );
    }

    #[test]
    fn icon_to_phosphor_unknown_returns_as_is() {
        assert_eq!(icon_to_phosphor("custom_icon"), "custom_icon");
        assert_eq!(icon_to_phosphor(""), "");
    }
}
