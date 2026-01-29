//! Carousel gallery widget for plugins
//!
//! Displays images in a carousel with thumbnail strip and navigation arrows.

use super::async_image;
use super::context::{RenderContext, UiEventHandler};
use super::image::{is_texture_cached, render_cached_texture};
use eframe::egui;

/// Render a carousel gallery widget
pub fn render_carousel<H: UiEventHandler + ?Sized>(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, H>,
    id: &str,
    images: &[(String, Option<String>)],
    current_index: usize,
    max_height: Option<f32>,
    thumbnail_height: Option<f32>,
    enable_lightbox: bool,
) {
    let colors = ctx.colors;

    if images.is_empty() {
        ui.label(
            egui::RichText::new("No images")
                .color(colors.on_surface_variant)
                .italics(),
        );
        return;
    }

    let thumb_height = thumbnail_height.unwrap_or(60.0);
    let main_height = max_height.unwrap_or(300.0);
    let current_idx = current_index.min(images.len().saturating_sub(1));

    ui.vertical(|ui| {
        // Main image display with navigation arrows
        ui.horizontal(|ui| {
            // Left arrow (only if more than one image)
            if images.len() > 1 {
                let can_prev = true; // Wrap-around navigation
                let arrow_response = ui.add_enabled(
                    can_prev,
                    egui::Button::new(
                        egui::RichText::new("◀")
                            .size(20.0)
                            .color(if can_prev {
                                colors.on_surface
                            } else {
                                colors.on_surface_variant
                            }),
                    )
                    .frame(false),
                );
                if arrow_response.clicked() {
                    (ctx.event_callback)(&format!("{}_prev", id), None);
                }
            }

            // Main image (clickable for lightbox)
            if let Some((cache_key, _url)) = images.get(current_idx) {
                let main_width = ui.available_width() - if images.len() > 1 { 60.0 } else { 0.0 };

                // Helper to render main image with click handling
                let render_main = |ui: &mut egui::Ui, size: egui::Vec2| -> egui::Response {
                    let rect = egui::Rect::from_min_size(ui.min_rect().min, size);
                    ui.allocate_rect(rect, egui::Sense::click())
                };

                // 1. Fast path: texture already in GPU memory
                if is_texture_cached(ui.ctx(), cache_key) {
                    let response = ui.allocate_ui(egui::vec2(main_width, main_height), |ui| {
                        if let Some(size) = render_cached_texture(ui, cache_key, Some(main_height)) {
                            render_main(ui, size)
                        } else {
                            ui.label(egui::RichText::new("🖼 [Invalid]").color(colors.on_surface_variant).italics());
                            ui.response()
                        }
                    });
                    if enable_lightbox && response.inner.clicked() {
                        (ctx.event_callback)(&format!("{}_open_lightbox", id), None);
                    }
                }
                // 2. Async decode complete: upload and render
                else if let Some(decoded) = async_image::get_decoded(ui.ctx(), cache_key) {
                    let response = ui.allocate_ui(egui::vec2(main_width, main_height), |ui| {
                        if let Some(size) = async_image::upload_and_render(ui, cache_key, &decoded, Some(main_height)) {
                            render_main(ui, size)
                        } else {
                            ui.label(egui::RichText::new("🖼 [Invalid]").color(colors.on_surface_variant).italics());
                            ui.response()
                        }
                    });
                    if enable_lightbox && response.inner.clicked() {
                        (ctx.event_callback)(&format!("{}_open_lightbox", id), None);
                    }
                }
                // 3. Decode in progress or failed
                else if async_image::is_decoding(ui.ctx(), cache_key) {
                    ui.add_sized(egui::vec2(main_width.min(200.0), main_height), egui::Spinner::new().size(32.0));
                }
                else if async_image::decode_failed(ui.ctx(), cache_key) {
                    ui.label(egui::RichText::new("🖼 [Decode failed]").color(colors.error).italics());
                }
                // 4. Start async decode
                else if let Some(cache) = ctx.content_cache {
                    match cache.get(cache_key) {
                        Ok(Some(bytes)) => {
                            async_image::request_decode(ui.ctx(), cache_key, bytes);
                            ui.add_sized(egui::vec2(main_width.min(200.0), main_height), egui::Spinner::new().size(32.0));
                        }
                        Ok(None) => {
                            ui.add_sized(egui::vec2(main_width.min(200.0), main_height), egui::Spinner::new().size(32.0));
                        }
                        Err(e) => {
                            ui.label(egui::RichText::new(format!("🖼 [Error: {}]", e)).color(colors.error).italics());
                        }
                    }
                } else {
                    ui.label(egui::RichText::new("🖼 [No cache]").color(colors.on_surface_variant).italics());
                }
            }

            // Right arrow (only if more than one image)
            if images.len() > 1 {
                let can_next = true; // Wrap-around navigation
                let arrow_response = ui.add_enabled(
                    can_next,
                    egui::Button::new(
                        egui::RichText::new("▶")
                            .size(20.0)
                            .color(if can_next {
                                colors.on_surface
                            } else {
                                colors.on_surface_variant
                            }),
                    )
                    .frame(false),
                );
                if arrow_response.clicked() {
                    (ctx.event_callback)(&format!("{}_next", id), None);
                }
            }
        });

        ui.add_space(8.0);

        // Thumbnail strip (only if more than one image)
        if images.len() > 1 {
            egui::ScrollArea::horizontal()
                .id_salt(format!("{}_thumbnails", id))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        for (i, (cache_key, _url)) in images.iter().enumerate() {
                            let is_selected = i == current_idx;

                            // Create a frame for the thumbnail
                            let frame = if is_selected {
                                egui::Frame::NONE
                                    .stroke(egui::Stroke::new(2.0, colors.primary))
                                    .inner_margin(2.0)
                            } else {
                                egui::Frame::NONE
                                    .stroke(egui::Stroke::new(1.0, colors.outline_variant))
                                    .inner_margin(2.0)
                            };

                            let thumb_response = frame.show(ui, |ui| {
                                // 1. Fast path: texture already in GPU memory
                                if is_texture_cached(ui.ctx(), cache_key) {
                                    if render_cached_texture(ui, cache_key, Some(thumb_height)).is_none() {
                                        ui.add_sized(
                                            egui::vec2(thumb_height, thumb_height),
                                            egui::Label::new("?"),
                                        );
                                    }
                                }
                                // 2. Async decode complete: upload texture (fast) and render
                                else if let Some(decoded) = async_image::get_decoded(ui.ctx(), cache_key) {
                                    async_image::upload_and_render(ui, cache_key, &decoded, Some(thumb_height));
                                }
                                // 3. Decode in progress: show spinner
                                else if async_image::is_decoding(ui.ctx(), cache_key) {
                                    ui.add_sized(
                                        egui::vec2(thumb_height, thumb_height),
                                        egui::Spinner::new().size(16.0),
                                    );
                                }
                                // 4. Decode failed: show error
                                else if async_image::decode_failed(ui.ctx(), cache_key) {
                                    ui.add_sized(
                                        egui::vec2(thumb_height, thumb_height),
                                        egui::Label::new("?"),
                                    );
                                }
                                // 5. Not started: read bytes and start async decode
                                else if let Some(cache) = ctx.content_cache {
                                    if let Ok(Some(bytes)) = cache.get(cache_key) {
                                        // Start background decode (non-blocking)
                                        async_image::request_decode(ui.ctx(), cache_key, bytes);
                                        ui.add_sized(
                                            egui::vec2(thumb_height, thumb_height),
                                            egui::Spinner::new().size(16.0),
                                        );
                                    } else {
                                        // Not in disk cache
                                        ui.add_sized(
                                            egui::vec2(thumb_height, thumb_height),
                                            egui::Spinner::new().size(16.0),
                                        );
                                    }
                                } else {
                                    ui.add_sized(
                                        egui::vec2(thumb_height, thumb_height),
                                        egui::Label::new("?"),
                                    );
                                }
                            });

                            // Handle click on thumbnail
                            if thumb_response.response.interact(egui::Sense::click()).clicked() {
                                (ctx.event_callback)(&format!("{}_select_{}", id, i), None);
                            }
                        }
                    });
                });
        }

        // Image counter
        if images.len() > 1 {
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 30.0);
                ui.label(
                    egui::RichText::new(format!("{} / {}", current_idx + 1, images.len()))
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
            });
        }
    });
}
