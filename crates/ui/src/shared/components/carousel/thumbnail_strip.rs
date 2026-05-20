//! Thumbnail strip component for carousel

use crate::shared::{async_image, theme::ThemeColors, SharedState};
use arclain_core::ContentCache;
use eframe::egui;
use std::sync::Arc;

/// Style configuration for thumbnail strip
#[derive(Clone)]
pub struct ThumbnailStripStyle {
    /// Height of each thumbnail
    pub height: f32,
    /// Width/height ratio (default 1.2 = slightly wider)
    pub aspect_ratio: f32,
    /// Spacing between thumbnails
    pub spacing: f32,
    /// Corner radius
    pub corner_radius: u8,
}

impl Default for ThumbnailStripStyle {
    fn default() -> Self {
        Self {
            height: 60.0,
            aspect_ratio: 1.2,
            spacing: 6.0,
            corner_radius: 4,
        }
    }
}

impl ThumbnailStripStyle {
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    fn thumb_width(&self) -> f32 {
        self.height * self.aspect_ratio
    }

    fn thumb_size(&self) -> egui::Vec2 {
        egui::vec2(self.thumb_width() + 4.0, self.height + 4.0)
    }
}

/// Thumbnail strip widget
pub struct ThumbnailStrip<'a> {
    id: &'a str,
    images: &'a [(String, Option<String>)],
    current_index: usize,
    style: ThumbnailStripStyle,
    max_width: f32,
    colors: Option<&'a ThemeColors>,
    content_cache: Option<&'a Arc<ContentCache>>,
    shared_state: Option<&'a SharedState>,
    plugin_id: Option<&'a str>,
}

impl<'a> ThumbnailStrip<'a> {
    pub fn new(
        id: &'a str,
        images: &'a [(String, Option<String>)],
        current_index: usize,
    ) -> Self {
        Self {
            id,
            images,
            current_index,
            style: ThumbnailStripStyle::default(),
            max_width: 500.0,
            colors: None,
            content_cache: None,
            shared_state: None,
            plugin_id: None,
        }
    }

    pub fn style(mut self, style: ThumbnailStripStyle) -> Self {
        self.style = style;
        self
    }

    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = width;
        self
    }

    pub fn colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn content_cache(mut self, cache: Option<&'a Arc<ContentCache>>) -> Self {
        self.content_cache = cache;
        self
    }

    /// SharedState for cache-miss fetches.
    pub fn shared_state(mut self, shared: Option<&'a SharedState>) -> Self {
        self.shared_state = shared;
        self
    }

    pub fn plugin_id(mut self, plugin_id: Option<&'a str>) -> Self {
        self.plugin_id = plugin_id;
        self
    }

    /// Show the thumbnail strip
    /// Returns the index of clicked thumbnail, if any
    pub fn show(self, ui: &mut egui::Ui) -> Option<usize> {
        let max_width = self.max_width;
        self.render_inner(ui, max_width)
    }

    /// Show the thumbnail strip at a specific rect
    /// Returns the index of clicked thumbnail, if any
    pub fn show_at(self, ui: &mut egui::Ui, rect: egui::Rect) -> Option<usize> {
        let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        self.render_inner(&mut child_ui, rect.width())
    }

    fn render_inner(self, ui: &mut egui::Ui, max_width: f32) -> Option<usize> {
        let fallback_colors = ThemeColors::dark();
        let colors = self.colors.unwrap_or(&fallback_colors);
        let mut clicked_index = None;

        egui::ScrollArea::horizontal()
            .id_salt(format!("{}_thumbnails", self.id))
            .max_width(max_width)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = self.style.spacing;

                    for (i, (cache_key, url)) in self.images.iter().enumerate() {
                        let is_selected = i == self.current_index;
                        let thumb_size = self.style.thumb_size();

                        let (rect, response) = ui.allocate_exact_size(thumb_size, egui::Sense::click());
                        let is_hovered = response.hovered();

                        // Border color based on state
                        let border_color = if is_selected {
                            colors.primary
                        } else if is_hovered {
                            colors.on_surface_variant
                        } else {
                            colors.outline_variant
                        };

                        let border_width = if is_selected { 2.0 } else { 1.0 };

                        // Background
                        ui.painter().rect_filled(
                            rect,
                            egui::CornerRadius::same(self.style.corner_radius),
                            colors.surface_variant,
                        );

                        // Border
                        ui.painter().rect_stroke(
                            rect,
                            egui::CornerRadius::same(self.style.corner_radius),
                            egui::Stroke::new(border_width, border_color),
                            egui::StrokeKind::Inside,
                        );

                        // Render thumbnail
                        let inner_rect = rect.shrink(2.0);
                        self.render_thumbnail(ui, cache_key, url.as_deref(), inner_rect, colors);

                        // Handle click
                        if response.clicked() {
                            clicked_index = Some(i);
                        }

                        response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                });
            });

        clicked_index
    }

    fn render_thumbnail(
        &self,
        ui: &egui::Ui,
        cache_key: &str,
        url: Option<&str>,
        rect: egui::Rect,
        colors: &ThemeColors,
    ) {
        let ctx = ui.ctx();
        let painter = ui.painter();

        // 1. Check GPU texture cache
        if async_image::is_texture_cached(ctx, cache_key) {
            if let Some(handle) = async_image::get_texture_handle(ctx, cache_key) {
                paint_image_centered(painter, &handle, rect);
                return;
            }
        }

        // 2. Check if async decode completed
        if let Some(decoded) = async_image::get_decoded(ctx, cache_key) {
            let handle = async_image::upload_texture(ctx, cache_key, &decoded);
            paint_image_centered(painter, &handle, rect);
            return;
        }

        // 3. Decode in progress
        if async_image::is_decoding(ctx, cache_key) {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "⟳",
                egui::FontId::proportional(12.0),
                colors.on_surface_variant,
            );
            ctx.request_repaint();
            return;
        }

        // 4. Start decode from cache
        if let Some(cache) = self.content_cache {
            if let Ok(Some(bytes)) = cache.get(cache_key) {
                async_image::request_decode(ctx, cache_key, bytes);
            } else if let (Some(u), Some(shared)) = (url, self.shared_state) {
                // 5. Cache miss — fetch the image. Throttle to once / 30s
                //    per cache_key so we don't spam the network every frame.
                let fetch_id = egui::Id::new(("thumb_fetch", cache_key));
                let now = std::time::Instant::now();
                let last_fired: Option<std::time::Instant> =
                    ctx.data(|d| d.get_temp(fetch_id));
                let should_fetch = match last_fired {
                    None => true,
                    Some(t) => now.duration_since(t).as_secs() > 30,
                };
                if should_fetch {
                    ctx.data_mut(|d| d.insert_temp(fetch_id, now));
                    crate::shared::image_fetcher::trigger_image_fetch(
                        shared,
                        self.plugin_id.map(|s| s.to_string()),
                        u.to_string(),
                        cache_key.to_string(),
                        ctx.clone(),
                    );
                }
            }
        }

        // Show loading indicator
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "⟳",
            egui::FontId::proportional(12.0),
            colors.on_surface_variant,
        );
    }
}

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
