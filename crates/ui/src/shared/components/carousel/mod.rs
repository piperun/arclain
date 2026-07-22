//! Carousel gallery widget
//!
//! A reusable image gallery with:
//! - Main image display with click-to-expand
//! - Thumbnail strip with horizontal scroll
//! - Navigation arrows for image cycling
//!
//! ## Structure
//! ```text
//! [‹] [     main image      ] [›]
//! [‹] [  thumbnail strip    ] [›]
//!            1 / 6
//! ```

mod image_view;
mod nav_button;
mod thumbnail_strip;

pub use image_view::ImageView;
pub use nav_button::{NavButton, NavButtonStyle};
pub use thumbnail_strip::{ThumbnailStrip, ThumbnailStripStyle};

use crate::shared::image_assets::ImageOwner;
use crate::shared::{theme::ThemeColors, SharedState};
use eframe::egui;

/// Events emitted by the carousel
#[derive(Debug, Clone, PartialEq)]
pub enum CarouselEvent {
    /// Navigate to previous image
    Previous,
    /// Navigate to next image
    Next,
    /// Select image at index
    Select(usize),
    /// Open lightbox
    OpenLightbox,
}

/// Configuration for the carousel widget
#[derive(Clone)]
pub struct CarouselConfig {
    /// Height of the main image area
    pub main_height: f32,
    /// Height of thumbnails
    pub thumbnail_height: f32,
    /// Whether lightbox is enabled
    pub enable_lightbox: bool,
    /// Gap between nav buttons and content
    pub nav_gap: f32,
}

impl Default for CarouselConfig {
    fn default() -> Self {
        Self {
            main_height: 300.0,
            thumbnail_height: 60.0,
            enable_lightbox: true,
            nav_gap: 4.0,
        }
    }
}

/// Carousel widget state
pub struct Carousel<'a> {
    id: &'a str,
    images: &'a [(String, Option<String>)],
    current_index: usize,
    config: CarouselConfig,
    colors: Option<&'a ThemeColors>,
    shared_state: Option<&'a SharedState>,
    plugin_id: Option<&'a str>,
    image_owner: Option<&'a ImageOwner>,
}

impl<'a> Carousel<'a> {
    pub fn new(id: &'a str, images: &'a [(String, Option<String>)], current_index: usize) -> Self {
        Self {
            id,
            images,
            current_index,
            config: CarouselConfig::default(),
            colors: None,
            shared_state: None,
            plugin_id: None,
            image_owner: None,
        }
    }

    pub fn config(mut self, config: CarouselConfig) -> Self {
        self.config = config;
        self
    }

    pub fn main_height(mut self, height: f32) -> Self {
        self.config.main_height = height;
        self
    }

    pub fn thumbnail_height(mut self, height: f32) -> Self {
        self.config.thumbnail_height = height;
        self
    }

    pub fn enable_lightbox(mut self, enable: bool) -> Self {
        self.config.enable_lightbox = enable;
        self
    }

    pub fn colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn image_owner(mut self, owner: Option<&'a ImageOwner>) -> Self {
        self.image_owner = owner;
        self
    }

    /// SharedState — needed for cache-miss network fetches. Without it, the
    /// carousel images stay on a forever spinner because the image_view /
    /// thumbnail_strip never trigger a download for missing bytes.
    pub fn shared_state(mut self, shared: Option<&'a SharedState>) -> Self {
        self.shared_state = shared;
        self
    }

    pub fn plugin_id(mut self, plugin_id: Option<&'a str>) -> Self {
        self.plugin_id = plugin_id;
        self
    }

