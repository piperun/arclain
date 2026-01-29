//! Content types for binary assets

use serde::{Deserialize, Serialize};

/// Type of content/asset
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cover" => Some(Self::Cover),
            "screenshot" => Some(Self::Screenshot),
            "thumbnail" => Some(Self::Thumbnail),
            "banner" => Some(Self::Banner),
            "video" => Some(Self::Video),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
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
    /// Width if image
    pub width: Option<u32>,
    /// Height if image
    pub height: Option<u32>,
}

impl ContentReference {
    /// Create a new content reference
    pub fn new(product_id: &str, content_type: ContentType, index: u32, cache_key: &str) -> Self {
        Self {
            product_id: product_id.to_string(),
            content_type,
            index,
            cache_key: cache_key.to_string(),
            source_url: None,
            width: None,
            height: None,
        }
    }

    /// Set source URL
    pub fn with_url(mut self, url: &str) -> Self {
        self.source_url = Some(url.to_string());
        self
    }

    /// Set dimensions
    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }
}
