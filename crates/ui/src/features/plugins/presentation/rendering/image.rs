//! Image rendering logic for plugins

use super::context::{RenderContext, UiEventHandler};
use crate::shared::image_assets::{ImageAssetState, ImageOwner};
use crate::shared::image_fetcher::trigger_image_fetch;
use crate::shared::SharedState;
use eframe::egui;

const IMAGE_FETCH_RETRY_INTERVAL_SECS: u64 = 30;

/// Everything the image helpers below need, independent of which plugin
/// renderer is driving them.
///
/// Extracted from `RenderContext` so both the flat `PluginUiElement`
/// renderer and the `PluginUiDocument` renderer
/// (`super::document`) share one image path instead of duplicating the
/// cache/fetch/decode state machine. `RenderContext` supplies one via
/// [`RenderContext::image_context`].
#[derive(Clone, Copy)]
pub struct ImageContext<'a> {
    pub shared_state: Option<&'a SharedState>,
    pub plugin_id: Option<&'a str>,
    pub image_owner: Option<&'a ImageOwner>,
}

/// Render an Image element
pub fn render_image(
    ui: &mut egui::Ui,
    ctx: &mut RenderContext<'_, impl UiEventHandler + ?Sized>,
    cache_key: &Option<String>,
    url: &Option<String>,
    max_height: Option<f32>,
) {
    let colors = ctx.colors;

    let images = ctx.image_context();
    if let Some(key) = cache_key {
        let (state, texture) = resolve_texture(ui, images, key);
        if let Some(texture) = texture {
            render_texture(ui, &texture, max_height);
            return;
        }
        if matches!(state, ImageAssetState::Failed(_)) {
            maybe_trigger_fetch(ui, images, key, url.as_deref());
        }
        let (message, color) = match state {
            ImageAssetState::Failed(message) if url.is_none() => {
                (format!("🖼 [Error: {message}]"), colors.error)
            }
            ImageAssetState::Failed(_) => {
                ("🖼 [Reloading...]".to_string(), colors.on_surface_variant)
            }
            _ => (format!("🖼 [Loading: {key}]"), colors.on_surface_variant),
        };
        ui.label(egui::RichText::new(message).color(color).italics());
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

pub(super) fn resolve_texture(
    ui: &egui::Ui,
    ctx: ImageContext<'_>,
    key: &str,
) -> (ImageAssetState, Option<egui::TextureHandle>) {
    let (Some(shared), Some(owner)) = (ctx.shared_state, ctx.image_owner) else {
        return (
            ImageAssetState::Failed("image asset store is unavailable".to_string()),
            None,
        );
    };
    let mut state = shared
        .image_assets
        .request(owner.clone(), key, ui.ctx().clone());
    let texture = match state {
        ImageAssetState::Decoded => {
            let texture = shared.image_assets.upload_ready(key, ui.ctx());
            if texture.is_some() {
                state = ImageAssetState::Uploaded;
            }
            texture
        }
        ImageAssetState::Uploaded => shared.image_assets.get_texture(owner, key),
        ImageAssetState::Loading | ImageAssetState::Failed(_) => None,
    };
    (state, texture)
}

pub(super) fn maybe_trigger_fetch(
    ui: &egui::Ui,
    ctx: ImageContext<'_>,
    key: &str,
    url: Option<&str>,
) {
    let (Some(url), Some(shared)) = (url, ctx.shared_state) else {
        return;
    };
    let fetch_id = egui::Id::new(("image_fetch", key));
    let now = std::time::Instant::now();
    let last_fired: Option<std::time::Instant> = ui.data(|data| data.get_temp(fetch_id));
    if last_fired
        .is_some_and(|last| now.duration_since(last).as_secs() <= IMAGE_FETCH_RETRY_INTERVAL_SECS)
    {
        return;
    }
    ui.data_mut(|data| data.insert_temp(fetch_id, now));
    trigger_image_fetch(
        shared,
        ctx.plugin_id.map(str::to_string),
        url.to_string(),
        key.to_string(),
        ui.ctx().clone(),
    );
}

pub(super) fn render_texture(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    max_height: Option<f32>,
) -> egui::Vec2 {
    let texture_size = texture.size_vec2();
    let max_height = max_height.unwrap_or(200.0);
    let scale = if texture_size.y > max_height {
        max_height / texture_size.y
    } else {
        1.0
    };
    let display_size = texture_size * scale;
    ui.image(egui::load::SizedTexture {
        id: texture.id(),
        size: display_size,
    });
    display_size
}
