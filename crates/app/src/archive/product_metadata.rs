//! One archive session's product metadata, as a frontend consumes it.
//!
//! Metadata reaches an archive session as a raw JSON document: a plugin
//! calls the `emit_metadata` host function, and the value it wrote is
//! what [`crate::archive::ArchiveSnapshot::metadata`] carries. Before
//! this module every frontend parsed that document itself, into
//! `arclain_core`'s own metadata type, and then read fields off it --
//! which meant each frontend owned a copy of the parse rule and a
//! dependency on a headless crate's struct layout, and could disagree
//! with the planner about what the very same document said.
//!
//! [`ProductMetadataSummary`] replaces that. It is deliberately *not* a
//! field-for-field mirror of the core type: mirroring would relocate the
//! coupling rather than remove it, and the fields a frontend never reads
//! are exactly the expensive ones (the whole raw document as a string,
//! and inline base64 image payloads) which a per-frame reader would then
//! clone for nothing. What is here is the union of what the display
//! surfaces actually read -- an identity, a title, an attribution, a
//! date, tags, and enough about each screenshot to *name* it.
//!
//! The parse itself is [`crate::organization::session_metadata_for_planning`],
//! the same function the organize preview and the session-bound organize
//! run plan from. That is the point of routing through it rather than
//! re-parsing here: what a panel displays and what the plan was built
//! from can never describe two different documents.

use arclain_core::features::organization::{GameMetadata, ScreenshotData};

/// One screenshot a session's product metadata offers, identified but
/// not carried.
///
/// A frontend enumerates these to report *which* screenshots a plan did
/// not schedule; it never needs the image itself to do that, so the
/// bytes never cross this boundary. A plugin that supplied a screenshot
/// inline is reported by its encoded length alone -- enough to write the
/// same line an issues report always wrote for it, and small enough to
/// live in a value a frontend clones on every read.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScreenshotRef {
    /// The source URL the plugin reported, the form nearly every
    /// provider supplies.
    Url { url: String },
    /// A file the plugin downloaded before reporting it.
    File { path: String },
    /// Base64 image data the plugin supplied inline, reported by the
    /// length of the encoded string.
    Inline { encoded_len: u64 },
}

impl ScreenshotRef {
    /// The identifier to print for this screenshot in a human-readable
    /// report.
    ///
    /// Lives here rather than in each frontend so every report names the
    /// same screenshot the same way -- and so a frontend never needs the
    /// inline payload just to describe it.
    pub fn identifier(&self) -> String {
        match self {
            Self::Url { url } => url.clone(),
            Self::File { path } => path.clone(),
            Self::Inline { encoded_len } => format!("Base64 data ({} bytes)", encoded_len),
        }
    }
}

/// What one archive session's plugin-reported product metadata says, in
/// the vocabulary the surfaces that display it actually use.
///
/// Every string field is normalized on the way in: a value the plugin
/// reported as absent, null, empty, or whitespace-only arrives here as
/// `None` (or, for [`Self::product_id`], as an empty string -- the one
/// field the document must carry for the parse to succeed at all). A
/// consumer therefore branches on `Option`, not on `is_empty()`, and
/// cannot accidentally render a present-but-blank title as a real one.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ProductMetadataSummary {
    /// The platform's own identifier for the product (`"RJ123456"`, an
    /// itch slug, a Steam app id). Always present: the document cannot
    /// parse without it.
    pub product_id: String,
    /// The product's title, or `None` when the plugin reported none.
    pub title: Option<String>,
    /// Creator / circle / publisher, or `None` when unattributed.
    pub creator: Option<String>,
    /// Release date exactly as the plugin reported it -- a display
    /// string, deliberately not parsed into a date type: the sources
    /// disagree about format and precision, and nothing here does date
    /// arithmetic.
    pub release_date: Option<String>,
    /// Tags in the order the plugin reported them.
    pub tags: Vec<String>,
    /// The screenshots this metadata offers, in first-reported order and
    /// deduplicated on the plugin's own value -- so a report enumerating
    /// them lists each distinct screenshot once, in a stable order,
    /// without the caller needing the payloads to tell two apart.
    pub screenshots: Vec<ScreenshotRef>,
}

