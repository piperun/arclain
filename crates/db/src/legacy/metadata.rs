//! Legacy Rusqlite implementation for product metadata
//!
//! Moved from crates/db/src/library/metadata.rs

use crate::library::metadata::{MetadataSource, ProductMetadata, CREATE_TABLE_SQL};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Parse from a database row
fn from_row(row: &Row<'_>) -> rusqlite::Result<ProductMetadata> {
    Ok(ProductMetadata {
        id: row.get(0)?,
        source: row.get(1)?,
        external_id: row.get(2)?,
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
        genres_json: row.get(17).ok(),
        tags_json: row.get(18).ok(),
        languages_json: row.get(19).ok(),
        product_formats_json: row.get(20).ok(),
        series_name: row.get(21).ok(),
        illustrator: row.get(22).ok(),
        voice_actors_json: row.get(23).ok(),
        miscellaneous: row.get(24).ok(),
        update_info: row.get(25).ok(),
        rankings_json: row.get(26).ok(),
        extras_json: row.get(27).ok(),
        raw_api_response: row.get(28).ok(),
        raw_html: row.get(29).ok(),
        geo_blocked: row.get(30).ok(),
        cached_at: row.get(31)?,
        updated_at: row.get(32).ok(),
        last_accessed: row.get(33).ok(),
    })
}

const SELECT_COLS: &str = "id, source, external_id, title, creator, description, release_date, \
    price, currency, rating, rating_count, purchase_count, favorite_count, review_count, \
    file_size, file_format, age_rating, genres_json, tags_json, languages_json, product_formats_json, \
    series_name, illustrator, voice_actors_json, miscellaneous, update_info, rankings_json, \
    extras_json, raw_api_response, raw_html, geo_blocked, cached_at, updated_at, last_accessed";

/// Initialize the product_metadata table
pub fn init_product_metadata_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    Ok(())
}

/// Load product metadata by ID
pub fn load(conn: &Connection, id: &str) -> Result<Option<ProductMetadata>> {
    let sql = format!("SELECT {} FROM product_metadata WHERE id = ?1", SELECT_COLS);
    let mut stmt = conn.prepare(&sql)?;
    let entry = stmt.query_row([id], from_row).optional()?;
    Ok(entry)
}

/// Save product metadata (upsert)
pub fn save(conn: &Connection, m: &ProductMetadata) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO product_metadata (
            id, source, external_id, title, creator, description, release_date,
            price, currency, rating, rating_count, purchase_count, favorite_count, review_count,
            file_size, file_format, age_rating, genres_json, tags_json, languages_json, product_formats_json,
            series_name, illustrator, voice_actors_json, miscellaneous, update_info, rankings_json,
            extras_json, raw_api_response, raw_html, geo_blocked, cached_at, updated_at, last_accessed
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34)",
        params![
            &m.id, &m.source, &m.external_id, &m.title, &m.creator, &m.description, &m.release_date,
            &m.price, &m.currency, &m.rating, &m.rating_count, &m.purchase_count, &m.favorite_count, &m.review_count,
            &m.file_size, &m.file_format, &m.age_rating, &m.genres_json, &m.tags_json, &m.languages_json, &m.product_formats_json,
            &m.series_name, &m.illustrator, &m.voice_actors_json, &m.miscellaneous, &m.update_info, &m.rankings_json,
            &m.extras_json, &m.raw_api_response, &m.raw_html, &m.geo_blocked, &m.cached_at, &m.updated_at, &m.last_accessed
        ],
    )?;
    Ok(())
}

/// Get product by external ID
pub fn get_by_external_id(
    conn: &Connection,
    source: MetadataSource,
    external_id: &str,
) -> Result<Option<ProductMetadata>> {
    let id = format!("{}:{}", source.as_str(), external_id);
    load(conn, &id)
}

/// List all products from a specific source
pub fn list_by_source(conn: &Connection, source: MetadataSource) -> Result<Vec<ProductMetadata>> {
    let sql = format!(
        "SELECT {} FROM product_metadata WHERE source = ?1",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([source.as_str()], from_row)?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Delete product metadata by ID
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM product_metadata WHERE id = ?1", [id])?;
    Ok(())
}
