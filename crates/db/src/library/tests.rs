//! Unit tests for library module (Diesel API)
//!
//! Metadata CRUD tests have been removed — metadata operations are now handled
//! by gameta_database::DieselBackend (tested in gameta's own test suite).
//! Completeness score tests live in library::metadata::tests.

use crate::library::content::{
    delete_product_content, get_all_content, get_cover, get_screenshots, save as save_content,
    ContentType, ProductContent,
};
use diesel::prelude::*;

/// Helper to create an in-memory Diesel connection for testing
fn setup_diesel_conn() -> diesel::SqliteConnection {
    let mut conn = diesel::SqliteConnection::establish(":memory:")
        .expect("Failed to create in-memory SQLite connection");

    diesel::sql_query(crate::library::content::CREATE_TABLE_SQL)
        .execute(&mut conn)
        .expect("Failed to create product_content table");

    conn
}

fn sample_content(product_id: &str, content_type: ContentType, index: i64) -> ProductContent {
    ProductContent {
        id: 0,
        product_id: product_id.to_string(),
        content_type: content_type.as_str().to_string(),
        content_index: index,
        content_hash: format!("hash-{}-{}", content_type.as_str(), index),
        cached_at: 1704067200, // 2024-01-01
        ..Default::default()
    }
}

// =============================================================================
// Content Tests
// =============================================================================

#[test]
fn test_content_save_and_get_all() {
    let mut conn = setup_diesel_conn();

    let content1 = sample_content("prod1", ContentType::Cover, 0);
    let content2 = sample_content("prod1", ContentType::Screenshot, 0);
    let content3 = sample_content("prod1", ContentType::Screenshot, 1);

    save_content(&mut conn, &content1).unwrap();
    save_content(&mut conn, &content2).unwrap();
    save_content(&mut conn, &content3).unwrap();

    let all = get_all_content(&mut conn, "prod1").unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn test_content_get_cover() {
    let mut conn = setup_diesel_conn();

    let cover = sample_content("prod2", ContentType::Cover, 0);
    let screenshot = sample_content("prod2", ContentType::Screenshot, 0);

    save_content(&mut conn, &cover).unwrap();
    save_content(&mut conn, &screenshot).unwrap();

    let result = get_cover(&mut conn, "prod2").unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().content_type, ContentType::Cover.as_str());
}

#[test]
fn test_content_get_screenshots() {
    let mut conn = setup_diesel_conn();

    let cover = sample_content("prod3", ContentType::Cover, 0);
    let ss1 = sample_content("prod3", ContentType::Screenshot, 0);
    let ss2 = sample_content("prod3", ContentType::Screenshot, 1);

    save_content(&mut conn, &cover).unwrap();
    save_content(&mut conn, &ss1).unwrap();
    save_content(&mut conn, &ss2).unwrap();

    let screenshots = get_screenshots(&mut conn, "prod3").unwrap();
    assert_eq!(screenshots.len(), 2);

    // Should be ordered by index
    assert_eq!(screenshots[0].content_index, 0);
    assert_eq!(screenshots[1].content_index, 1);
}

#[test]
fn test_content_delete() {
    let mut conn = setup_diesel_conn();

    let content1 = sample_content("prod4", ContentType::Cover, 0);
    let content2 = sample_content("prod4", ContentType::Screenshot, 0);

    save_content(&mut conn, &content1).unwrap();
    save_content(&mut conn, &content2).unwrap();

    assert_eq!(get_all_content(&mut conn, "prod4").unwrap().len(), 2);

    delete_product_content(&mut conn, "prod4").unwrap();

    assert_eq!(get_all_content(&mut conn, "prod4").unwrap().len(), 0);
}
