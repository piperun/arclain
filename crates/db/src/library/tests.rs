//! Unit tests for library module (Diesel API)

use crate::library::content::{
    delete_product_content, get_all_content, get_cover, get_screenshots, save as save_content,
    ContentType, ProductContent,
};
use crate::library::metadata::{
    delete, exists, list_by_source, load, save, MetadataSource, ProductMetadata,
};
use diesel::prelude::*;

/// Helper to create an in-memory Diesel connection for testing
fn setup_diesel_conn() -> diesel::SqliteConnection {
    let mut conn = diesel::SqliteConnection::establish(":memory:")
        .expect("Failed to create in-memory SQLite connection");

    // Run migrations / create schema
    diesel::sql_query(crate::library::metadata::CREATE_TABLE_SQL)
        .execute(&mut conn)
        .expect("Failed to create product_metadata table");

    diesel::sql_query(crate::library::content::CREATE_TABLE_SQL)
        .execute(&mut conn)
        .expect("Failed to create product_content table");

    conn
}

fn sample_metadata(id: &str) -> ProductMetadata {
    ProductMetadata {
        id: id.to_string(),
        source: "dlsite".to_string(),
        external_id: id.split(':').last().unwrap_or(id).to_string(),
        title: Some("Test Product".to_string()),
        creator: Some("Test Creator".to_string()),
        cached_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    }
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
// Metadata Tests
// =============================================================================

#[test]
fn test_metadata_save_and_load() {
    let mut conn = setup_diesel_conn();

    let meta = sample_metadata("dlsite:RJ123456");
    save(&mut conn, &meta).expect("Failed to save metadata");

    let loaded = load(&mut conn, "dlsite:RJ123456")
        .expect("Failed to load metadata")
        .expect("Metadata not found");

    assert_eq!(loaded.id, "dlsite:RJ123456");
    assert_eq!(loaded.title, Some("Test Product".to_string()));
    assert_eq!(loaded.creator, Some("Test Creator".to_string()));
}

#[test]
fn test_metadata_load_nonexistent() {
    let mut conn = setup_diesel_conn();

    let result = load(&mut conn, "nonexistent").expect("Query failed");
    assert!(result.is_none());
}

#[test]
fn test_metadata_exists() {
    let mut conn = setup_diesel_conn();

    assert!(!exists(&mut conn, "dlsite:RJ999999").unwrap());

    let meta = sample_metadata("dlsite:RJ999999");
    save(&mut conn, &meta).unwrap();

    assert!(exists(&mut conn, "dlsite:RJ999999").unwrap());
}

#[test]
fn test_metadata_delete() {
    let mut conn = setup_diesel_conn();

    let meta = sample_metadata("dlsite:RJ111111");
    save(&mut conn, &meta).unwrap();
    assert!(exists(&mut conn, "dlsite:RJ111111").unwrap());

    delete(&mut conn, "dlsite:RJ111111").unwrap();
    assert!(!exists(&mut conn, "dlsite:RJ111111").unwrap());
}

#[test]
fn test_metadata_list_by_source() {
    let mut conn = setup_diesel_conn();

    // Save multiple products with different sources
    let mut meta1 = sample_metadata("dlsite:RJ001");
    meta1.source = "dlsite".to_string();
    save(&mut conn, &meta1).unwrap();

    let mut meta2 = sample_metadata("dlsite:RJ002");
    meta2.source = "dlsite".to_string();
    save(&mut conn, &meta2).unwrap();

    // A custom source product (source string matches MetadataSource::Custom.as_str())
    let mut meta3 = sample_metadata("custom:CUSTOM001");
    meta3.source = "custom".to_string(); // Must match MetadataSource::Custom.as_str()
    save(&mut conn, &meta3).unwrap();

    let dlsite_products = list_by_source(&mut conn, MetadataSource::DLSite).unwrap();
    assert_eq!(dlsite_products.len(), 2);

    let custom_products = list_by_source(&mut conn, MetadataSource::Custom).unwrap();
    assert_eq!(custom_products.len(), 1);
}

#[test]
fn test_metadata_upsert() {
    let mut conn = setup_diesel_conn();

    let mut meta = sample_metadata("dlsite:RJ555");
    meta.title = Some("Original Title".to_string());
    save(&mut conn, &meta).unwrap();

    // Update the same record
    meta.title = Some("Updated Title".to_string());
    save(&mut conn, &meta).unwrap();

    let loaded = load(&mut conn, "dlsite:RJ555").unwrap().unwrap();
    assert_eq!(loaded.title, Some("Updated Title".to_string()));
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
