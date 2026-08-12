//! Pure data types for the cache feature.
//!
//! Both the diesel-backed [`super::cache_index`] module and the
//! rusqlite mirror in [`super::cache_index_rusqlite`] consume these
//! types. Extracting them out of `cache_index.rs` (audit module-org
//! callout) lets callers depend on the taxonomy without pulling in
//! the diesel/rusqlite plumbing that surrounds it.

/// Type of cached content
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    /// Image screenshots/samples
    Screenshot,
    /// Thumbnail images
    Thumbnail,
    /// Structured metadata (JSON)
    Metadata,
    /// Raw HTML pages
    Html,
    /// Cover/main images
    Cover,
    /// Persistent data written through the bounded Wirt host API
    PluginData,
    /// Other/unknown content
    Other,
}

impl CacheType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheType::Screenshot => "screenshot",
            CacheType::Thumbnail => "thumbnail",
            CacheType::Metadata => "metadata",
            CacheType::Html => "html",
            CacheType::Cover => "cover",
            CacheType::PluginData => "plugin_data",
            CacheType::Other => "other",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "screenshot" => CacheType::Screenshot,
            "thumbnail" => CacheType::Thumbnail,
            "metadata" => CacheType::Metadata,
            "html" => CacheType::Html,
            "cover" => CacheType::Cover,
            "plugin_data" => CacheType::PluginData,
            _ => CacheType::Other,
        }
    }

    /// Infer cache type from a cache key
    pub fn from_key(key: &str) -> Self {
        if key.contains(":html:") || key.ends_with(":html") {
            CacheType::Html
        } else if key.contains(":json:") || key.ends_with(":json") {
            CacheType::Metadata
        } else if key.contains(":cover") {
            CacheType::Cover
        } else if key.contains(":screenshot") || key.contains(":sample") {
            CacheType::Screenshot
        } else if key.contains(":thumbnail") || key.contains(":thumb") {
            CacheType::Thumbnail
        } else {
            CacheType::Other
        }
    }

    /// Extract product_id from a cache key if possible.
    ///
    /// Expected formats: `"provider:product_id:asset_type"` or
    /// `"provider:type:product_id"`.
    pub fn extract_product_id(key: &str) -> Option<String> {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() >= 2 {
            // Handle "dlsite:RJ123456:cover" format
            let candidate = parts[1];
            // Check if it looks like a product ID (starts with RJ/VJ/BJ or is alphanumeric)
            if candidate.starts_with("RJ")
                || candidate.starts_with("VJ")
                || candidate.starts_with("BJ")
                || (candidate.chars().all(|c| c.is_alphanumeric()) && candidate.len() > 4)
            {
                // Skip if it's a type indicator
                if candidate != "html" && candidate != "json" && candidate != "search" {
                    return Some(format!("{}:{}", parts[0], candidate));
                }
            }
            // Handle "dlsite:html:RJ123456" format
            if parts.len() >= 3 && (parts[1] == "html" || parts[1] == "json") {
                return Some(format!("{}:{}", parts[0], parts[2]));
            }
        }
        None
    }
}

/// A cached content entry (high-level form, cache-type as an enum
/// rather than the diesel-row's `String`).
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub id: i64,
    pub key: String,
    pub product_id: Option<String>,
    pub content_hash: String,
    pub source_url: Option<String>,
    pub cache_type: CacheType,
    pub created_at: String,
    pub last_accessed: Option<String>,
    pub size_bytes: Option<i64>,
}
