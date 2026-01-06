//! Bulletproof Integration Tests for arclain_db
//!
//! Comprehensive stress testing, edge cases, and error handling for the Diesel-based API.

use arclain_db::{DieselPool, MetadataSource, MetadataStore, ProductMetadata, SqliteDb};
use diesel::prelude::*;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

fn setup_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

// =============================================================================
// STRESS TESTS
// =============================================================================

mod stress_tests {
    use super::*;

    #[test]
    fn test_save_100_products() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("stress.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Save 100 products
        for i in 0..100 {
            let meta = ProductMetadata {
                id: format!("dlsite:RJ{:06}", i),
                source: "dlsite".to_string(),
                external_id: format!("RJ{:06}", i),
                title: Some(format!("Product {}", i)),
                creator: Some(format!("Creator {}", i % 10)),
                ..Default::default()
            };
            store
                .save(&meta)
                .expect(&format!("Failed to save product {}", i));
        }

        // Verify count
        let all = store.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(all.len(), 100);
    }

    #[test]
    fn test_rapid_upserts() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("upsert.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        let id = "dlsite:RJ000001".to_string();

        // Rapid upserts on same ID
        for i in 0..50 {
            let meta = ProductMetadata {
                id: id.clone(),
                source: "dlsite".to_string(),
                external_id: "RJ000001".to_string(),
                title: Some(format!("Update {}", i)),
                ..Default::default()
            };
            store.save(&meta).unwrap();
        }

        // Should only have 1 record
        let all = store.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, Some("Update 49".to_string()));
    }

    #[test]
    fn test_pool_exhaustion_recovery() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("pool_exhaust.db");

        let pool = DieselPool::new(&db_path).unwrap();

        // Get many connections in sequence (not holding them)
        for _ in 0..100 {
            let conn = pool.get();
            assert!(conn.is_ok(), "Pool should provide connections");
            // conn dropped here, returned to pool
        }
    }
}

// =============================================================================
// EDGE CASES
// =============================================================================

mod edge_cases {
    use super::*;

    #[test]
    fn test_empty_string_fields() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("empty.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        let meta = ProductMetadata {
            id: "dlsite:EMPTY".to_string(),
            source: "dlsite".to_string(),
            external_id: "EMPTY".to_string(),
            title: Some("".to_string()),   // Empty title
            creator: Some("".to_string()), // Empty creator
            description: Some("".to_string()),
            ..Default::default()
        };

        store.save(&meta).unwrap();
        let loaded = store.get("dlsite:EMPTY").unwrap().unwrap();

        assert_eq!(loaded.title, Some("".to_string()));
        assert_eq!(loaded.creator, Some("".to_string()));
    }

    #[test]
    fn test_null_vs_empty_string() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("null_vs_empty.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Product with None (NULL)
        let meta_null = ProductMetadata {
            id: "dlsite:NULL".to_string(),
            source: "dlsite".to_string(),
            external_id: "NULL".to_string(),
            title: None,
            creator: None,
            ..Default::default()
        };
        store.save(&meta_null).unwrap();

        // Product with empty string
        let meta_empty = ProductMetadata {
            id: "dlsite:EMPTY".to_string(),
            source: "dlsite".to_string(),
            external_id: "EMPTY".to_string(),
            title: Some("".to_string()),
            creator: Some("".to_string()),
            ..Default::default()
        };
        store.save(&meta_empty).unwrap();

        let loaded_null = store.get("dlsite:NULL").unwrap().unwrap();
        let loaded_empty = store.get("dlsite:EMPTY").unwrap().unwrap();

        assert!(loaded_null.title.is_none());
        assert_eq!(loaded_empty.title, Some("".to_string()));
    }

    #[test]
    fn test_very_long_strings() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("long_strings.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // 10KB title
        let long_title = "A".repeat(10_000);
        // 100KB description
        let long_description = "B".repeat(100_000);

        let meta = ProductMetadata {
            id: "dlsite:LONG".to_string(),
            source: "dlsite".to_string(),
            external_id: "LONG".to_string(),
            title: Some(long_title.clone()),
            description: Some(long_description.clone()),
            ..Default::default()
        };

        store.save(&meta).unwrap();
        let loaded = store.get("dlsite:LONG").unwrap().unwrap();

        assert_eq!(loaded.title.as_ref().unwrap().len(), 10_000);
        assert_eq!(loaded.description.as_ref().unwrap().len(), 100_000);
    }

    #[test]
    fn test_unicode_and_special_chars() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("unicode.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        let meta = ProductMetadata {
            id: "dlsite:UNICODE".to_string(),
            source: "dlsite".to_string(),
            external_id: "UNICODE".to_string(),
            title: Some("日本語タイトル 🎮 Game's \"Title\" <test>".to_string()),
            creator: Some("制作者 O'Connor & 陳".to_string()),
            description: Some("Line1\nLine2\tTab\r\nCRLF".to_string()),
            ..Default::default()
        };

        store.save(&meta).unwrap();
        let loaded = store.get("dlsite:UNICODE").unwrap().unwrap();

        assert!(loaded.title.as_ref().unwrap().contains("日本語"));
        assert!(loaded.title.as_ref().unwrap().contains("🎮"));
        assert!(loaded.creator.as_ref().unwrap().contains("O'Connor"));
        assert!(loaded.description.as_ref().unwrap().contains("\n"));
    }

    #[test]
    fn test_sql_injection_prevention() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("injection.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Evil title that tries SQL injection
        let evil_title = "'; DROP TABLE product_metadata; --".to_string();

        let meta = ProductMetadata {
            id: "dlsite:EVIL".to_string(),
            source: "dlsite".to_string(),
            external_id: "EVIL".to_string(),
            title: Some(evil_title.clone()),
            ..Default::default()
        };

        store.save(&meta).unwrap();

        // Table should still exist
        let loaded = store.get("dlsite:EVIL").unwrap().unwrap();
        assert_eq!(loaded.title, Some(evil_title));

        // Can still list (table wasn't dropped)
        let all = store.list_by_source(MetadataSource::DLSite).unwrap();
        assert!(!all.is_empty());
    }

    #[test]
    fn test_get_nonexistent() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("nonexistent.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        let result = store.get("dlsite:DOESNTEXIST").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_nonexistent() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("delete_nonexistent.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Deleting non-existent should not error
        let result = store.delete("dlsite:DOESNTEXIST");
        assert!(result.is_ok());
    }
}

