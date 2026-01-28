//! Full-screen lightbox overlay for viewing images
//!
//! Provides a modal image viewer with keyboard navigation and overlay controls.

use crate::shared::theme::AppTheme;
use arclain_data::ContentCache;
use eframe::egui;
use std::sync::Arc;

/// State for the full-screen image lightbox
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightboxState {
    /// Whether the lightbox is currently shown
    pub show: bool,
    /// List of images: (cache_key, optional_url)
    pub images: Vec<(String, Option<String>)>,
    /// Current image index
    pub current_index: usize,
    /// Optional title (e.g., product name)
    pub title: Option<String>,
    /// Plugin ID that opened the lightbox (for event callbacks)
    pub source_plugin: Option<String>,
}

impl LightboxState {
    /// Open the lightbox with the given images
    pub fn open(images: Vec<(String, Option<String>)>, start_index: usize, title: Option<String>) -> Self {
        let clamped_index = start_index.min(images.len().saturating_sub(1));
        Self {
            show: true,
            images,
            current_index: clamped_index,
            title,
            source_plugin: None,
        }
    }

    /// Close the lightbox
    pub fn close(&mut self) {
        self.show = false;
    }

    /// Navigate to the next image
    pub fn next(&mut self) {
        if !self.images.is_empty() {
            self.current_index = (self.current_index + 1) % self.images.len();
        }
    }

    /// Navigate to the previous image
    pub fn prev(&mut self) {
        if !self.images.is_empty() {
            self.current_index = if self.current_index == 0 {
                self.images.len() - 1
            } else {
                self.current_index - 1
            };
        }
    }

    /// Go to a specific image index
    pub fn go_to(&mut self, index: usize) {
        if index < self.images.len() {
            self.current_index = index;
        }
    }

    /// Get the current image (cache_key, url)
    pub fn current_image(&self) -> Option<&(String, Option<String>)> {
        self.images.get(self.current_index)
    }

    /// Get the total number of images
    pub fn image_count(&self) -> usize {
        self.images.len()
    }
}

/// Result from rendering the lightbox
#[derive(Debug, Clone, PartialEq)]
pub enum LightboxResult {
    /// No action taken
    None,
    /// Lightbox was closed
    Closed,
    /// Image was changed to the given index
    ImageChanged(usize),
}

/// Render the lightbox overlay
pub fn render_lightbox(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &mut LightboxState,
    content_cache: Option<&Arc<ContentCache>>,
) -> LightboxResult {
    if !state.show || state.images.is_empty() {
        return LightboxResult::None;
    }

    let mut result = LightboxResult::None;

    // Handle keyboard input first (before rendering)
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Escape) {
            result = LightboxResult::Closed;
        } else if i.key_pressed(egui::Key::ArrowLeft) {
            state.prev();
            result = LightboxResult::ImageChanged(state.current_index);
        } else if i.key_pressed(egui::Key::ArrowRight) {
            state.next();
            result = LightboxResult::ImageChanged(state.current_index);
        } else if i.key_pressed(egui::Key::Home) {
            state.go_to(0);
            result = LightboxResult::ImageChanged(state.current_index);
        } else if i.key_pressed(egui::Key::End) {
            state.go_to(state.images.len().saturating_sub(1));
            result = LightboxResult::ImageChanged(state.current_index);
        }
    });

    // If we already decided to close, do it now
    if result == LightboxResult::Closed {
        state.close();
        return result;
    }

    let screen = ctx.input(|i| i.viewport_rect());

    // Dark overlay that captures clicks
    egui::Area::new(egui::Id::new("lightbox_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            // Paint dark background
            ui.painter().rect_filled(
                screen,
                0.0,
                egui::Color32::from_black_alpha(230),
            );

            // Allocate the full screen and check for clicks outside the image area
            let overlay_response = ui.allocate_rect(screen, egui::Sense::click());

            // Calculate image area (centered, with padding for navigation)
            let nav_button_width = 60.0;
            let top_bar_height = 50.0;
            let bottom_bar_height = 40.0;

            let image_area = egui::Rect::from_min_max(
                egui::pos2(nav_button_width, top_bar_height),
                egui::pos2(screen.width() - nav_button_width, screen.height() - bottom_bar_height),
            );

            // Render the current image
            let mut image_rect = egui::Rect::NOTHING;
            if let Some((cache_key, _url)) = state.current_image() {
                if let Some(cache) = content_cache {
                    if let Ok(Some(bytes)) = cache.get(cache_key) {
                        if let Some((handle, tex_size)) = load_texture_from_bytes(ctx, cache_key, &bytes) {
                            // Scale image to fit within image_area while maintaining aspect ratio
                            let available = image_area.size();
                            let scale = (available.x / tex_size.x).min(available.y / tex_size.y).min(1.0);
                            let display_size = egui::vec2(tex_size.x * scale, tex_size.y * scale);

                            // Center the image in the available area
                            let image_pos = egui::pos2(
                                image_area.center().x - display_size.x / 2.0,
                                image_area.center().y - display_size.y / 2.0,
                            );
                            image_rect = egui::Rect::from_min_size(image_pos, display_size);

                            // Paint the image
                            ui.painter().image(
                                handle.id(),
                                image_rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                    } else {
                        // Loading state
                        let center = image_area.center();
                        ui.painter().text(
                            center,
                            egui::Align2::CENTER_CENTER,
                            "Loading...",
                            egui::FontId::proportional(16.0),
                            theme.colors.on_surface,
                        );
                    }
                }
            }

            // Close button (top-right)
            let close_rect = egui::Rect::from_min_size(
                egui::pos2(screen.width() - 50.0, 10.0),
                egui::vec2(40.0, 40.0),
            );
            let close_response = ui.allocate_rect(close_rect, egui::Sense::click());
            let close_color = if close_response.hovered() {
                theme.colors.error
            } else {
                theme.colors.on_surface
            };
            ui.painter().text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                "✕",
                egui::FontId::proportional(24.0),
                close_color,
            );
            if close_response.clicked() {
                result = LightboxResult::Closed;
            }

            // Navigation arrows (only if more than one image)
            if state.images.len() > 1 {
                // Left arrow
                let left_rect = egui::Rect::from_min_size(
                    egui::pos2(10.0, screen.height() / 2.0 - 30.0),
                    egui::vec2(40.0, 60.0),
                );
                let left_response = ui.allocate_rect(left_rect, egui::Sense::click());
                let left_color = if left_response.hovered() {
                    theme.colors.primary
                } else {
                    theme.colors.on_surface
                };
                ui.painter().text(
                    left_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "◀",
                    egui::FontId::proportional(32.0),
                    left_color,
                );
                if left_response.clicked() {
                    state.prev();
                    result = LightboxResult::ImageChanged(state.current_index);
                }

                // Right arrow
                let right_rect = egui::Rect::from_min_size(
                    egui::pos2(screen.width() - 50.0, screen.height() / 2.0 - 30.0),
                    egui::vec2(40.0, 60.0),
                );
                let right_response = ui.allocate_rect(right_rect, egui::Sense::click());
                let right_color = if right_response.hovered() {
                    theme.colors.primary
                } else {
                    theme.colors.on_surface
                };
                ui.painter().text(
                    right_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "▶",
                    egui::FontId::proportional(32.0),
                    right_color,
                );
                if right_response.clicked() {
                    state.next();
                    result = LightboxResult::ImageChanged(state.current_index);
                }
            }

            // Image counter (bottom center)
            let counter_text = format!("{} / {}", state.current_index + 1, state.images.len());
            ui.painter().text(
                egui::pos2(screen.width() / 2.0, screen.height() - 20.0),
                egui::Align2::CENTER_CENTER,
                counter_text,
                egui::FontId::proportional(14.0),
                theme.colors.on_surface_variant,
            );

            // Title (top center, if provided)
            if let Some(title) = &state.title {
                ui.painter().text(
                    egui::pos2(screen.width() / 2.0, 25.0),
                    egui::Align2::CENTER_CENTER,
                    title,
                    egui::FontId::proportional(16.0),
                    theme.colors.on_surface,
                );
            }

            // Click outside image to close (but not on navigation buttons)
            if overlay_response.clicked() {
                // Check if click was outside the image rect and navigation areas
                if let Some(pos) = overlay_response.interact_pointer_pos() {
                    let on_close_button = close_rect.contains(pos);
                    let on_left_nav = state.images.len() > 1 && pos.x < nav_button_width;
                    let on_right_nav = state.images.len() > 1 && pos.x > screen.width() - nav_button_width;
                    let on_image = image_rect.contains(pos);

                    if !on_close_button && !on_left_nav && !on_right_nav && !on_image {
                        result = LightboxResult::Closed;
                    }
                }
            }
        });

    // Apply close if needed
    if result == LightboxResult::Closed {
        state.close();
    }

    result
}

