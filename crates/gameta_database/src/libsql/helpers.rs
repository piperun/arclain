//! Helper functions for libSQL backend

use gameta_core::{MetadataSource, ProductMetadata, StorageError};

/// Generate a simple timestamp (seconds since UNIX epoch)
///
/// For production use, consider using the `chrono` crate for proper
/// ISO 8601 formatted timestamps with timezone support.
pub fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

/// Convert a libsql row to ProductMetadata
///
/// Expects columns in this order:
/// 0: id, 1: source, 2: external_id, 3: title, 4: creator, 5: description,
/// 6: release_date, 7: price, 8: currency, 9: rating, 10: rating_count,
/// 11: purchase_count, 12: favorite_count, 13: review_count, 14: file_size,
/// 15: file_format, 16: age_rating, 17: genres, 18: tags, 19: languages,
/// 20: extras, 21: raw_api_response, 22: raw_html, 23: geo_blocked,
/// 24: cached_at, 25: updated_at
pub fn row_to_metadata(row: &libsql::Row) -> Result<ProductMetadata, StorageError> {
    Ok(ProductMetadata {
        id: row.get(0).unwrap_or_default(),
        source: MetadataSource::from_str(&row.get::<String>(1).unwrap_or_default())
            .unwrap_or(MetadataSource::Custom),
        external_id: row.get(2).unwrap_or_default(),
        title: row.get(3).ok(),
        creator: row.get(4).ok(),
        description: row.get(5).ok(),
        release_date: row.get(6).ok(),
        price: row.get(7).ok(),
        currency: row.get(8).ok(),
        rating: row.get(9).ok(),
        rating_count: row.get(10).ok(),
        purchase_count: row.get(11).ok(),
        favorite_count: row.get(12).ok(),
        review_count: row.get(13).ok(),
        file_size: row.get(14).ok(),
        file_format: row.get(15).ok(),
        age_rating: row.get(16).ok(),
        genres: row
            .get::<String>(17)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        tags: row
            .get::<String>(18)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        languages: row
            .get::<String>(19)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        extras: row
            .get::<String>(20)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null),
        raw_api_response: row.get(21).ok(),
        raw_html: row.get(22).ok(),
        geo_blocked: row.get::<i32>(23).unwrap_or(0) != 0,
        cached_at: row.get(24).unwrap_or(0),
        updated_at: row.get(25).ok(),
    })
}
