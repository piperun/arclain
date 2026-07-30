//! TRANSITIONAL: the pre-facade `arclain_core::GameMetadata` shape of a
//! tab's product metadata.
//!
//! The metadata a tab holds is
//! [`arclain_app::archive::ProductMetadataSummary`] -- the application's
//! own read model, carrying what display surfaces read and nothing else.
//! One consumer has not migrated yet: the Process page's pipeline
//! preview, which calls `arclain_core::preview_pipeline_with_metadata`
//! and therefore needs a value of the core type.
//!
//! This is the same shape [`super::AdoptedInventory`]'s `legacy_rows`
//! takes for the entry list: a derived, one-way projection built at the
//! call site, never a second store anything writes into. It goes away
//! with its one consumer -- the preview is due to resolve each input's
//! metadata the way the run does (from the library, per input) rather
//! than from whichever session happens to be open, at which point
//! nothing hands it a tab's metadata at all.
//!
//! Deliberately lossy, and safe to be: the projection is exact for every
//! field that consumer reads (`title` picks the output stem,
//! `product_id` keys its preview cache) and empty for the rest, because
//! carrying `source`/`description`/the raw document just to fill fields
//! no caller reads is precisely the per-frame clone cost the summary
//! exists to remove.

use arclain_app::archive::ProductMetadataSummary;
use arclain_core::features::organization::GameMetadata;

/// Projects `summary` onto the core metadata shape.
///
/// See the module doc: only `product_id` and `title` are meaningful, and
/// only because those are the two fields the one remaining consumer
/// reads.
pub fn legacy_pipeline_metadata(summary: &ProductMetadataSummary) -> GameMetadata {
    GameMetadata {
        product_id: summary.product_id.clone(),
        source: String::new(),
        title: summary.title.clone().unwrap_or_default(),
        description: None,
        tags: Vec::new(),
        release_date: None,
        creator: None,
        screenshots: Vec::new(),
        metadata_json: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> ProductMetadataSummary {
        ProductMetadataSummary {
            product_id: "RJ123456".to_string(),
            title: Some("Placeholder Title".to_string()),
            creator: Some("Placeholder Circle".to_string()),
            release_date: Some("2024-01-01".to_string()),
            tags: vec!["Tag A".to_string()],
            screenshots: Vec::new(),
        }
    }

    /// The two fields the pipeline preview reads survive the projection
    /// verbatim -- the output stem it predicts and the key it caches
    /// that prediction under both come from these.
    #[test]
    fn carries_the_identity_and_title_the_pipeline_preview_reads() {
        let legacy = legacy_pipeline_metadata(&summary());
        assert_eq!(legacy.product_id, "RJ123456");
        assert_eq!(legacy.title, "Placeholder Title");
    }

    /// A title the plugin did not report becomes the empty string the
    /// core stem derivation already treats as "fall through to the next
    /// candidate" -- so an untitled product still names its output from
    /// the detected code or the input stem, exactly as before.
    #[test]
    fn an_absent_title_projects_to_the_blank_core_falls_through_on() {
        let legacy = legacy_pipeline_metadata(&ProductMetadataSummary {
            title: None,
            ..summary()
        });
        assert!(legacy.title.is_empty());
    }
}
