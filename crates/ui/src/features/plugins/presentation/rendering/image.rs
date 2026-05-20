//! Image rendering logic for plugins

use super::context::{RenderContext, UiEventHandler};
use crate::shared::image_fetcher::trigger_image_fetch;
use eframe::egui;

/// Render an Image element
pub fn render_image(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    cache_key: &Option<String>,
    url: &Option<String>,
    max_height: Option<f32>,
) {
    let colors = ctx.colors;

    // Try to load image from cache
    if let Some(key) = cache_key {
        if let Some(cache) = ctx.content_cache {
            match cache.get(key) {
                Ok(Some(bytes)) => {
                    // Try to decode image and display
                    if let Some(size) = try_render_image(ui, key, &bytes, max_height) {
                        // Successfully rendered
                        let _ = size;
                    } else {
                        // Failed to decode - delete bad cache entry and trigger re-fetch
                        tracing::debug!("Deleting corrupt cache entry: {}", key);
                        let _ = cache.remove(key);

                        // Trigger re-fetch if we have URL
                        if let Some(shared) = ctx.shared_state {
                            if let Some(url_str) = url {
                                trigger_image_fetch(
                                    shared,
                                    ctx.plugin_id.map(|s| s.to_string()),
                                    url_str.clone(),
                                    key.clone(),
                                    ui.ctx().clone(),
                                );
                            }
                        }

                        ui.label(
                            egui::RichText::new("🖼 [Reloading...]")
                                .color(colors.on_surface_variant)
                                .italics(),
                        );
                    }
                }
                Ok(None) => {
                    // Not in cache yet, try to fetch
                    if let Some(shared) = ctx.shared_state {
                        if let Some(url_str) = url {
                            // Check if we already triggered a fetch for this key
                            // Use a timestamp to allow retry after 30 seconds if fetch failed
                            let fetch_id = egui::Id::new(("fetch", key.as_str()));
                            let now = std::time::Instant::now();
                            let fetch_started: Option<std::time::Instant> =
                                ui.data(|d| d.get_temp(fetch_id));

                            let should_fetch = match fetch_started {
                                None => true,
                                Some(started) => now.duration_since(started).as_secs() > 30,
                            };

                            if should_fetch {
                                ui.data_mut(|d| d.insert_temp(fetch_id, now));
                                trigger_image_fetch(
                                    shared,
                                    ctx.plugin_id.map(|s| s.to_string()),
                                    url_str.clone(),
                                    key.clone(),
                                    ui.ctx().clone(),
                                );
                            }
                        }
                    }

                    ui.label(
                        egui::RichText::new(format!("🖼 [Loading: {}]", key))
                            .color(colors.on_surface_variant)
                            .italics(),
                    );
                }
                Err(e) => {
                    ui.label(
                        egui::RichText::new(format!("🖼 [Error: {}]", e))
                            .color(colors.error)
                            .italics(),
                    );
                }
            }
        } else {
            ui.label(
                egui::RichText::new("🖼 [No cache available]")
                    .color(colors.on_surface_variant)
                    .italics(),
            );
        }
    } else if let Some(url_str) = url {
        // URL without cache key - show placeholder
        ui.label(
            egui::RichText::new(format!("🖼 [Image: {}]", url_str))
                .color(colors.on_surface_variant)
                .italics(),
        );
    } else {
        ui.label(
            egui::RichText::new("🖼 [No image source]")
                .color(colors.on_surface_variant)
                .italics(),
        );
    }
}

/// Check if an image texture is already cached in egui (avoids disk I/O)
pub fn is_texture_cached(ctx: &egui::Context, cache_key: &str) -> bool {
    let texture_id = egui::Id::new(("plugin_image", cache_key));
    ctx.data(|d| d.get_temp::<egui::TextureHandle>(texture_id)).is_some()
}

/// Render an already-cached texture (fast path - no bytes needed)
/// Returns None if texture not cached
pub fn render_cached_texture(
    ui: &mut egui::Ui,
    cache_key: &str,
    max_height: Option<f32>,
) -> Option<egui::Vec2> {
    let ctx = ui.ctx();
    let texture_id = egui::Id::new(("plugin_image", cache_key));

    let handle: egui::TextureHandle = ctx.data(|d| d.get_temp(texture_id))?;

    // Calculate display size respecting max_height
    let tex_size = handle.size_vec2();
    let max_h = max_height.unwrap_or(200.0);
    let scale = if tex_size.y > max_h {
        max_h / tex_size.y
    } else {
        1.0
    };
    let display_size = egui::vec2(tex_size.x * scale, tex_size.y * scale);

    // Render the image
    ui.image(egui::load::SizedTexture {
        id: handle.id(),
        size: display_size,
    });

    Some(display_size)
}

/// Try to render an image from raw bytes
/// Returns the displayed size if successful, None if decoding failed
pub fn try_render_image(
    ui: &mut egui::Ui,
    cache_key: &str,
    bytes: &[u8],
    max_height: Option<f32>,
) -> Option<egui::Vec2> {
    let ctx = ui.ctx();

    // Generate a stable ID for this image's texture
    let texture_id = egui::Id::new(("plugin_image", cache_key));

    // Check if texture is already loaded in egui's memory
    let existing_handle: Option<egui::TextureHandle> = ctx.data(|d| d.get_temp(texture_id));

    let handle = if let Some(h) = existing_handle {
        h
    } else {
        // Try to decode the image bytes using the image crate
        let img = image::load_from_memory(bytes).ok()?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = rgba.into_raw();

        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);

        // Load into egui
        let handle = ctx.load_texture(cache_key, color_image, egui::TextureOptions::default());

        // Cache the handle for future frames
        ctx.data_mut(|d| d.insert_temp(texture_id, handle.clone()));

        handle
    };

    // Calculate display size respecting max_height
    let tex_size = handle.size_vec2();
    let max_h = max_height.unwrap_or(200.0);
    let scale = if tex_size.y > max_h {
        max_h / tex_size.y
    } else {
        1.0
    };
    let display_size = egui::vec2(tex_size.x * scale, tex_size.y * scale);

    // Render the image
    ui.image(egui::load::SizedTexture {
        id: handle.id(),
        size: display_size,
    });

    Some(display_size)
}

// `trigger_image_fetch` lives at `crate::shared::image_fetcher` post
// 2026-05-21 — it's generic infrastructure, not plugin-specific, and
// the carousel in `shared/components/` was reaching across the
// shared→features layering boundary to call it from here.