/// Parses a session's raw metadata document into the summary a frontend
/// reads.
///
/// Pure and synchronous -- no app handle, no runtime, no I/O -- so the
/// egui composition can call it at the frame boundary where it already
/// drains the document, and so a bridge can call it on whatever thread
/// it likes. `None` in yields `None` out, and a document that fails to
/// parse also yields `None` rather than an error: that is what a
/// metadata-less plan already means everywhere else in this crate, and a
/// display surface has nothing useful to do with the distinction.
///
/// Frontends that hold a session id rather than a document should call
/// [`crate::ArclainApp::product_metadata`] instead; it reads the
/// session's current document and comes through here.
pub fn product_metadata_from_document(
    document: Option<serde_json::Value>,
) -> Option<ProductMetadataSummary> {
    crate::organization::session_metadata_for_planning(document).map(summarize_product_metadata)
}

/// Projects the parsed core metadata onto the summary.
fn summarize_product_metadata(metadata: GameMetadata) -> ProductMetadataSummary {
    ProductMetadataSummary {
        product_id: metadata.product_id,
        title: non_blank(Some(metadata.title)),
        creator: non_blank(metadata.creator),
        release_date: non_blank(metadata.release_date),
        tags: metadata.tags,
        screenshots: summarize_screenshots(&metadata.screenshots),
    }
}

