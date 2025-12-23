//! DLsite-specific cache key generation
//!
//! Provides standardized key generation for caching DLsite assets.
//! Other providers (Steam, itch.io) should implement their own key schemes.

/// Generate cache key for a screenshot (highest quality sample images)
pub fn screenshot_key(product_id: &str, index: usize) -> String {
    format!("dlsite:{}:screenshot_{}", product_id, index)
}

/// Generate cache key for the main cover image
pub fn cover_key(product_id: &str) -> String {
    format!("dlsite:{}:cover", product_id)
}

/// Generate cache key for thumbnail/preview image
pub fn thumbnail_key(product_id: &str) -> String {
    format!("dlsite:{}:thumbnail", product_id)
}

/// Generate cache key for the HTML page
pub fn html_key(product_id: &str) -> String {
    format!("dlsite:html:{}", product_id)
}

/// Generate cache key for the JSON API response
pub fn json_key(product_id: &str) -> String {
    format!("dlsite:json:{}", product_id)
}

/// Get all cache keys for a product
pub fn all_keys(product_id: &str, screenshot_count: usize) -> Vec<String> {
    let mut keys = vec![
        cover_key(product_id),
        thumbnail_key(product_id),
        html_key(product_id),
        json_key(product_id),
    ];

    for i in 0..screenshot_count {
        keys.push(screenshot_key(product_id, i));
    }

    keys
}
