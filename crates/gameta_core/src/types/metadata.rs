//! Product metadata types

use serde::{Deserialize, Serialize};

/// Source of metadata
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl std::fmt::Display for MetadataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Unified product metadata structure
///
/// This structure holds all metadata for a product, regardless of source platform.
/// Platform-specific data is stored in the `extras` field as JSON.
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

    // Categorization
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub languages: Vec<String>,

    // Platform-specific extras (voice_actors, authors, screenshots, etc.)
    pub extras: serde_json::Value,

    // Raw responses for re-parsing (storage layer only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_api_response: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_html: Option<String>,

    // Availability
    /// Whether this product is geo-blocked in user's region
    #[serde(default)]
    pub geo_blocked: bool,

    // Timestamps (storage layer)
    #[serde(default)]
    pub cached_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

impl ProductMetadata {
    /// Create new metadata with minimal required fields
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
            geo_blocked: false,
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            updated_at: None,
        }
    }

    /// Convert metadata to the JSON format expected by plugins.
    ///
    /// This produces a normalized structure that includes:
    /// - Core fields (title, creator, description, etc.)
    /// - Source-specific extras merged at top level
    /// - Source-specific nested object (e.g., "dlsite": { "id": "...", "url": "..." })
    /// - Common aliases for cross-source compatibility
    pub fn to_plugin_json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "product_id": &self.external_id,
            "source": self.source.as_str(),
            "title": &self.title,
            "creator": &self.creator,
            "circle": &self.creator, // Alias for DLSite compatibility
            "description": &self.description,
            "release_date": &self.release_date,
            "tags": &self.tags,
            "genres": &self.genres,
            "geo_blocked": self.geo_blocked,
            "file_size": &self.file_size,
            "common": {
                "dlsite_id": &self.external_id // Legacy compatibility
            }
        });

        // Merge extras into top level (voice_actors, screenshots, etc.)
        if let Some(obj) = self.extras.as_object() {
            for (key, value) in obj {
                json[key] = value.clone();
            }
        }

        // Add source-specific nested object
        match self.source {
            MetadataSource::DLSite => {
                json["dlsite"] = serde_json::json!({
                    "id": &self.external_id,
                    "code": &self.external_id,
                    "price": self.price.map(|p| p.to_string()).unwrap_or_default(),
                    "url": format!("https://www.dlsite.com/pro/work/=/product_id/{}.html", &self.external_id)
                });
            }
            MetadataSource::Steam => {
                json["steam"] = serde_json::json!({
                    "id": &self.external_id,
                    "url": format!("https://store.steampowered.com/app/{}", &self.external_id)
                });
            }
            MetadataSource::Itchio => {
                json["itchio"] = serde_json::json!({
                    "id": &self.external_id
                });
            }
            MetadataSource::GOG => {
                json["gog"] = serde_json::json!({
                    "id": &self.external_id
                });
            }
            MetadataSource::Custom => {
                json["custom"] = serde_json::json!({
                    "id": &self.external_id
                });
            }
        }

        json
    }

    /// Convert to plugin JSON and serialize to string
    pub fn to_plugin_json_string(&self) -> String {
        self.to_plugin_json().to_string()
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
