//! Content types for binary assets

use serde::{Deserialize, Serialize};

/// Type of content/asset
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    Cover,
    Screenshot,
    Thumbnail,
    Banner,
    Video,
    Other,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Screenshot => "screenshot",
            Self::Thumbnail => "thumbnail",
            Self::Banner => "banner",
            Self::Video => "video",
            Self::Other => "other",
        }
    }
}

/// Reference to cached binary content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentReference {
    /// Product this content belongs to
    pub product_id: String,
    /// Type of content
    pub content_type: ContentType,
    /// Index within type (e.g., screenshot #0, #1)
    pub index: u32,
    /// Cache key/hash for retrieval
    pub cache_key: String,
    /// Source URL
    pub source_url: Option<String>,
    /// Dimensions if image
    pub width: Option<u32>,
    pub height: Option<u32>,
}