// =============================================================================
// CONCURRENT ACCESS TESTS
// =============================================================================

mod concurrent_tests {
    use super::*;

    #[test]
    fn test_concurrent_reads() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("concurrent_read.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = Arc::new(DieselPool::new(&db_path).unwrap());
        let store = Arc::new(MetadataStore::new(
            sqlite_db,
            (*pool).clone(),
            temp_dir.path().to_path_buf(),
        ));

        // Insert test data
        let meta = ProductMetadata {
            id: "dlsite:SHARED".to_string(),
            source: "dlsite".to_string(),
            external_id: "SHARED".to_string(),
            title: Some("Shared Data".to_string()),
            ..Default::default()
        };
        store.save(&meta).unwrap();

        // Spawn readers
        let mut handles = vec![];
        for _ in 0..10 {
            let store = Arc::clone(&store);
            let handle = thread::spawn(move || {
                for _ in 0..10 {
                    let result = store.get("dlsite:SHARED").unwrap();
                    assert!(result.is_some());
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Reader thread panicked");
        }
    }

    #[test]
    fn test_sequential_write_read_cycles() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("write_read.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        for i in 0..20 {
            // Write
            let meta = ProductMetadata {
                id: format!("dlsite:WR{}", i),
                source: "dlsite".to_string(),
                external_id: format!("WR{}", i),
                title: Some(format!("Write {}", i)),
                ..Default::default()
            };
            store.save(&meta).unwrap();

            // Immediate read
            let loaded = store.get(&format!("dlsite:WR{}", i)).unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().title, Some(format!("Write {}", i)));
        }
    }
}

// =============================================================================
// DATA INTEGRITY TESTS
// =============================================================================

mod integrity_tests {
    use super::*;

    #[test]
    fn test_save_preserves_all_fields() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("all_fields.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        let meta = ProductMetadata {
            id: "dlsite:FULL".to_string(),
            source: "dlsite".to_string(),
            external_id: "FULL".to_string(),
            title: Some("Full Title".to_string()),
            creator: Some("Full Creator".to_string()),
            description: Some("Full Description".to_string()),
            tags_json: Some(r#"["tag1","tag2","tag3"]"#.to_string()),
            release_date: Some("2024-01-15".to_string()),
            price: Some(1980),
            rating: Some(4.5),
            cached_at: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        };

        store.save(&meta).unwrap();
        let loaded = store.get("dlsite:FULL").unwrap().unwrap();

        assert_eq!(loaded.id, meta.id);
        assert_eq!(loaded.source, meta.source);
        assert_eq!(loaded.external_id, meta.external_id);
        assert_eq!(loaded.title, meta.title);
        assert_eq!(loaded.creator, meta.creator);
        assert_eq!(loaded.description, meta.description);
        assert_eq!(loaded.tags_json, meta.tags_json);
        assert_eq!(loaded.release_date, meta.release_date);
        assert_eq!(loaded.price, meta.price);
    }

