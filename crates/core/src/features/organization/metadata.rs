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

/// Where one screenshot's image lives.
///
/// Providers report whichever form they have, and the three arrive in
/// three different wire shapes:
///
/// - `"https://host/a.jpg"` -- a bare string is the source URL. This is
///   what gameta's `extras.screenshots` holds, so it is what every real
///   product carries.
/// - `{"FilePath": "/tmp/a.jpg"}` -- a file the plugin downloaded first.
/// - `{"Base64": "..."}` -- the encoded image itself.
///
/// Read by hand rather than derived, because a derived `Deserialize`
/// accepts only the two tagged forms and reports a bare string as an
/// unknown variant -- which fails the whole enclosing document, not the
/// one field, so a screenshot list used to cost the title and the
/// creator with it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScreenshotData {
    Url(String),
    FilePath(PathBuf),
    Base64(String),
}

impl serde::Serialize for ScreenshotData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Url(url) => serializer.serialize_str(url),
            Self::FilePath(path) => {
                serializer.serialize_newtype_variant("ScreenshotData", 1, "FilePath", path)
            }
            Self::Base64(data) => {
                serializer.serialize_newtype_variant("ScreenshotData", 2, "Base64", data)
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for ScreenshotData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        enum Tagged {
            FilePath(PathBuf),
            Base64(String),
        }

        // Bare string first: a map cannot deserialize as `String`, so a
        // tagged entry falls through to `Tagged` and reports its own
        // unknown-variant error there.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Url(String),
            Tagged(Tagged),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Url(url) => Self::Url(url),
            Wire::Tagged(Tagged::FilePath(path)) => Self::FilePath(path),
            Wire::Tagged(Tagged::Base64(data)) => Self::Base64(data),
        })
    }
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

    /// Regression: signal JSON had null title/source/tags/screenshots which
    /// crashed serde deserialization before the `string_or_default` /
    /// `vec_or_default` custom deserializers were added.
    #[test]
    fn test_from_json_null_fields_dont_panic() {
        let json = r#"{
            "product_id": "RJ123",
            "source": null,
            "title": null,
            "tags": null,
            "screenshots": null
        }"#;
        let meta = GameMetadata::from_json(json).unwrap();
        assert_eq!(meta.product_id, "RJ123");
        assert_eq!(meta.source, "");
        assert_eq!(meta.title, "");
        assert!(meta.tags.is_empty());
        assert!(meta.screenshots.is_empty());
    }

    /// Verify that completely absent optional fields (not even present in
    /// the JSON) also deserialize cleanly via `#[serde(default)]`.
    #[test]
    fn test_from_json_missing_fields_use_defaults() {
        let json = r#"{ "product_id": "RJ456" }"#;
        let meta = GameMetadata::from_json(json).unwrap();
        assert_eq!(meta.product_id, "RJ456");
        assert_eq!(meta.source, "");
        assert_eq!(meta.title, "");
        assert!(meta.tags.is_empty());
        assert!(meta.screenshots.is_empty());
        assert!(meta.description.is_none());
        assert!(meta.release_date.is_none());
        assert!(meta.creator.is_none());
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
    /// The shape every provider actually emits. Screenshots live in
    /// gameta's `extras` as a list of source URLs, and
    /// `ProductMetadata::to_plugin_json` merges `extras` to the top
    /// level, so this is what reaches `from_json` for a real product.
    /// A screenshot entry that fails to deserialize fails the whole
    /// document, taking the title and creator with it.
    #[test]
    fn bare_screenshot_strings_parse_as_urls() {
        let json = r#"{
            "product_id": "RJ123456",
            "source": "dlsite",
            "title": "Placeholder Title",
            "screenshots": [
                "https://img.example.test/RJ123456_img_main.jpg",
                "https://img.example.test/RJ123456_img_smp1.jpg"
            ]
        }"#;

        let meta = GameMetadata::from_json(json).expect("a URL screenshot list must parse");

        assert_eq!(meta.title, "Placeholder Title");
        assert_eq!(
            meta.screenshots,
            vec![
                ScreenshotData::Url("https://img.example.test/RJ123456_img_main.jpg".to_string()),
                ScreenshotData::Url("https://img.example.test/RJ123456_img_smp1.jpg".to_string()),
            ]
        );
    }
}
