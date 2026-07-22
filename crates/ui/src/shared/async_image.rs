//! Async image decoding for lazy loading
//!
//! Decodes images on background threads to avoid blocking UI.
//! Usage:
//! 1. Call `request_decode()` with image bytes
//! 2. Call `get_decoded()` each frame to check if ready
//! 3. When ready, upload texture (fast) and render

use eframe::egui;
use std::sync::Arc;

/// Decoded image data ready for texture upload
#[derive(Clone)]
pub struct DecodedImage {
    pub size: [usize; 2],
    pub pixels: Arc<Vec<u8>>, // RGBA pixels
}

/// State of an async decode request
#[derive(Clone)]
enum DecodeState {
    Pending,
    Complete(DecodedImage),
    Failed,
}

/// Request async decode of image bytes.
/// Returns true if decode was started, false if already pending/complete.
pub fn request_decode(ctx: &egui::Context, cache_key: &str, bytes: Vec<u8>) -> bool {
    let state_id = egui::Id::new(("async_decode", cache_key));

    // Check if already requested
    let existing: Option<DecodeState> = ctx.data(|d| d.get_temp(state_id));
    if existing.is_some() {
        return false;
    }

    // Mark as pending
    ctx.data_mut(|d| d.insert_temp(state_id, DecodeState::Pending));

    // Spawn background decode
    let ctx_clone = ctx.clone();
    let key = cache_key.to_string();

    std::thread::spawn(move || {
        let result = decode_image(&bytes);

        let state_id = egui::Id::new(("async_decode", key.as_str()));
        ctx_clone.data_mut(|d| {
            if let Some(decoded) = result {
                d.insert_temp(state_id, DecodeState::Complete(decoded));
            } else {
                d.insert_temp(state_id, DecodeState::Failed);
            }
        });

        // Trigger repaint so UI can pick up the result
        ctx_clone.request_repaint();
    });

    true
}

/// Check if decode is complete and get the result.
/// Returns Some(DecodedImage) if ready, None if pending or failed.
pub fn get_decoded(ctx: &egui::Context, cache_key: &str) -> Option<DecodedImage> {
    let state_id = egui::Id::new(("async_decode", cache_key));
    let state: Option<DecodeState> = ctx.data(|d| d.get_temp(state_id));

    match state {
        Some(DecodeState::Complete(img)) => Some(img),
        _ => None,
    }
}

/// Check if decode is in progress
pub fn is_decoding(ctx: &egui::Context, cache_key: &str) -> bool {
    let state_id = egui::Id::new(("async_decode", cache_key));
    let state: Option<DecodeState> = ctx.data(|d| d.get_temp(state_id));

    matches!(state, Some(DecodeState::Pending))
}

/// Check if decode failed
pub fn decode_failed(ctx: &egui::Context, cache_key: &str) -> bool {
    let state_id = egui::Id::new(("async_decode", cache_key));
    let state: Option<DecodeState> = ctx.data(|d| d.get_temp(state_id));

    matches!(state, Some(DecodeState::Failed))
}

/// Synchronously decode image bytes to RGBA pixels
fn decode_image(bytes: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();

    Some(DecodedImage {
        size,
        pixels: Arc::new(pixels),
    })
}

/// Upload decoded image to egui texture.
/// Returns the texture handle.
pub fn upload_texture(
    ctx: &egui::Context,
    cache_key: &str,
    decoded: &DecodedImage,
) -> egui::TextureHandle {
    let texture_id = egui::Id::new(("carousel_image", cache_key));

    // Check if texture already uploaded
    if let Some(handle) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(texture_id)) {
        return handle;
    }

    // Upload to GPU (fast - just a memcpy)
    let color_image = egui::ColorImage::from_rgba_unmultiplied(decoded.size, &decoded.pixels);
    let handle = ctx.load_texture(cache_key, color_image, egui::TextureOptions::default());

    // Cache the handle
    ctx.data_mut(|d| d.insert_temp(texture_id, handle.clone()));
    handle
}

/// Check if a texture is already cached
pub fn is_texture_cached(ctx: &egui::Context, cache_key: &str) -> bool {
    let texture_id = egui::Id::new(("carousel_image", cache_key));
    ctx.data(|d| d.get_temp::<egui::TextureHandle>(texture_id))
        .is_some()
}

/// Get cached texture handle
pub fn get_texture_handle(ctx: &egui::Context, cache_key: &str) -> Option<egui::TextureHandle> {
    let texture_id = egui::Id::new(("carousel_image", cache_key));
    ctx.data(|d| d.get_temp(texture_id))
}