    /// Render the carousel and return any event that occurred
    pub fn show(self, ui: &mut egui::Ui) -> Option<CarouselEvent> {
        let fallback_colors = ThemeColors::dark();
        let colors = self.colors.unwrap_or(&fallback_colors);

        if self.images.is_empty() {
            ui.label(
                egui::RichText::new("No images")
                    .color(colors.on_surface_variant)
                    .italics(),
            );
            return None;
        }

        let current_idx = self.current_index.min(self.images.len().saturating_sub(1));
        let has_multiple = self.images.len() > 1;
        let mut event = None;

        // Layout constants
        let nav_button_width = 28.0;
        let thumb_nav_width = 24.0;
        let main_height = self.config.main_height;
        let nav_height = main_height.min(80.0);
        let thumb_row_height = self.config.thumbnail_height + 4.0;

        // Calculate content width - use available width minus nav buttons
        let available_width = ui.available_width();
        let nav_space = if has_multiple {
            (nav_button_width + self.config.nav_gap) * 2.0
        } else {
            0.0
        };
        let content_width = (available_width - nav_space).max(100.0);

        // Total carousel width
        let total_width = content_width + nav_space;

        // Calculate horizontal offset to center the carousel
        let offset_x = (available_width - total_width).max(0.0) / 2.0;

        ui.vertical(|ui| {
            // === MAIN IMAGE ROW ===
            // Allocate the full row height
            let (row_rect, _) = ui.allocate_exact_size(
                egui::vec2(available_width, main_height),
                egui::Sense::hover(),
            );

            // Calculate positions within the row
            let mut x = row_rect.left() + offset_x;

            // Left nav button (vertically centered)
            if has_multiple {
                let nav_rect = egui::Rect::from_center_size(
                    egui::pos2(x + nav_button_width / 2.0, row_rect.center().y),
                    egui::vec2(nav_button_width, nav_height),
                );
                x += nav_button_width + self.config.nav_gap;

                if NavButton::new("‹")
                    .id("main_prev")
                    .style(NavButtonStyle::default().height(nav_height))
                    .colors(colors)
                    .show_at(ui, nav_rect)
                    .clicked()
                {
                    event = Some(CarouselEvent::Previous);
                }
            }

            // Main image
            let image_rect = egui::Rect::from_min_size(
                egui::pos2(x, row_rect.top()),
                egui::vec2(content_width, main_height),
            );
            x += content_width;

            if let Some((cache_key, image_url)) = self.images.get(current_idx) {
                let image_event = ImageView::new(cache_key)
                    .image_url(image_url.as_deref())
                    .colors(colors)
                    .image_owner(self.image_owner)
                    .shared_state(self.shared_state)
                    .plugin_id(self.plugin_id)
                    .enable_lightbox(self.config.enable_lightbox)
                    .show_at(ui, image_rect);

                if image_event == Some(CarouselEvent::OpenLightbox) {
                    event = Some(CarouselEvent::OpenLightbox);
                }
            }

            // Right nav button (vertically centered)
            if has_multiple {
                x += self.config.nav_gap;
                let nav_rect = egui::Rect::from_center_size(
                    egui::pos2(x + nav_button_width / 2.0, row_rect.center().y),
                    egui::vec2(nav_button_width, nav_height),
                );

                if NavButton::new("›")
                    .id("main_next")
                    .style(NavButtonStyle::default().height(nav_height))
                    .colors(colors)
                    .show_at(ui, nav_rect)
                    .clicked()
                {
                    event = Some(CarouselEvent::Next);
                }
            }

            // === THUMBNAIL ROW ===
            if has_multiple {
                ui.add_space(8.0);

                // Allocate thumbnail row
                let (thumb_row_rect, _) = ui.allocate_exact_size(
                    egui::vec2(available_width, thumb_row_height),
                    egui::Sense::hover(),
                );

                // Calculate thumbnail layout - use same nav width for alignment
                let thumb_nav_space = (thumb_nav_width + self.config.nav_gap) * 2.0;
                let strip_width = (total_width - thumb_nav_space).max(100.0);
                let thumb_offset_x =
                    (available_width - (strip_width + thumb_nav_space)).max(0.0) / 2.0;

                let mut thumb_x = thumb_row_rect.left() + thumb_offset_x;

                // Left nav button
                let left_nav_rect = egui::Rect::from_center_size(
                    egui::pos2(thumb_x + thumb_nav_width / 2.0, thumb_row_rect.center().y),
                    egui::vec2(thumb_nav_width, thumb_row_height),
                );
                thumb_x += thumb_nav_width + self.config.nav_gap;

                if NavButton::new("‹")
                    .id("thumb_prev")
                    .style(NavButtonStyle::small().height(thumb_row_height))
                    .colors(colors)
                    .show_at(ui, left_nav_rect)
                    .clicked()
                {
                    event = Some(CarouselEvent::Previous);
                }

                // Thumbnail strip
                let strip_rect = egui::Rect::from_min_size(
                    egui::pos2(thumb_x, thumb_row_rect.top()),
                    egui::vec2(strip_width, thumb_row_height),
                );
                thumb_x += strip_width + self.config.nav_gap;

                let strip_event = ThumbnailStrip::new(self.id, self.images, current_idx)
                    .style(ThumbnailStripStyle::default().height(self.config.thumbnail_height))
                    .max_width(strip_width)
                    .colors(colors)
                    .image_owner(self.image_owner)
                    .shared_state(self.shared_state)
                    .plugin_id(self.plugin_id)
                    .show_at(ui, strip_rect);

                if let Some(idx) = strip_event {
                    event = Some(CarouselEvent::Select(idx));
                }

                // Right nav button
                let right_nav_rect = egui::Rect::from_center_size(
                    egui::pos2(thumb_x + thumb_nav_width / 2.0, thumb_row_rect.center().y),
                    egui::vec2(thumb_nav_width, thumb_row_height),
                );

                if NavButton::new("›")
                    .id("thumb_next")
                    .style(NavButtonStyle::small().height(thumb_row_height))
                    .colors(colors)
                    .show_at(ui, right_nav_rect)
                    .clicked()
                {
                    event = Some(CarouselEvent::Next);
                }
            }

            // === COUNTER (below carousel) ===
            if has_multiple {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} / {}", current_idx + 1, self.images.len()))
                            .size(12.0)
                            .color(colors.on_surface_variant),
                    );
                });
            }
        });

        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carousel_config_defaults() {
        let config = CarouselConfig::default();
        assert_eq!(config.main_height, 300.0);
        assert_eq!(config.thumbnail_height, 60.0);
        assert!(config.enable_lightbox);
        assert_eq!(config.nav_gap, 4.0);
    }

    #[test]
    fn carousel_builder_methods() {
        let images: Vec<(String, Option<String>)> = vec![
            ("key1".into(), Some("thumb1".into())),
            ("key2".into(), None),
        ];
        let carousel = Carousel::new("test", &images, 0)
            .main_height(400.0)
            .thumbnail_height(80.0)
            .enable_lightbox(false);

        assert_eq!(carousel.config.main_height, 400.0);
        assert_eq!(carousel.config.thumbnail_height, 80.0);
        assert!(!carousel.config.enable_lightbox);
        assert_eq!(carousel.id, "test");
        assert_eq!(carousel.current_index, 0);
        assert_eq!(carousel.images.len(), 2);
    }

    #[test]
    fn carousel_event_variants() {
        // Ensure all variants are distinct
        assert_ne!(CarouselEvent::Previous, CarouselEvent::Next);
        assert_ne!(CarouselEvent::Select(0), CarouselEvent::Select(1));
        assert_ne!(CarouselEvent::OpenLightbox, CarouselEvent::Previous);
    }
}
