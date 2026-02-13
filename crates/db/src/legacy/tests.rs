//! Unit tests for legacy Rusqlite API (content only)
//!
//! Legacy metadata tests have been removed — metadata CRUD is now handled
//! by gameta_database::DieselBackend, tested in gameta's own test suite.

use crate::legacy::content::{
    delete_product_content, get_all_content, init_product_content_schema, save as save_content,
};
use crate::library::content::{ContentType, ProductContent};
use rusqlite::Connection;

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
    init_product_content_schema(&conn).expect("Failed to init content schema");
    conn
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
