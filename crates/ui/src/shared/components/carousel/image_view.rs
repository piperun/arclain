//! Main image view component for carousel

use super::CarouselEvent;
use crate::shared::{async_image, theme::ThemeColors};
use arclain_data::ContentCache;
use eframe::egui;
use std::sync::Arc;

/// Main image display widget
pub struct ImageView<'a> {
    cache_key: &'a str,
    colors: Option<&'a ThemeColors>,
    content_cache: Option<&'a Arc<ContentCache>>,
    enable_lightbox: bool,
}

impl<'a> ImageView<'a> {
    pub fn new(cache_key: &'a str) -> Self {
        Self {
            cache_key,
            colors: None,
            content_cache: None,
            enable_lightbox: true,
        }
    }

    pub fn colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn content_cache(mut self, cache: Option<&'a Arc<ContentCache>>) -> Self {
        self.content_cache = cache;
        self
    }

    pub fn enable_lightbox(mut self, enable: bool) -> Self {
        self.enable_lightbox = enable;
        self
    }

    /// Show the image at a specific rect
    /// Returns CarouselEvent::OpenLightbox if clicked
    pub fn show_at(self, ui: &mut egui::Ui, rect: egui::Rect) -> Option<CarouselEvent> {
        let fallback_colors = ThemeColors::dark();
        let colors = self.colors.unwrap_or(&fallback_colors);

        // Draw background
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(8),
            colors.surface_variant,
        );

        // Render image
        let image_rendered = self.render_image(ui, rect, colors);

        // Make clickable
        let response = ui.interact(
            rect,
            ui.id().with(("image_view", self.cache_key)),
            egui::Sense::click(),
        );

        // Store state before consuming response
        let is_hovered = response.hovered();
        let was_clicked = response.clicked();

        // Hover hint
        if self.enable_lightbox && is_hovered && image_rendered {
            self.render_expand_hint(ui, rect);
        }

        // Cursor
        if self.enable_lightbox && image_rendered {
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
        }

        // Return event if clicked
        if self.enable_lightbox && was_clicked {
            Some(CarouselEvent::OpenLightbox)
        } else {
            None
        }
    }

    fn render_image(&self, ui: &egui::Ui, rect: egui::Rect, colors: &ThemeColors) -> bool {
        let ctx = ui.ctx();
        let painter = ui.painter();

        // 1. Check GPU texture cache
        if async_image::is_texture_cached(ctx, self.cache_key) {
            if let Some(handle) = async_image::get_texture_handle(ctx, self.cache_key) {
                paint_image_centered(painter, &handle, rect);
                return true;
            }
        }

        // 2. Check if async decode completed
        if let Some(decoded) = async_image::get_decoded(ctx, self.cache_key) {
            let handle = async_image::upload_texture(ctx, self.cache_key, &decoded);
            paint_image_centered(painter, &handle, rect);
            return true;
        }

        // 3. Decode in progress
        if async_image::is_decoding(ctx, self.cache_key) {
            render_loading(painter, rect, colors);
            ctx.request_repaint();
            return false;
        }

        // 4. Decode failed
        if async_image::decode_failed(ctx, self.cache_key) {
            render_error(painter, rect, colors);
            return false;
        }

        // 5. Start decode from cache
        if let Some(cache) = self.content_cache {
            if let Ok(Some(bytes)) = cache.get(self.cache_key) {
                async_image::request_decode(ctx, self.cache_key, bytes);
                render_loading(painter, rect, colors);
                ctx.request_repaint();
                return false;
            }
        }

        render_loading(painter, rect, colors);
        false
    }

    fn render_expand_hint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let hint_text = "🔍 Click to expand";
        let hint_pos = rect.center_bottom() - egui::vec2(0.0, 12.0);

        let hint_galley = ui.painter().layout_no_wrap(
            hint_text.to_string(),
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        let hint_rect = egui::Rect::from_center_size(
            hint_pos,
            hint_galley.size() + egui::vec2(16.0, 8.0),
        );

        ui.painter().rect_filled(
            hint_rect,
            egui::CornerRadius::same(4),
            egui::Color32::from_black_alpha(200),
        );

        ui.painter().text(
            hint_rect.center(),
            egui::Align2::CENTER_CENTER,
            hint_text,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
    }
}

/// Paint image centered within rect, maintaining aspect ratio
fn paint_image_centered(painter: &egui::Painter, handle: &egui::TextureHandle, rect: egui::Rect) {
    let tex_size = handle.size_vec2();
    let scale = (rect.width() / tex_size.x).min(rect.height() / tex_size.y);
    let display_size = tex_size * scale;
    let image_rect = egui::Rect::from_center_size(rect.center(), display_size);

    painter.image(
        handle.id(),
        image_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

fn render_loading(painter: &egui::Painter, rect: egui::Rect, colors: &ThemeColors) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "⟳",
        egui::FontId::proportional(32.0),
        colors.on_surface_variant,
    );
}

fn render_error(painter: &egui::Painter, rect: egui::Rect, colors: &ThemeColors) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "🖼 Failed",
        egui::FontId::proportional(14.0),
        colors.error,
    );
}
