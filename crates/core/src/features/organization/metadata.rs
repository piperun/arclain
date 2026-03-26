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
    #[serde(default, deserialize_with = "string_or_default")]
    pub source: String,

    /// Title - extracted for convenience and folder naming
    #[serde(default, deserialize_with = "string_or_default")]
    pub title: String,

    /// Description - extracted for convenience
    pub description: Option<String>,

    /// Tags - extracted for convenience
    #[serde(default, deserialize_with = "vec_or_default")]
    pub tags: Vec<String>,

    /// Release date - extracted for convenience
    pub release_date: Option<String>,

    /// Creator/Circle/Publisher - extracted for convenience
    pub creator: Option<String>,

    /// Screenshots to embed
    #[serde(default, deserialize_with = "vec_or_default")]
    pub screenshots: Vec<ScreenshotData>,

    /// Full layered JSON with both common and platform-specific data
    /// This is what gets saved as metadata.json in the archive
    #[serde(skip)]
    pub metadata_json: String,
}

/// Deserialize a String that might be null in the JSON (falls back to empty string)
fn string_or_default<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let opt = <Option<String> as serde::Deserialize>::deserialize(d)?;
    Ok(opt.unwrap_or_default())
}

/// Deserialize a Vec that might be null in the JSON (falls back to empty vec)
fn vec_or_default<'de, D: serde::Deserializer<'de>, T: serde::Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    let opt = <Option<Vec<T>> as serde::Deserialize>::deserialize(d)?;
    Ok(opt.unwrap_or_default())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_json_basic() {
        let json = r#"{
            "product_id": "RJ123456",
            "source": "dlsite",
            "title": "Test Game",
            "creator": "Test Circle",
            "tags": ["RPG", "Fantasy"],
            "screenshots": []
        }"#;
        let meta = GameMetadata::from_json(json).unwrap();
        assert_eq!(meta.product_id, "RJ123456");
        assert_eq!(meta.source, "dlsite");
        assert_eq!(meta.title, "Test Game");
        assert_eq!(meta.creator, Some("Test Circle".to_string()));
        assert_eq!(meta.tags, vec!["RPG", "Fantasy"]);
        assert_eq!(meta.metadata_json, json);
    }

    #[test]
    fn test_from_json_creator_from_circle_fallback() {
        let json = r#"{
            "product_id": "RJ999",
            "source": "dlsite",
            "title": "Game",
            "circle": "サークル名",
            "tags": [],
            "screenshots": []
        }"#;
        let meta = GameMetadata::from_json(json).unwrap();
        // creator is not in the serde struct fields directly,
        // so it should be None initially, then filled from "circle"
        assert_eq!(meta.creator, Some("サークル名".to_string()));
    }

    #[test]
    fn test_from_json_optional_fields() {
        let json = r#"{
            "product_id": "12345",
            "source": "steam",
            "title": "Minimal",
            "tags": [],
            "screenshots": []
        }"#;
        let meta = GameMetadata::from_json(json).unwrap();
        assert!(meta.description.is_none());
        assert!(meta.release_date.is_none());
        assert!(meta.creator.is_none());
    }

    #[test]
    fn test_from_json_invalid_json_returns_error() {
        assert!(GameMetadata::from_json("not json").is_err());
    }

    #[test]
    fn test_from_json_with_screenshots() {
        let json = r#"{
            "product_id": "RJ100",
            "source": "dlsite",
            "title": "With Screenshots",
            "tags": [],
            "screenshots": [
                {"FilePath": "/tmp/img1.jpg"},
                {"Base64": "aGVsbG8="}
            ]
        }"#;
        let meta = GameMetadata::from_json(json).unwrap();
        assert_eq!(meta.screenshots.len(), 2);
        assert_eq!(
            meta.screenshots[0],
            ScreenshotData::FilePath(PathBuf::from("/tmp/img1.jpg"))
        );
        assert_eq!(
            meta.screenshots[1],
            ScreenshotData::Base64("aGVsbG8=".to_string())
        );
    }
}
