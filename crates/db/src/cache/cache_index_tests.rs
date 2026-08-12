//! Tests for `cache_index.rs`. Lifted from the inline `#[cfg(test)]
//! mod tests { ... }` block — same names, same coverage, just out of
//! the production file.
//!
//! Loaded as `cache_index::tests` via `#[path]` from `cache_index.rs`,
//! so `super::*` here is everything in `cache_index.rs` (including the
//! `CacheType` / `CacheEntry` re-imports from `super::types`).

use super::*;
use diesel::Connection;
use diesel::RunQueryDsl;

// =========================================================================
// CacheType::as_str / from_str round-trip
// =========================================================================

#[test]
fn test_cache_type_roundtrip() {
    let types = [
        CacheType::Screenshot,
        CacheType::Thumbnail,
        CacheType::Metadata,
        CacheType::Html,
        CacheType::Cover,
        CacheType::PluginData,
        CacheType::Other,
    ];
    for ct in &types {
        assert_eq!(CacheType::from_str(ct.as_str()).as_str(), ct.as_str());
    }
}

#[test]
fn test_cache_type_from_str_unknown_falls_back_to_other() {
    assert_eq!(CacheType::from_str("unknown").as_str(), "other");
    assert_eq!(CacheType::from_str("").as_str(), "other");
}

// =========================================================================
// CacheType::from_key
// =========================================================================

#[test]
fn test_from_key_html() {
    assert_eq!(CacheType::from_key("dlsite:html:RJ123456").as_str(), "html");
    assert_eq!(CacheType::from_key("dlsite:RJ123456:html").as_str(), "html");
}

#[test]
fn test_from_key_metadata() {
    assert_eq!(
        CacheType::from_key("dlsite:json:RJ123456").as_str(),
        "metadata"
    );
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:json").as_str(),
        "metadata"
    );
}

#[test]
fn test_from_key_cover() {
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:cover").as_str(),
        "cover"
    );
}

#[test]
fn test_from_key_screenshot() {
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:screenshot_0").as_str(),
        "screenshot"
    );
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:sample_1").as_str(),
        "screenshot"
    );
}

#[test]
fn test_from_key_thumbnail() {
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:thumbnail").as_str(),
        "thumbnail"
    );
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:thumb").as_str(),
        "thumbnail"
    );
}

#[test]
fn test_from_key_other() {
    assert_eq!(
        CacheType::from_key("dlsite:RJ123456:something").as_str(),
        "other"
    );
    assert_eq!(CacheType::from_key("unknown").as_str(), "other");
}

// =========================================================================
// CacheType::extract_product_id
// =========================================================================

#[test]
fn test_extract_product_id_standard_format() {
    // "dlsite:RJ123456:cover" -> "dlsite:RJ123456"
    assert_eq!(
        CacheType::extract_product_id("dlsite:RJ123456:cover"),
        Some("dlsite:RJ123456".to_string())
    );
    assert_eq!(
        CacheType::extract_product_id("dlsite:VJ001234:screenshot_0"),
        Some("dlsite:VJ001234".to_string())
    );
    assert_eq!(
        CacheType::extract_product_id("dlsite:BJ999999:thumb"),
        Some("dlsite:BJ999999".to_string())
    );
}

#[test]
fn test_extract_product_id_html_json_format() {
    // "dlsite:html:RJ123456" -> "dlsite:RJ123456"
    assert_eq!(
        CacheType::extract_product_id("dlsite:html:RJ123456"),
        Some("dlsite:RJ123456".to_string())
    );
    assert_eq!(
        CacheType::extract_product_id("dlsite:json:RJ123456"),
        Some("dlsite:RJ123456".to_string())
    );
}

#[test]
fn test_extract_product_id_no_match() {
    assert_eq!(CacheType::extract_product_id(""), None);
    assert_eq!(CacheType::extract_product_id("single"), None);
    // "dlsite:search:query" — search is skipped
    assert_eq!(CacheType::extract_product_id("dlsite:search:test"), None);
}

#[test]
fn test_extract_product_id_long_alphanumeric() {
    // Non-RJ/VJ/BJ but long enough to be a product ID
    assert_eq!(
        CacheType::extract_product_id("steam:12345678:cover"),
        Some("steam:12345678".to_string())
    );
}

// =========================================================================
// Diesel CRUD operations
// =========================================================================

mod diesel_crud {
    use super::*;

