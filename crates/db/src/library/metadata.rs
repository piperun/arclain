//! Product metadata types and utilities.
//!
//! The canonical types (`ProductMetadata`, `MetadataSource`) come from gameta_core.
//! This module provides arclain-specific extensions like `completeness_score()`.

pub use gameta_core::{MetadataSource, ProductMetadata};

/// Extension trait for arclain-specific completeness scoring.
///
/// Used by LibraryService to prevent overwriting higher-quality data
/// with lower-quality data during saves.
pub trait CompletenessScore {
    fn completeness_score(&self) -> u32;
}

impl CompletenessScore for ProductMetadata {
    /// Calculate a completeness score for data quality comparison.
    /// Higher score = more complete data.
    fn completeness_score(&self) -> u32 {
        let mut score: u32 = 0;

        // Core identity fields (high value)
        if self.title.is_some() {
            score += 10;
        }
        if self.creator.is_some() {
            score += 10;
        }

        // Description and content info (medium value)
        if self.description.is_some() {
            score += 5;
        }
        if self.release_date.is_some() {
            score += 2;
        }
        if self.price.is_some() {
            score += 1;
        }
        if self.file_size.is_some() {
            score += 1;
        }
        if self.file_format.is_some() {
            score += 1;
        }
        if self.age_rating.is_some() {
            score += 2;
        }

        // Ratings and stats
        if self.rating.is_some() {
            score += 2;
        }
        if self.rating_count.is_some() {
            score += 1;
        }
        if self.purchase_count.is_some() {
            score += 1;
        }
        if self.favorite_count.is_some() {
            score += 1;
        }
        if self.review_count.is_some() {
            score += 1;
        }

        // Vec fields: count actual elements
        score += self.genres.len() as u32 * 2;
        score += self.tags.len() as u32;
        score += self.languages.len() as u32;

        // Extras object fields (DLSite-specific data moved here)
        if let Some(obj) = self.extras.as_object() {
            if obj.contains_key("series_name") {
                score += 2;
            }
            if obj.contains_key("illustrator") {
                score += 2;
            }
            if obj.contains_key("miscellaneous") {
                score += 1;
            }
            if obj.contains_key("update_info") {
                score += 1;
            }
            if let Some(va) = obj.get("voice_actors").and_then(|v| v.as_array()) {
                score += va.len() as u32;
            }
            if let Some(pf) = obj.get("product_formats").and_then(|v| v.as_array()) {
                score += pf.len() as u32;
            }
            if let Some(r) = obj.get("rankings").and_then(|v| v.as_object()) {
                score += r.len() as u32;
            }
        }

        // Raw data storage (indicates full fetch)
        if self.raw_api_response.is_some() {
            score += 5;
        }
        if self.raw_html.is_some() {
            score += 3;
        }

        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completeness_score_basic_fields() {
        let mut m = ProductMetadata::new(MetadataSource::DLSite, "RJ100");
        let base_score = m.completeness_score();
        assert_eq!(base_score, 0);

        m.title = Some("Title".into());
        assert_eq!(m.completeness_score(), 10);

        m.creator = Some("Creator".into());
        assert_eq!(m.completeness_score(), 20);

        m.description = Some("Desc".into());
        assert_eq!(m.completeness_score(), 25);

        m.release_date = Some("2024-01-01".into());
        assert_eq!(m.completeness_score(), 27);

        m.price = Some(1000);
        m.file_size = Some("1GB".into());
        m.file_format = Some("ZIP".into());
        m.age_rating = Some("R-18".into());
        assert_eq!(m.completeness_score(), 32);
    }

    #[test]
    fn test_completeness_score_ratings() {
        let mut m = ProductMetadata::new(MetadataSource::DLSite, "RJ200");
        m.rating = Some(4.5);        // +2
        m.rating_count = Some(100);   // +1
        m.purchase_count = Some(5000);// +1
        m.favorite_count = Some(200); // +1
        m.review_count = Some(50);    // +1
        assert_eq!(m.completeness_score(), 6);
    }

    #[test]
    fn test_completeness_score_vec_fields() {
        let mut m = ProductMetadata::new(MetadataSource::DLSite, "RJ300");

        m.genres = vec!["RPG".into(), "Adventure".into()]; // 2*2 = 4
        m.tags = vec!["t1".into(), "t2".into(), "t3".into()]; // 3*1 = 3
        m.languages = vec!["JP".into()]; // 1*1 = 1
        assert_eq!(m.completeness_score(), 8);
    }

    #[test]
    fn test_completeness_score_extras() {
        let mut m = ProductMetadata::new(MetadataSource::DLSite, "RJ400");

        m.extras = serde_json::json!({
            "series_name": "Test Series",
            "illustrator": "Artist",
            "miscellaneous": "misc",
            "update_info": "v1.1",
            "voice_actors": ["VA1", "VA2", "VA3"],
            "product_formats": ["Download", "CD"],
            "rankings": {"daily": 1, "weekly": 5},
        });

        // series_name=2, illustrator=2, miscellaneous=1, update_info=1,
        // voice_actors=3, product_formats=2, rankings=2 keys
        assert_eq!(m.completeness_score(), 13);
    }

    #[test]
    fn test_completeness_score_raw_data() {
        let mut m = ProductMetadata::new(MetadataSource::DLSite, "RJ500");

        m.raw_api_response = Some("{}".into());
        assert_eq!(m.completeness_score(), 5);

        m.raw_html = Some("<html/>".into());
        assert_eq!(m.completeness_score(), 8);
    }

    #[test]
    fn test_completeness_score_fully_populated() {
        let m = ProductMetadata {
            id: "dlsite:RJ600".into(),
            source: MetadataSource::DLSite,
            external_id: "RJ600".into(),
            title: Some("T".into()),          // 10
            creator: Some("C".into()),         // 10
            description: Some("D".into()),     // 5
            release_date: Some("2024".into()), // 2
            price: Some(1000),                 // 1
            currency: None,
            rating: Some(4.0),                 // 2
            rating_count: Some(10),            // 1
            purchase_count: Some(100),         // 1
            favorite_count: Some(50),          // 1
            review_count: Some(5),             // 1
            file_size: Some("1GB".into()),     // 1
            file_format: Some("ZIP".into()),   // 1
            age_rating: Some("R-18".into()),   // 2
            genres: vec!["G1".into()],         // 2
            tags: vec!["T1".into()],           // 1
            languages: vec!["JP".into()],      // 1
            extras: serde_json::json!({"series_name": "S"}), // 2
            raw_api_response: Some("{}".into()), // 5
            raw_html: Some("<h/>".into()),       // 3
            geo_blocked: false,
            cached_at: 0,
            updated_at: None,
        };

        // Total: 10+10+5+2+1+2+1+1+1+1+1+1+2+2+1+1+2+5+3 = 52
        assert_eq!(m.completeness_score(), 52);
    }
}
