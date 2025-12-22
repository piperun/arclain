//! Product metadata types

use serde::{Deserialize, Serialize};

/// Source of metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataSource {
    DLSite,
    Itchio,
    Steam,
    GOG,
    Custom,
}

impl MetadataSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DLSite => "dlsite",
            Self::Itchio => "itchio",
            Self::Steam => "steam",
            Self::GOG => "gog",
            Self::Custom => "custom",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "dlsite" => Some(Self::DLSite),
            "itchio" | "itch" => Some(Self::Itchio),
            "steam" => Some(Self::Steam),
            "gog" => Some(Self::GOG),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Unified product metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductMetadata {
    /// Unique ID: "{source}:{external_id}" e.g. "dlsite:RJ123456"
    pub id: String,
    /// Source platform
    pub source: MetadataSource,
    /// Platform-specific ID (e.g., "RJ123456")
    pub external_id: String,

    // Basic info
    pub title: Option<String>,
    pub creator: Option<String>,
    pub description: Option<String>,
    pub release_date: Option<String>,

    // Pricing
    pub price: Option<i64>,
    pub currency: Option<String>,

    // Ratings/Stats
    pub rating: Option<f64>,
    pub rating_count: Option<i64>,
    pub purchase_count: Option<i64>,
    pub favorite_count: Option<i64>,
    pub review_count: Option<i64>,

    // File info
    pub file_size: Option<String>,
    pub file_format: Option<String>,
    pub age_rating: Option<String>,

    // Categorization (JSON arrays stored as strings)
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub languages: Vec<String>,

    // Platform-specific extras
    pub extras: serde_json::Value,

    // Raw responses for re-parsing
    pub raw_api_response: Option<String>,
    pub raw_html: Option<String>,

    // Timestamps
    pub cached_at: i64,
    pub updated_at: Option<i64>,
}

impl ProductMetadata {
    pub fn new(source: MetadataSource, external_id: &str) -> Self {
        Self {
            id: format!("{}:{}", source.as_str(), external_id),
            source,
            external_id: external_id.to_string(),
            title: None,
            creator: None,
            description: None,
            release_date: None,
            price: None,
            currency: None,
            rating: None,
            rating_count: None,
            purchase_count: None,
            favorite_count: None,
            review_count: None,
            file_size: None,
            file_format: None,
            age_rating: None,
            genres: Vec::new(),
            tags: Vec::new(),
            languages: Vec::new(),
            extras: serde_json::Value::Null,
            raw_api_response: None,
            raw_html: None,
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            updated_at: None,
        }
    }
}

/// Search result from a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub external_id: String,
    pub title: String,
    pub creator: Option<String>,
    pub thumbnail_url: Option<String>,
}
