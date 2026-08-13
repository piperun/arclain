//! Top Tab Bar Component
//!
//! Renders the main application tab bar with host tabs and plugin-registered tabs.
//! Supports badge rendering (numeric count and dot indicators).

use arclain_app::plugins::{BadgeLevel, PluginBadgeDto};
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
    /// Optional badge, in the application facade's own vocabulary
    /// ([`PluginBadgeDto`]) rather than the plugin runtime's -- this
    /// component renders host tabs and plugin tabs through one shape and
    /// must not name a headless crate to do it.
    pub badge: Option<PluginBadgeDto>,
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

            // Debug overlay (EGUI_UI_DEBUG_GUIDELINES=1):
            //   1. Outer tab rect + center cross + tag with position+size.
            //   2. Inner content rect (after the Frame's inner_margin)
            //      vs the outer rect — gap arrows on each side. If the
            //      gaps are top=bottom and left=right, the content
            //      block is centered inside the tab's padding box.
            //      If top≠bottom, content is vertically off-center.
            //   3. Inner-rect center vs outer-rect center as a Δ — if
            //      Δy is non-zero, the visual centering is biased.
            #[cfg(debug_assertions)]
            {
                let enabled = arclain_widgets::ui_debug_guidelines_enabled();
                let outer = response.rect;
                // Same inner_margin numbers as the Frame above (left/right=12,
                // top/bottom=8). Could be derived from the Frame but
                // explicit is simpler.
                let inner = egui::Rect::from_min_max(
                    egui::pos2(outer.left() + 12.0, outer.top() + 8.0),
                    egui::pos2(outer.right() - 12.0, outer.bottom() - 8.0),
                );

                arclain_widgets::debug::paint_widget_rect_debug(
                    ui.painter(),
                    outer,
                    &format!("tab:{}", tab.id),
                    enabled,
                );
                arclain_widgets::debug::paint_child_in_parent_debug(
                    ui.painter(),
                    outer,
                    inner,
                    &format!("tab:{}.pad", tab.id),
                    enabled,
                );
                arclain_widgets::debug::paint_centering_debug(
                    ui.painter(),
                    outer,
                    inner,
                    &format!("tab:{}.Δ", tab.id),
                    enabled,
                );
            }

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
fn render_badge(ui: &mut Ui, badge: &PluginBadgeDto, colors: &ThemeColors) {
    let color = badge_color(badge.level, colors);

    if let Some(count) = badge.count {
        if count > 0 {
            // Compact numeric badge. Uses mesh_bounds-based visual centering
            // — egui's Align2::CENTER_CENTER centers the line-box (incl.
            // ascender/descender slack), making digits look top-heavy in
            // small containers. We do the layout + paint by hand so the
            // debug overlay below can show the actual painted mesh rect.
            let text = format!("{}", count);
            let font_id = egui::FontId::proportional(10.0);
            let text_color = colors.on_primary;

            let h_pad = 4.0_f32;
            let badge_height = 16.0_f32;

            // Layout first to learn the mesh extent (needed both for
            // sizing the rect and for visual-centering on its center).
            let galley = ui
                .painter()
                .layout_no_wrap(text, font_id.clone(), text_color);

            let badge_size = egui::vec2(
                (galley.mesh_bounds.width() + h_pad * 2.0)
                    .max(badge_height)
                    .ceil(),
                badge_height,
            );
            let (rect, _) = ui.allocate_exact_size(badge_size, egui::Sense::hover());

            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same((badge_height / 2.0) as u8),
                color,
            );

            // Paint at the offset that lands mesh_bounds.center on rect.center.
            // Equivalent to what arclain_widgets::layout_text_visually_centered
            // returns when given the real target_center.
            let paint_origin = rect.center() - galley.mesh_bounds.center().to_vec2();
            ui.painter()
                .galley(paint_origin, galley.clone(), text_color);

            // Debug overlay: gated by the project-wide
            // EGUI_UI_DEBUG_GUIDELINES env var. Set it before launching
            // to see container rect + mesh-bounds rect + Δ between
            // their centers.
            #[cfg(debug_assertions)]
            arclain_widgets::debug::paint_text_centering_debug(
                ui.painter(),
                rect,
                paint_origin,
                &galley,
                "badge",
                arclain_widgets::ui_debug_guidelines_enabled(),
            );
        }
    } else if badge.dot {
        // Dot badge
        let (_, rect) = ui.allocate_space(Vec2::splat(8.0));
        ui.painter().circle_filled(rect.center(), 4.0, color);
    }
}