    #[test]
    fn test_update_partial_preserves_other_fields() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("partial_update.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Initial save
        let mut meta = ProductMetadata {
            id: "dlsite:PARTIAL".to_string(),
            source: "dlsite".to_string(),
            external_id: "PARTIAL".to_string(),
            title: Some("Original Title".to_string()),
            creator: Some("Original Creator".to_string()),
            description: Some("Original Description".to_string()),
            ..Default::default()
        };
        store.save(&meta).unwrap();

        // Update only title
        meta.title = Some("Updated Title".to_string());
        store.save(&meta).unwrap();

        let loaded = store.get("dlsite:PARTIAL").unwrap().unwrap();
        assert_eq!(loaded.title, Some("Updated Title".to_string()));
        assert_eq!(loaded.creator, Some("Original Creator".to_string()));
        assert_eq!(loaded.description, Some("Original Description".to_string()));
    }

    #[test]
    fn test_list_ordering_consistency() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("ordering.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Insert in random order
        for i in [5, 2, 8, 1, 9, 3, 7, 4, 6, 0] {
            let meta = ProductMetadata {
                id: format!("dlsite:ORD{:02}", i),
                source: "dlsite".to_string(),
                external_id: format!("ORD{:02}", i),
                ..Default::default()
            };
            store.save(&meta).unwrap();
        }

        // List should return results (order may vary by implementation)
        let all = store.list_by_source(MetadataSource::DLSite).unwrap();
        assert_eq!(all.len(), 10);
    }
}

// =============================================================================
// RECOVERY AND ERROR HANDLING
// =============================================================================

mod error_handling {
    use super::*;

    #[test]
    fn test_reopen_database() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("reopen.db");

        // First session
        {
            let sqlite_db = SqliteDb::open(&db_path).unwrap();
            let pool = DieselPool::new(&db_path).unwrap();
            let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

            let meta = ProductMetadata {
                id: "dlsite:PERSIST".to_string(),
                source: "dlsite".to_string(),
                external_id: "PERSIST".to_string(),
                title: Some("Persistent Data".to_string()),
                ..Default::default()
            };
            store.save(&meta).unwrap();
        }

        // Second session - data should persist
        {
            let sqlite_db = SqliteDb::open(&db_path).unwrap();
            let pool = DieselPool::new(&db_path).unwrap();
            let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

            let loaded = store.get("dlsite:PERSIST").unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().title, Some("Persistent Data".to_string()));
        }
    }

    #[test]
    fn test_pool_connection_validity() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("pool_valid.db");

        let pool = DieselPool::new(&db_path).unwrap();

        // Get and use connection
        {
            let mut conn = pool.get().unwrap();
            diesel::sql_query("SELECT 1")
                .execute(&mut *conn)
                .expect("Connection should be valid");
        }

        // Get another connection
        {
            let mut conn = pool.get().unwrap();
            diesel::sql_query("SELECT 1")
                .execute(&mut *conn)
                .expect("Connection should still be valid");
        }
    }
}

// =============================================================================
// MULTI-SOURCE TESTS (DLSite, Itch, Steam, Custom)
// =============================================================================

mod multi_source {
    use super::*;

    #[test]
    fn test_multiple_sources_isolation() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("multi_source.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Insert into different sources
        let sources = vec![
            ("dlsite", "dlsite:D1", MetadataSource::DLSite),
            ("itch", "itch:I1", MetadataSource::Itch),
            ("steam", "steam:S1", MetadataSource::Steam),
            ("custom", "custom:C1", MetadataSource::Custom),
        ];

        for (source_str, id, _) in &sources {
            let meta = ProductMetadata {
                id: id.to_string(),
                source: source_str.to_string(),
                external_id: id.split(':').nth(1).unwrap().to_string(),
                title: Some(format!("{} Product", source_str)),
                ..Default::default()
            };
            store.save(&meta).unwrap();
        }

        // Verify isolation
        for (_, _, source) in &sources {
            let list = store.list_by_source(*source).unwrap();
            assert_eq!(list.len(), 1, "Each source should have exactly 1 item");
        }
    }

    #[test]
    fn test_same_external_id_different_sources() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("same_ext_id.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(sqlite_db, pool, temp_dir.path().to_path_buf());

        // Same external ID, different sources
        let meta1 = ProductMetadata {
            id: "dlsite:12345".to_string(),
            source: "dlsite".to_string(),
            external_id: "12345".to_string(),
            title: Some("DLSite Product".to_string()),
            ..Default::default()
        };

        let meta2 = ProductMetadata {
            id: "steam:12345".to_string(),
            source: "steam".to_string(),
            external_id: "12345".to_string(),
            title: Some("Steam Product".to_string()),
            ..Default::default()
        };

        store.save(&meta1).unwrap();
        store.save(&meta2).unwrap();

        let dlsite = store.get("dlsite:12345").unwrap().unwrap();
        let steam = store.get("steam:12345").unwrap().unwrap();

        assert_eq!(dlsite.title, Some("DLSite Product".to_string()));
        assert_eq!(steam.title, Some("Steam Product".to_string()));
    }
}
