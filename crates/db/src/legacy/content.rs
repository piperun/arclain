//! Legacy Rusqlite implementation for product content
//!
//! Moved from crates/db/src/library/content.rs

use crate::library::content::{ContentType, ProductContent, CREATE_TABLE_SQL};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Parse from a database row
fn from_row(row: &Row<'_>) -> rusqlite::Result<ProductContent> {
    Ok(ProductContent {
        id: row.get(0)?,
        product_id: row.get(1)?,
        content_type: row.get(2)?,
        content_index: row.get(3)?,
        content_hash: row.get(4)?,
        source_url: row.get(5).ok(),
        width: row.get(6).ok(),
        height: row.get(7).ok(),
        size_bytes: row.get(8).ok(),
        cached_at: row.get(9)?,
    })
}

const SELECT_COLS: &str =
    "id, product_id, content_type, content_index, content_hash, source_url, width, height, size_bytes, cached_at";

/// Initialize the product_content table
pub fn init_product_content_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    Ok(())
}

/// Save product content (upsert)
pub fn save(conn: &Connection, c: &ProductContent) -> Result<i64> {
    conn.execute(
        "INSERT OR REPLACE INTO product_content (
            product_id, content_type, content_index, content_hash, source_url, width, height, size_bytes, cached_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &c.product_id,
            &c.content_type,
            &c.content_index,
            &c.content_hash,
            &c.source_url,
            &c.width,
            &c.height,
            &c.size_bytes,
            &c.cached_at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get cover image for a product
pub fn get_cover(conn: &Connection, product_id: &str) -> Result<Option<ProductContent>> {
    let sql = format!(
        "SELECT {} FROM product_content WHERE product_id = ?1 AND content_type = ?2 LIMIT 1",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let entry = stmt
        .query_row([product_id, ContentType::Cover.as_str()], from_row)
        .optional()?;
    Ok(entry)
}

/// Get all screenshots for a product, ordered by index
pub fn get_screenshots(conn: &Connection, product_id: &str) -> Result<Vec<ProductContent>> {
    let sql = format!(
        "SELECT {} FROM product_content WHERE product_id = ?1 AND content_type = ?2 ORDER BY content_index ASC",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([product_id, ContentType::Screenshot.as_str()], from_row)?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Get all content for a product
pub fn get_all_content(conn: &Connection, product_id: &str) -> Result<Vec<ProductContent>> {
    let sql = format!(
        "SELECT {} FROM product_content WHERE product_id = ?1 ORDER BY content_type, content_index ASC",
        SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([product_id], from_row)?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Delete all content for a product
pub fn delete_product_content(conn: &Connection, product_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM product_content WHERE product_id = ?1",
        [product_id],
    )?;
    Ok(())
}
