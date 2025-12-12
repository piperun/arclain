use std::path::PathBuf;

/// Generic game/product metadata from any source (DLSite, itch.io, Steam, etc.)
///
/// The `metadata_json` field contains a layered JSON structure:
/// ```json
/// {
///   "source": "dlsite",
///   "product_id": "RJ123456",
///   "common": {
///     "title": "Game Title",
///     "description": "...",
///     "tags": ["tag1", "tag2"],
///     "creator": "Creator Name",
///     "release_date": "2024-01-01"
///   },
///   "dlsite": {
///     // All DLSite-specific fields preserved here
///     "circle": "サークル名",
///     "work_format": "ゲーム",
///     // ... etc
///   }
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GameMetadata {
    /// Product ID - platform-specific identifier (e.g., "RJ123456", "itch-slug")
    pub product_id: String,

    /// Source platform (e.g., "dlsite", "itch", "steam")
    pub source: String,

    /// Title - extracted for convenience and folder naming
    pub title: String,

    /// Description - extracted for convenience
    pub description: Option<String>,

    /// Tags - extracted for convenience
    pub tags: Vec<String>,

    /// Release date - extracted for convenience
    pub release_date: Option<String>,

    /// Creator/Circle/Publisher - extracted for convenience
    pub creator: Option<String>,

    /// Screenshots to embed
    pub screenshots: Vec<ScreenshotData>,

    /// Full layered JSON with both common and platform-specific data
    /// This is what gets saved as metadata.json in the archive
    #[serde(skip)]
    pub metadata_json: String,
}

impl GameMetadata {
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        let mut metadata: Self = serde_json::from_str(json)?;
        metadata.metadata_json = json.to_string();

        // DEBUG: Log what we parsed
        tracing::info!(
            "[GameMetadata] Parsed from JSON - title: {:?}, creator: {:?}",
            metadata.title,
            metadata.creator
        );

        // If creator is None, try to extract from 'circle' field in the raw JSON
        if metadata.creator.is_none() {
            if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(json) {
                if let Some(circle) = json_value.get("circle").and_then(|v| v.as_str()) {
                    tracing::info!(
                        "[GameMetadata] Found 'circle' field in JSON: {}, using as creator",
                        circle
                    );
                    metadata.creator = Some(circle.to_string());
                }
            }
        }

        Ok(metadata)
    }
}

/// Screenshot data provided by plugin
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ScreenshotData {
    FilePath(PathBuf), // Downloaded by plugin
    Base64(String),    // Base64-encoded
}