/// Which theme token draws a badge of this level.
///
/// A plugin names what its badge means and the host answers with a
/// colour, so restyling moves every plugin's badge at once and no
/// plugin-authored text ever reaches a paint call. Four of the five
/// levels have a status token of their own; `Neutral` reports no status
/// at all and takes the app's own accent, which is the closest thing
/// `ThemeColors` has to "a badge with nothing to say".
fn badge_color(level: BadgeLevel, colors: &ThemeColors) -> Color32 {
    match level {
        BadgeLevel::Neutral => colors.primary,
        BadgeLevel::Info => colors.info,
        BadgeLevel::Success => colors.success,
        BadgeLevel::Warning => colors.warning,
        BadgeLevel::Error => colors.error,
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
    fn every_badge_level_takes_the_theme_token_that_means_the_same_thing() {
        let colors = test_colors();
        assert_eq!(badge_color(BadgeLevel::Info, &colors), colors.info);
        assert_eq!(badge_color(BadgeLevel::Success, &colors), colors.success);
        assert_eq!(badge_color(BadgeLevel::Warning, &colors), colors.warning);
        assert_eq!(badge_color(BadgeLevel::Error, &colors), colors.error);
    }

    /// `Neutral` is the one level with no status token behind it. It takes
    /// the app's accent, and the assertion is written against what that
    /// means rather than against a hex value: whatever the active theme
    /// makes of `primary`, a neutral badge follows it.
    #[test]
    fn a_neutral_badge_takes_the_apps_own_accent() {
        let colors = test_colors();
        assert_eq!(badge_color(BadgeLevel::Neutral, &colors), colors.primary);
    }

    /// No two levels may collapse onto one colour, or the vocabulary
    /// promises a distinction the screen does not keep. This is the
    /// assertion that fails if a theme is restyled carelessly -- and it is
    /// the reason `Neutral` does not take `on_surface_variant`, which the
    /// shipped theme sets to the same value as `info`.
    #[test]
    fn no_two_levels_render_as_the_same_colour() {
        for colors in [ThemeColors::light(), ThemeColors::dark()] {
            let mut seen: Vec<Color32> = Vec::new();
            for level in [
                BadgeLevel::Neutral,
                BadgeLevel::Info,
                BadgeLevel::Success,
                BadgeLevel::Warning,
                BadgeLevel::Error,
            ] {
                let color = badge_color(level, &colors);
                assert!(
                    !seen.contains(&color),
                    "{level:?} renders as {color:?}, which another level already took"
                );
                seen.push(color);
            }
        }
    }

    // =========================================================================
    // render_badge
    // =========================================================================

    /// Collect the fill of every dot-sized circle the frame painted.
    fn painted_dot_fills(shapes: Vec<egui::epaint::ClippedShape>) -> Vec<Color32> {
        fn walk(shape: &egui::Shape, found: &mut Vec<Color32>) {
            match shape {
                egui::Shape::Circle(circle) if circle.radius == 4.0 => found.push(circle.fill),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &shapes {
            walk(&clipped.shape, &mut found);
        }
        found
    }

    /// Renders one tab whose badge carries `level` and reports the colour
    /// the dot was actually painted with.
    fn painted_badge_color(level: BadgeLevel, colors: &ThemeColors) -> Color32 {
        let tabs = vec![TopTab {
            id: "t".to_string(),
            label: "T".to_string(),
            icon: "DATABASE".to_string(),
            badge: Some(PluginBadgeDto {
                count: None,
                dot: true,
                level,
            }),
            source: None,
        }];
        let mut state = TopTabBarState::new("t");
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 64.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render(ui, colors, &mut state, &tabs);
            });
        });

        let fills = painted_dot_fills(output.shapes);
        assert_eq!(fills.len(), 1, "one badge paints one dot, got {fills:?}");
        fills[0]
    }

    /// The table above compares the resolver to the theme, which cannot
    /// see whether the renderer reads the resolver at all. This one reads
    /// the colour off the painted circle, so a renderer that ignored the
    /// level -- or pinned one colour for every badge -- fails here.
    #[test]
    fn the_level_a_badge_names_is_the_colour_it_gets_painted() {
        let colors = test_colors();
        for level in [
            BadgeLevel::Neutral,
            BadgeLevel::Info,
            BadgeLevel::Success,
            BadgeLevel::Warning,
            BadgeLevel::Error,
        ] {
            assert_eq!(
                painted_badge_color(level, &colors),
                badge_color(level, &colors),
                "a {level:?} badge is painted with the colour the host assigns it"
            );
        }
    }

    /// The colour follows the theme rather than the plugin: the same badge
    /// drawn against a different theme comes out a different colour, with
    /// nothing on the plugin's side changed.
    #[test]
    fn the_same_badge_changes_colour_when_the_theme_does() {
        let light = painted_badge_color(BadgeLevel::Neutral, &ThemeColors::light());
        let dark = painted_badge_color(BadgeLevel::Neutral, &ThemeColors::dark());
        assert_ne!(light, dark);
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