/// `None` for a value that is absent, empty, or whitespace-only; the
/// original string otherwise (untrimmed -- the blank test decides
/// presence, it does not rewrite a value the plugin reported).
fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// Deduplicates on the core value -- which still carries the inline
/// payload, so two distinct screenshots are never merged -- and only
/// then reduces each survivor to its reference form.
fn summarize_screenshots(screenshots: &[ScreenshotData]) -> Vec<ScreenshotRef> {
    let mut seen = std::collections::HashSet::new();
    screenshots
        .iter()
        .filter(|data| seen.insert(*data))
        .map(|data| match data {
            ScreenshotData::Url(url) => ScreenshotRef::Url { url: url.clone() },
            ScreenshotData::FilePath(path) => ScreenshotRef::File {
                path: path.display().to_string(),
            },
            ScreenshotData::Base64(encoded) => ScreenshotRef::Inline {
                encoded_len: encoded.len() as u64,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin document in the layered shape `emit_metadata` writes.
    /// Placeholder product codes only.
    fn document(body: &str) -> Option<serde_json::Value> {
        Some(serde_json::from_str(body).expect("fixture parses"))
    }

    #[test]
    fn summarizes_a_full_document() {
        let summary = product_metadata_from_document(document(
            r#"{
                "product_id": "RJ123456",
                "source": "dlsite",
                "title": "Placeholder Title",
                "description": "A description no display surface reads.",
                "creator": "Placeholder Circle",
                "release_date": "2024-01-01",
                "tags": ["Tag A", "Tag B"],
                "screenshots": [
                    {"FilePath": "/tmp/shot1.jpg"},
                    {"Base64": "aGVsbG8="}
                ]
            }"#,
        ))
        .expect("summary");

        assert_eq!(summary.product_id, "RJ123456");
        assert_eq!(summary.title.as_deref(), Some("Placeholder Title"));
        assert_eq!(summary.creator.as_deref(), Some("Placeholder Circle"));
        assert_eq!(summary.release_date.as_deref(), Some("2024-01-01"));
        assert_eq!(summary.tags, vec!["Tag A", "Tag B"]);
        assert_eq!(
            summary.screenshots,
            vec![
                ScreenshotRef::File {
                    path: std::path::PathBuf::from("/tmp/shot1.jpg")
                        .display()
                        .to_string(),
                },
                ScreenshotRef::Inline { encoded_len: 8 },
            ]
        );
    }

    /// The document's `circle` field is the creator when no explicit
    /// `creator` is present -- the parse rule the planner uses, reached
    /// through the same function rather than reimplemented here.
    #[test]
    fn creator_falls_back_to_the_circle_field() {
        let summary = product_metadata_from_document(document(
            r#"{"product_id": "RJ123456", "circle": "Placeholder Circle"}"#,
        ))
        .expect("summary");

        assert_eq!(summary.creator.as_deref(), Some("Placeholder Circle"));
    }

    /// Blank is absent: a title/creator/date the plugin reported as an
    /// empty or whitespace-only string is `None`, so a display surface
    /// renders its no-value branch instead of an empty badge.
    #[test]
    fn blank_strings_are_reported_as_absent() {
        let summary = product_metadata_from_document(document(
            r#"{
                "product_id": "RJ123456",
                "title": "   ",
                "creator": "",
                "release_date": " "
            }"#,
        ))
        .expect("summary");

        assert_eq!(summary.title, None);
        assert_eq!(summary.creator, None);
        assert_eq!(summary.release_date, None);
    }

    /// Null title/tags/screenshots are the shape a plugin actually
    /// emits; they must not fail the parse.
    #[test]
    fn null_fields_summarize_to_empty_rather_than_failing() {
        let summary = product_metadata_from_document(document(
            r#"{
                "product_id": "RJ123456",
                "title": null,
                "tags": null,
                "screenshots": null
            }"#,
        ))
        .expect("summary");

        assert_eq!(summary.product_id, "RJ123456");
        assert_eq!(summary.title, None);
        assert!(summary.tags.is_empty());
        assert!(summary.screenshots.is_empty());
    }

    #[test]
    fn an_absent_document_summarizes_to_nothing() {
        assert_eq!(product_metadata_from_document(None), None);
    }

    /// An unparseable document is a metadata-less session, not an error
    /// -- the same treatment the planner gives it.
    #[test]
    fn an_unparseable_document_summarizes_to_nothing() {
        assert_eq!(
            product_metadata_from_document(Some(serde_json::json!({"no_product_id": true}))),
            None
        );
    }

    /// Dedup happens on the plugin's own value, before the inline
    /// payload is dropped -- so two inline screenshots that merely
    /// share an encoded length both survive, and a genuinely repeated
    /// one does not.
    #[test]
    fn screenshots_dedupe_on_the_reported_value_in_first_reported_order() {
        let summary = product_metadata_from_document(document(
            r#"{
                "product_id": "RJ123456",
                "screenshots": [
                    {"FilePath": "/tmp/b.jpg"},
                    {"FilePath": "/tmp/a.jpg"},
                    {"FilePath": "/tmp/b.jpg"},
                    {"Base64": "aGVsbG8="},
                    {"Base64": "d29ybGQh"},
                    {"Base64": "aGVsbG8="}
                ]
            }"#,
        ))
        .expect("summary");

        let paths = |name: &str| ScreenshotRef::File {
            path: std::path::PathBuf::from(format!("/tmp/{name}"))
                .display()
                .to_string(),
        };
        assert_eq!(
            summary.screenshots,
            vec![
                paths("b.jpg"),
                paths("a.jpg"),
                ScreenshotRef::Inline { encoded_len: 8 },
                ScreenshotRef::Inline { encoded_len: 8 },
            ]
        );
    }

    #[test]
    fn screenshot_identifiers_name_a_file_by_path_and_inline_data_by_length() {
        assert_eq!(
            ScreenshotRef::File {
                path: "covers/front.png".to_string(),
            }
            .identifier(),
            "covers/front.png"
        );
        assert_eq!(
            ScreenshotRef::Inline { encoded_len: 8 }.identifier(),
            "Base64 data (8 bytes)"
        );
    }

    #[test]
    fn summary_round_trips_through_json_with_snake_case_names() {
        let summary = ProductMetadataSummary {
            product_id: "RJ123456".to_string(),
            title: Some("Placeholder Title".to_string()),
            creator: None,
            release_date: None,
            tags: vec!["Tag A".to_string()],
            screenshots: vec![
                ScreenshotRef::File {
                    path: "covers/front.png".to_string(),
                },
                ScreenshotRef::Inline { encoded_len: 8 },
            ],
        };

        let json = serde_json::to_value(&summary).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "product_id": "RJ123456",
                "title": "Placeholder Title",
                "creator": null,
                "release_date": null,
                "tags": ["Tag A"],
                "screenshots": [
                    {"kind": "file", "path": "covers/front.png"},
                    {"kind": "inline", "encoded_len": 8}
                ]
            })
        );
        let decoded: ProductMetadataSummary = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, summary);
    }
}