    fn setup_diesel() -> diesel::SqliteConnection {
        let mut conn = diesel::SqliteConnection::establish(":memory:").expect("in-memory SQLite");
        diesel::sql_query(
            "CREATE TABLE cache_index (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key TEXT UNIQUE NOT NULL,
                product_id TEXT,
                content_hash TEXT NOT NULL,
                source_url TEXT,
                cache_type TEXT NOT NULL DEFAULT 'other',
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                last_accessed TEXT,
                size_bytes INTEGER
            )",
        )
        .execute(&mut conn)
        .unwrap();
        // Minimal product_metadata stub for stats queries
        diesel::sql_query("CREATE TABLE product_metadata (id TEXT PRIMARY KEY)")
            .execute(&mut conn)
            .unwrap();
        conn
    }

    #[test]
    fn test_upsert_and_get() {
        let mut conn = setup_diesel();
        let id = upsert_cache_entry(
            &mut conn,
            "dlsite:RJ100:cover",
            Some("dlsite:RJ100"),
            "sha256-abc",
            Some("https://example.com/img.jpg"),
            CacheType::Cover,
            Some(2048),
        )
        .unwrap();
        assert!(id > 0);

        let entry = get_cache_entry(&mut conn, "dlsite:RJ100:cover")
            .unwrap()
            .unwrap();
        assert_eq!(entry.key, "dlsite:RJ100:cover");
        assert_eq!(entry.product_id, Some("dlsite:RJ100".to_string()));
        assert_eq!(entry.content_hash, "sha256-abc");
        assert_eq!(entry.cache_type, CacheType::Cover);
        assert_eq!(entry.size_bytes, Some(2048));
    }

    #[test]
    fn test_upsert_updates_existing() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "key1",
            None,
            "hash-v1",
            None,
            CacheType::Other,
            Some(100),
        )
        .unwrap();

        upsert_cache_entry(
            &mut conn,
            "key1",
            None,
            "hash-v2",
            None,
            CacheType::Other,
            Some(200),
        )
        .unwrap();

        let entry = get_cache_entry(&mut conn, "key1").unwrap().unwrap();
        assert_eq!(entry.content_hash, "hash-v2");
        assert_eq!(entry.size_bytes, Some(200));
    }

    #[test]
    fn upsert_reclassifies_existing_entry_and_product() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "shared-key",
            Some("product-a"),
            "hash-v1",
            None,
            CacheType::Other,
            Some(100),
        )
        .unwrap();

        upsert_cache_entry(
            &mut conn,
            "shared-key",
            Some("product-b"),
            "hash-v2",
            None,
            CacheType::PluginData,
            Some(200),
        )
        .unwrap();

        let entry = get_cache_entry(&mut conn, "shared-key").unwrap().unwrap();
        assert_eq!(entry.cache_type, CacheType::PluginData);
        assert_eq!(entry.product_id.as_deref(), Some("product-b"));
    }

    #[test]
    fn test_get_nonexistent() {
        let mut conn = setup_diesel();
        assert!(get_cache_entry(&mut conn, "nope").unwrap().is_none());
    }

    #[test]
    fn test_has_cache_entry() {
        let mut conn = setup_diesel();
        assert!(!has_cache_entry(&mut conn, "k").unwrap());

        upsert_cache_entry(&mut conn, "k", None, "h", None, CacheType::Other, None).unwrap();
        assert!(has_cache_entry(&mut conn, "k").unwrap());
    }

    #[test]
    fn has_content_hash_uses_a_targeted_existence_query() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "first",
            None,
            "shared-hash",
            None,
            CacheType::Other,
            None,
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "second",
            None,
            "other-hash",
            None,
            CacheType::Other,
            None,
        )
        .unwrap();

        assert!(has_content_hash(&mut conn, "shared-hash").unwrap());
        assert!(!has_content_hash(&mut conn, "missing-hash").unwrap());
    }

    #[test]
    fn test_delete_cache_entry() {
        let mut conn = setup_diesel();
        upsert_cache_entry(&mut conn, "del", None, "h", None, CacheType::Other, None).unwrap();
        assert!(delete_cache_entry(&mut conn, "del").unwrap());
        assert!(!has_cache_entry(&mut conn, "del").unwrap());

        // Deleting non-existent returns false
        assert!(!delete_cache_entry(&mut conn, "del").unwrap());
    }

    #[test]
    fn test_get_entries_by_product() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "dlsite:RJ200:cover",
            Some("dlsite:RJ200"),
            "h1",
            None,
            CacheType::Cover,
            None,
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "dlsite:RJ200:screenshot_0",
            Some("dlsite:RJ200"),
            "h2",
            None,
            CacheType::Screenshot,
            None,
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "dlsite:RJ999:cover",
            Some("dlsite:RJ999"),
            "h3",
            None,
            CacheType::Cover,
            None,
        )
        .unwrap();

        let entries = get_entries_by_product(&mut conn, "dlsite:RJ200").unwrap();
        assert_eq!(entries.len(), 2);

        let entries = get_entries_by_product(&mut conn, "dlsite:RJ999").unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_delete_by_pattern() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "dlsite:RJ300:cover",
            None,
            "h",
            None,
            CacheType::Cover,
            None,
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "dlsite:RJ300:html",
            None,
            "h",
            None,
            CacheType::Html,
            None,
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "steam:123:cover",
            None,
            "h",
            None,
            CacheType::Cover,
            None,
        )
        .unwrap();

        let deleted = delete_by_pattern(&mut conn, "dlsite:*").unwrap();
        assert_eq!(deleted, 2);
        assert!(has_cache_entry(&mut conn, "steam:123:cover").unwrap());
    }

    #[test]
    fn delete_by_pattern_treats_sql_wildcards_and_escape_as_literals() {
        let mut conn = setup_diesel();
        for cache_key in [
            "owner:a_b:item",
            "owner:axb:item",
            "percent%literal",
            "percentXliteral",
            "under_literal",
            "underXliteral",
            "bang!literal",
            "wild:one",
            "wild:two",
        ] {
            upsert_cache_entry(
                &mut conn,
                cache_key,
                None,
                "h",
                None,
                CacheType::Other,
                None,
            )
            .unwrap();
        }

        assert_eq!(delete_by_pattern(&mut conn, "owner:a_b:*").unwrap(), 1);
        assert!(has_cache_entry(&mut conn, "owner:axb:item").unwrap());

        assert_eq!(delete_by_pattern(&mut conn, "percent%literal").unwrap(), 1);
        assert!(has_cache_entry(&mut conn, "percentXliteral").unwrap());

        assert_eq!(delete_by_pattern(&mut conn, "under_literal").unwrap(), 1);
        assert!(has_cache_entry(&mut conn, "underXliteral").unwrap());

        assert_eq!(delete_by_pattern(&mut conn, "bang!literal").unwrap(), 1);
        assert_eq!(delete_by_pattern(&mut conn, "wild:*").unwrap(), 2);
    }

    #[test]
    fn owner_prefix_count_and_page_are_exact_bounded_sql_projections() {
        let mut conn = setup_diesel();
        let owner_prefix = "\u{1}arclain-cache:v1:p:8:plugin-a:";
        for (cache_key, entry_type) in [
            (
                format!("{owner_prefix}state:%literal"),
                CacheType::PluginData,
            ),
            (
                format!("{owner_prefix}state:_literal"),
                CacheType::PluginData,
            ),
            (format!("{owner_prefix}state:one"), CacheType::PluginData),
            (format!("{owner_prefix}state:two"), CacheType::PluginData),
            (
                format!("{owner_prefix}state:reserved-metadata"),
                CacheType::Metadata,
            ),
            (
                format!("{owner_prefix}State:case-sensitive"),
                CacheType::PluginData,
            ),
            (
                format!("{owner_prefix}status:outside-prefix"),
                CacheType::PluginData,
            ),
            (
                "\u{1}arclain-cache:v1:h:0::state:host".to_string(),
                CacheType::PluginData,
            ),
            (
                "\u{1}arclain-cache:v1:p:8:plugin-b:state:other".to_string(),
                CacheType::PluginData,
            ),
        ] {
            upsert_cache_entry(&mut conn, &cache_key, None, "h", None, entry_type, Some(1))
                .unwrap();
        }

        let state_prefix = format!("{owner_prefix}state:");
        assert_eq!(
            count_keys_with_prefix(&mut conn, &state_prefix, CacheType::PluginData).unwrap(),
            4
        );
        assert_eq!(
            list_keys_with_prefix_page(&mut conn, &state_prefix, CacheType::PluginData, 1, 2,)
                .unwrap(),
            vec![
                format!("{owner_prefix}state:_literal"),
                format!("{owner_prefix}state:one"),
            ]
        );

        let percent_prefix = format!("{owner_prefix}state:%");
        assert_eq!(
            count_keys_with_prefix(&mut conn, &percent_prefix, CacheType::PluginData).unwrap(),
            1
        );
        let underscore_prefix = format!("{owner_prefix}state:_");
        assert_eq!(
            list_keys_with_prefix_page(
                &mut conn,
                &underscore_prefix,
                CacheType::PluginData,
                0,
                256,
            )
            .unwrap(),
            vec![format!("{owner_prefix}state:_literal")]
        );
    }

    #[test]
    fn test_clear_all_entries() {
        let mut conn = setup_diesel();
        upsert_cache_entry(&mut conn, "a", None, "h", None, CacheType::Other, None).unwrap();
        upsert_cache_entry(&mut conn, "b", None, "h", None, CacheType::Other, None).unwrap();

        clear_all_entries(&mut conn).unwrap();
        assert!(!has_cache_entry(&mut conn, "a").unwrap());
        assert!(!has_cache_entry(&mut conn, "b").unwrap());
    }

    #[test]
    fn test_touch_cache_entry() {
        let mut conn = setup_diesel();
        upsert_cache_entry(&mut conn, "t", None, "h", None, CacheType::Other, None).unwrap();

        let before = get_cache_entry(&mut conn, "t").unwrap().unwrap();
        assert!(before.last_accessed.is_none());

        touch_cache_entry(&mut conn, "t").unwrap();
        let after = get_cache_entry(&mut conn, "t").unwrap().unwrap();
        assert!(after.last_accessed.is_some());
    }

    #[test]
    fn list_entries_lru_orders_by_effective_access_then_id() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "old-created",
            None,
            "h1",
            None,
            CacheType::Other,
            Some(1),
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "recent-access",
            None,
            "h2",
            None,
            CacheType::Other,
            Some(2),
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "old-access",
            None,
            "h3",
            None,
            CacheType::Other,
            Some(3),
        )
        .unwrap();
        for statement in [
            "UPDATE cache_index SET created_at = '2020-01-01 00:00:00' WHERE key = 'old-created'",
            "UPDATE cache_index SET created_at = '2019-01-01 00:00:00', last_accessed = '2025-01-01 00:00:00' WHERE key = 'recent-access'",
            "UPDATE cache_index SET created_at = '2024-01-01 00:00:00', last_accessed = '2021-01-01 00:00:00' WHERE key = 'old-access'",
        ] {
            diesel::sql_query(statement).execute(&mut conn).unwrap();
        }

        let entries = list_entries_lru(&mut conn).unwrap();
        let keys: Vec<_> = entries.iter().map(|entry| entry.key.as_str()).collect();

        assert_eq!(keys, vec!["old-created", "old-access", "recent-access"]);
        let page = list_entries_lru_page(&mut conn, 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].key, "old-access");
        assert!(list_entries_lru_page(&mut conn, 0, 0).unwrap().is_empty());
    }

    #[test]
    fn test_get_all_content_hashes() {
        let mut conn = setup_diesel();
        upsert_cache_entry(&mut conn, "a", None, "hash1", None, CacheType::Other, None).unwrap();
        upsert_cache_entry(&mut conn, "b", None, "hash2", None, CacheType::Other, None).unwrap();
        upsert_cache_entry(&mut conn, "c", None, "hash1", None, CacheType::Other, None).unwrap();

        let hashes = get_all_content_hashes(&mut conn).unwrap();
        assert_eq!(hashes.len(), 2); // distinct
        assert!(hashes.contains(&"hash1".to_string()));
        assert!(hashes.contains(&"hash2".to_string()));
    }

    #[test]
    fn test_get_cache_stats_empty() {
        let mut conn = setup_diesel();
        let stats = get_cache_stats(&mut conn).unwrap();
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_size_bytes, 0);
        assert!(stats.oldest_entry_date.is_none());
    }

    #[test]
    fn test_get_cache_stats_with_data() {
        let mut conn = setup_diesel();
        upsert_cache_entry(
            &mut conn,
            "dlsite:RJ100:cover",
            Some("dlsite:RJ100"),
            "h1",
            None,
            CacheType::Cover,
            Some(1000),
        )
        .unwrap();
        upsert_cache_entry(
            &mut conn,
            "dlsite:search:query1",
            None,
            "h2",
            None,
            CacheType::Other,
            Some(500),
        )
        .unwrap();

        let stats = get_cache_stats(&mut conn).unwrap();
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.total_size_bytes, 1500);
        assert_eq!(stats.search_cache_entries, 1);
        assert!(stats.oldest_entry_date.is_some());
    }
}