/// Load a texture from raw bytes, returning the handle and original size
fn load_texture_from_bytes(
    ctx: &egui::Context,
    cache_key: &str,
    bytes: &[u8],
) -> Option<(egui::TextureHandle, egui::Vec2)> {
    let texture_id = egui::Id::new(("lightbox_image", cache_key));

    // Check if texture is already loaded
    let existing: Option<(egui::TextureHandle, egui::Vec2)> = ctx.data(|d| d.get_temp(texture_id));

    if let Some(cached) = existing {
        return Some(cached);
    }

    // Try to decode the image
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let size = egui::vec2(rgba.width() as f32, rgba.height() as f32);
    let pixels = rgba.into_raw();

    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [size.x as usize, size.y as usize],
        &pixels,
    );

    let handle = ctx.load_texture(
        format!("lightbox_{}", cache_key),
        color_image,
        egui::TextureOptions::default(),
    );

    // Cache for future frames
    ctx.data_mut(|d| d.insert_temp(texture_id, (handle.clone(), size)));

    Some((handle, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lightbox_state_navigation() {
        let images = vec![
            ("key1".to_string(), None),
            ("key2".to_string(), None),
            ("key3".to_string(), None),
        ];
        let mut state = LightboxState::open(images, 0, None);

        assert_eq!(state.current_index, 0);
        assert!(state.show);

        state.next();
        assert_eq!(state.current_index, 1);

        state.next();
        assert_eq!(state.current_index, 2);

        // Wrap around
        state.next();
        assert_eq!(state.current_index, 0);

        // Previous wraps back
        state.prev();
        assert_eq!(state.current_index, 2);

        // Go to specific
        state.go_to(1);
        assert_eq!(state.current_index, 1);

        // Close
        state.close();
        assert!(!state.show);
    }

    #[test]
    fn test_lightbox_empty_images() {
        let state = LightboxState::open(vec![], 0, None);
        assert_eq!(state.current_index, 0);
        assert!(state.current_image().is_none());
    }

    #[test]
    fn test_lightbox_start_index_clamping() {
        let images = vec![
            ("key1".to_string(), None),
            ("key2".to_string(), None),
        ];
        // Start index beyond length should be clamped
        let state = LightboxState::open(images, 10, None);
        assert_eq!(state.current_index, 1); // Clamped to last valid index
    }
}
