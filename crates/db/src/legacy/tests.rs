//! Unit tests for legacy Rusqlite API

use crate::legacy::content::{
    delete_product_content, get_all_content, init_product_content_schema, save as save_content,
};
use crate::legacy::metadata::{delete, init_product_metadata_schema, list_by_source, load, save};
use crate::library::content::{ContentType, ProductContent};
use crate::library::metadata::{MetadataSource, ProductMetadata};
use rusqlite::Connection;

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
    init_product_metadata_schema(&conn).expect("Failed to init metadata schema");
    init_product_content_schema(&conn).expect("Failed to init content schema");
    conn
}

fn sample_metadata(id: &str) -> ProductMetadata {
    ProductMetadata {
        id: id.to_string(),
        source: "dlsite".to_string(),
        external_id: id.split(':').last().unwrap_or(id).to_string(),
        title: Some("Legacy Test Product".to_string()),
        creator: Some("Legacy Creator".to_string()),
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
        content_hash: format!("legacy-hash-{}", index),
        cached_at: 1704067200,
        ..Default::default()
    }
}

// =============================================================================
// Legacy Metadata Tests
// =============================================================================

#[test]
fn test_legacy_metadata_roundtrip() {
    let conn = setup_test_db();

    let meta = sample_metadata("dlsite:LEGACY001");
    save(&conn, &meta).expect("Failed to save metadata");

    let loaded = load(&conn, "dlsite:LEGACY001")
        .expect("Failed to load metadata")
        .expect("Metadata not found");

    assert_eq!(loaded.id, "dlsite:LEGACY001");
    assert_eq!(loaded.title, Some("Legacy Test Product".to_string()));
}

#[test]
fn test_legacy_metadata_delete() {
    let conn = setup_test_db();

    let meta = sample_metadata("dlsite:LEGACY002");
    save(&conn, &meta).unwrap();

    assert!(load(&conn, "dlsite:LEGACY002").unwrap().is_some());

    delete(&conn, "dlsite:LEGACY002").unwrap();

    assert!(load(&conn, "dlsite:LEGACY002").unwrap().is_none());
}

#[test]
fn test_legacy_metadata_list_by_source() {
    let conn = setup_test_db();

    let mut m1 = sample_metadata("dlsite:L1");
    m1.source = "dlsite".to_string();
    save(&conn, &m1).unwrap();

    let mut m2 = sample_metadata("fanza:L1");
    m2.source = "fanza".to_string();
    save(&conn, &m2).unwrap();

    let dlsite_list = list_by_source(&conn, MetadataSource::DLSite).unwrap();
    assert_eq!(dlsite_list.len(), 1);
    assert_eq!(dlsite_list[0].source, "dlsite");
}

// =============================================================================
// Legacy Content Tests
// =============================================================================

#[test]
fn test_legacy_content_roundtrip() {
    let conn = setup_test_db();

    let content = sample_content("legacy_prod", ContentType::Cover, 0);
    save_content(&conn, &content).expect("Failed to save content");

    let all = get_all_content(&conn, "legacy_prod").expect("Failed to get content");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].content_type, ContentType::Cover.as_str());
}

#[test]
fn test_legacy_content_delete() {
    let conn = setup_test_db();

    let c1 = sample_content("legacy_prod2", ContentType::Cover, 0);
    let c2 = sample_content("legacy_prod2", ContentType::Screenshot, 0);
    save_content(&conn, &c1).unwrap();
    save_content(&conn, &c2).unwrap();

    assert_eq!(get_all_content(&conn, "legacy_prod2").unwrap().len(), 2);

    delete_product_content(&conn, "legacy_prod2").unwrap();

    assert_eq!(get_all_content(&conn, "legacy_prod2").unwrap().len(), 0);
}
