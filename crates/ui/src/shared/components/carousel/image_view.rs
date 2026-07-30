//! Main image view component for carousel

use super::CarouselEvent;
use crate::shared::image_assets::{ImageAssetState, ImageOwner};
use crate::shared::{theme::ThemeColors, SharedState};
use eframe::egui;

/// Main image display widget
pub struct ImageView<'a> {
    cache_key: &'a str,
    image_url: Option<&'a str>,
    colors: Option<&'a ThemeColors>,
    shared_state: Option<&'a SharedState>,
    plugin_id: Option<&'a str>,
    image_owner: Option<&'a ImageOwner>,
    enable_lightbox: bool,
}

impl<'a> ImageView<'a> {
    pub fn new(cache_key: &'a str) -> Self {
        Self {
            cache_key,
            image_url: None,
            colors: None,
            shared_state: None,
            plugin_id: None,
            image_owner: None,
            enable_lightbox: true,
        }
    }

    pub fn colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn image_owner(mut self, owner: Option<&'a ImageOwner>) -> Self {
        self.image_owner = owner;
        self
    }

    /// Source URL for cache-miss fetches. Without this, the view spins
    /// forever when the bytes haven't been cached yet.
    pub fn image_url(mut self, url: Option<&'a str>) -> Self {
        self.image_url = url;
        self
    }

    /// SharedState gives access to the image-asset store and the tokio
    /// runtime for the cache-miss fetch path -- the request itself belongs
    /// to the application now, not to this frontend. Must be set together
    /// with `image_url`.
    pub fn shared_state(mut self, shared: Option<&'a SharedState>) -> Self {
        self.shared_state = shared;
        self
    }

    /// Plugin id for proxy / domain-whitelist scoping when the fetch fires.
    pub fn plugin_id(mut self, plugin_id: Option<&'a str>) -> Self {
        self.plugin_id = plugin_id;
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
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), colors.surface_variant);

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

        let state = if let (Some(shared), Some(owner)) = (self.shared_state, self.image_owner) {
            let state = shared
                .image_assets
                .request(owner.clone(), self.cache_key, ctx.clone());
            let texture = match state {
                ImageAssetState::Decoded => shared.image_assets.upload_ready(self.cache_key, ctx),
                ImageAssetState::Uploaded => shared.image_assets.get_texture(owner, self.cache_key),
                ImageAssetState::Loading | ImageAssetState::Failed(_) => None,
            };
            if let Some(texture) = texture {
                paint_image_centered(painter, &texture, rect);
                return true;
            }
            state
        } else {
            ImageAssetState::Failed("image asset store is unavailable".to_string())
        };

        // Cache miss or corrupt bytes — kick off a network fetch if we have a URL +
        //    SharedState. Throttled to once per 30s per cache_key (so we
        //    don't fire the same fetch every frame). The plugin emits the
        //    URL in its Carousel config; we just need to act on it.
        if matches!(state, ImageAssetState::Failed(_)) {
            if let (Some(url), Some(shared)) = (self.image_url, self.shared_state) {
                let fetch_id = egui::Id::new(("carousel_fetch", self.cache_key));
                let now = std::time::Instant::now();
                let last_fired: Option<std::time::Instant> = ctx.data(|d| d.get_temp(fetch_id));
                let should_fetch = match last_fired {
                    None => true,
                    Some(t) => now.duration_since(t).as_secs() > 30,
                };
                if should_fetch {
                    ctx.data_mut(|d| d.insert_temp(fetch_id, now));
                    crate::shared::image_fetcher::trigger_image_fetch(
                        shared,
                        self.plugin_id.map(|s| s.to_string()),
                        url.to_string(),
                        self.cache_key.to_string(),
                        ctx.clone(),
                    );
                }
            }
        }

        if matches!(state, ImageAssetState::Failed(_)) && self.image_url.is_none() {
            render_error(painter, rect, colors);
        } else {
            render_loading(painter, rect, colors);
        }
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
        let hint_rect =
            egui::Rect::from_center_size(hint_pos, hint_galley.size() + egui::vec2(16.0, 8.0));

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
