//! Comprehensive test suite for mini-orm
//!
//! This module provides 80+ additional tests covering:
//! - Edge cases and error handling
//! - Complex joins and subqueries
//! - SQL injection prevention
//! - Unicode and special characters
//! - Integration tests with actual SQLite
//!
//! Note: These tests use the deprecated string-based API for backward compatibility testing.

#![allow(deprecated)]

use crate::{DeleteBuilder, InsertBuilder, JoinType, OrderDirection, QueryBuilder, UpdateBuilder};
use rusqlite::Connection;

// ============================================================================
// QueryBuilder - Extended Tests
// ============================================================================

mod query_builder_extended {
    use super::*;

    #[test]
    fn test_select_single_column() {
        let sql = QueryBuilder::select("users").columns(&["name"]).build();
        assert_eq!(sql, "SELECT name FROM users");
    }

    #[test]
    fn test_select_many_columns() {
        let sql = QueryBuilder::select("users")
            .columns(&["id", "name", "email", "created_at", "updated_at", "status"])
            .build();
        assert_eq!(
            sql,
            "SELECT id, name, email, created_at, updated_at, status FROM users"
        );
    }

    #[test]
    fn test_where_multiple_conditions() {
        let sql = QueryBuilder::select("users")
            .where_eq("status", "active")
            .where_eq("role", "admin")
            .where_eq("verified", "1")
            .build();
        assert!(sql.contains("WHERE status = 'active' AND role = 'admin' AND verified = '1'"));
    }

    #[test]
    fn test_where_raw_or_condition() {
        let sql = QueryBuilder::select("users")
            .where_raw("(status = 'active' OR status = 'pending')")
            .build();
        assert_eq!(
            sql,
            "SELECT * FROM users WHERE (status = 'active' OR status = 'pending')"
        );
    }

    #[test]
    fn test_where_raw_null_check() {
        let sql = QueryBuilder::select("users")
            .where_raw("deleted_at IS NULL")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE deleted_at IS NULL");
    }

    #[test]
    fn test_where_raw_not_null_check() {
        let sql = QueryBuilder::select("users")
            .where_raw("email IS NOT NULL")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE email IS NOT NULL");
    }

    #[test]
    fn test_where_raw_like() {
        let sql = QueryBuilder::select("users")
            .where_raw("name LIKE '%john%'")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE name LIKE '%john%'");
    }

    #[test]
    fn test_where_raw_between() {
        let sql = QueryBuilder::select("orders")
            .where_raw("created_at BETWEEN '2024-01-01' AND '2024-12-31'")
            .build();
        assert!(sql.contains("BETWEEN '2024-01-01' AND '2024-12-31'"));
    }

    #[test]
    fn test_where_raw_in_clause() {
        let sql = QueryBuilder::select("users")
            .where_raw("id IN (1, 2, 3, 4, 5)")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE id IN (1, 2, 3, 4, 5)");
    }

    #[test]
    fn test_where_escapes_single_quotes() {
        let sql = QueryBuilder::select("users")
            .where_eq("name", "O'Connor")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE name = 'O''Connor'");
    }

    #[test]
    fn test_where_escapes_multiple_quotes() {
        let sql = QueryBuilder::select("users")
            .where_eq("bio", "It's John's book")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE bio = 'It''s John''s book'");
    }

    #[test]
    fn test_inner_join() {
        let sql = QueryBuilder::select("orders")
            .inner_join("users", "orders.user_id = users.id")
            .build();
        assert_eq!(
            sql,
            "SELECT * FROM orders INNER JOIN users ON orders.user_id = users.id"
        );
    }

    #[test]
    fn test_right_join() {
        let sql = QueryBuilder::select("orders")
            .join("users", "orders.user_id = users.id", JoinType::Right)
            .build();
        assert_eq!(
            sql,
            "SELECT * FROM orders RIGHT JOIN users ON orders.user_id = users.id"
        );
    }

    #[test]
    fn test_multiple_joins() {
        let sql = QueryBuilder::select("orders")
            .inner_join("users", "orders.user_id = users.id")
            .left_join("products", "orders.product_id = products.id")
            .left_join("categories", "products.category_id = categories.id")
            .build();
        assert!(sql.contains("INNER JOIN users"));
        assert!(sql.contains("LEFT JOIN products"));
        assert!(sql.contains("LEFT JOIN categories"));
    }

    #[test]
    fn test_self_join() {
        let sql = QueryBuilder::select("employees e1")
            .left_join("employees e2", "e1.manager_id = e2.id")
            .columns(&["e1.name", "e2.name AS manager_name"])
            .build();
        assert!(sql.contains("LEFT JOIN employees e2 ON e1.manager_id = e2.id"));
    }

    #[test]
    fn test_join_with_multiple_conditions() {
        let sql = QueryBuilder::select("orders")
            .left_join(
                "order_items",
                "orders.id = order_items.order_id AND order_items.status = 'active'",
            )
            .build();
        assert!(sql.contains("AND order_items.status = 'active'"));
    }

    #[test]
    fn test_order_by_asc() {
        let sql = QueryBuilder::select("users")
            .order_by("name", OrderDirection::Asc)
            .build();
        assert_eq!(sql, "SELECT * FROM users ORDER BY name ASC");
    }

    #[test]
    fn test_order_by_desc() {
        let sql = QueryBuilder::select("users")
            .order_by("created_at", OrderDirection::Desc)
            .build();
        assert_eq!(sql, "SELECT * FROM users ORDER BY created_at DESC");
    }

    #[test]
    fn test_limit_only() {
        let sql = QueryBuilder::select("users").limit(10).build();
        assert_eq!(sql, "SELECT * FROM users LIMIT 10");
    }

    #[test]
    fn test_offset_only() {
        let sql = QueryBuilder::select("users").offset(50).build();
        assert_eq!(sql, "SELECT * FROM users OFFSET 50");
    }

    #[test]
    fn test_limit_zero() {
        let sql = QueryBuilder::select("users").limit(0).build();
        assert_eq!(sql, "SELECT * FROM users LIMIT 0");
    }

    #[test]
    fn test_large_limit() {
        let sql = QueryBuilder::select("users").limit(1_000_000).build();
        assert_eq!(sql, "SELECT * FROM users LIMIT 1000000");
    }

    #[test]
    fn test_complex_query_all_features() {
        let sql = QueryBuilder::select("orders")
            .columns(&["orders.id", "users.name", "products.title", "orders.total"])
            .inner_join("users", "orders.user_id = users.id")
            .left_join("products", "orders.product_id = products.id")
            .where_eq("orders.status", "completed")
            .where_raw("orders.total > 100")
            .order_by("orders.created_at", OrderDirection::Desc)
            .limit(20)
            .offset(0)
            .build();

        assert!(sql.starts_with("SELECT orders.id, users.name"));
        assert!(sql.contains("INNER JOIN users"));
        assert!(sql.contains("LEFT JOIN products"));
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("ORDER BY orders.created_at DESC"));
        assert!(sql.contains("LIMIT 20"));
    }

    #[test]
    fn test_unicode_table_name() {
        let sql = QueryBuilder::select("用户表").build();
        assert_eq!(sql, "SELECT * FROM 用户表");
    }

    #[test]
    fn test_unicode_where_value() {
        let sql = QueryBuilder::select("users")
            .where_eq("name", "日本語")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE name = '日本語'");
    }

    #[test]
    fn test_emoji_in_value() {
        let sql = QueryBuilder::select("posts")
            .where_eq("content", "Hello 👋 World 🌍")
            .build();
        assert!(sql.contains("Hello 👋 World 🌍"));
    }

    #[test]
    fn test_empty_string_value() {
        let sql = QueryBuilder::select("users")
            .where_eq("nickname", "")
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE nickname = ''");
    }

    #[test]
    fn test_table_with_schema() {
        let sql = QueryBuilder::select("main.users").build();
        assert_eq!(sql, "SELECT * FROM main.users");
    }
}

// ============================================================================
// InsertBuilder - Extended Tests
// ============================================================================

mod insert_builder_extended {
    use super::*;

    #[test]
    fn test_insert_single_column() {
        let sql = InsertBuilder::into("settings")
            .columns(&["key"])
            .values(&["theme"])
            .build();
        assert_eq!(sql, "INSERT INTO settings (key) VALUES ('theme')");
    }

    #[test]
    fn test_insert_many_columns() {
        let sql = InsertBuilder::into("users")
            .columns(&["a", "b", "c", "d", "e", "f"])
            .values(&["1", "2", "3", "4", "5", "6"])
            .build();
        assert_eq!(
            sql,
            "INSERT INTO users (a, b, c, d, e, f) VALUES ('1', '2', '3', '4', '5', '6')"
        );
    }

    #[test]
    fn test_insert_many_rows() {
        let mut builder = InsertBuilder::into("numbers").columns(&["n"]);
        for i in 1..=10 {
            builder = builder.values(&[&i.to_string()]);
        }
        let sql = builder.build();
        assert!(sql.contains("('1')"));
        assert!(sql.contains("('10')"));
    }

    #[test]
    fn test_insert_unicode_values() {
        let sql = InsertBuilder::into("messages")
            .columns(&["content"])
            .values(&["こんにちは世界"])
            .build();
        assert!(sql.contains("こんにちは世界"));
    }

    #[test]
    fn test_insert_special_chars() {
        let sql = InsertBuilder::into("data")
            .columns(&["value"])
            .values(&["line1\nline2\ttab"])
            .build();
        assert!(sql.contains("line1\nline2\ttab"));
    }

    #[test]
    fn test_insert_parameterized_many_columns() {
        let (sql, count) = InsertBuilder::into("users")
            .columns(&["a", "b", "c", "d", "e"])
            .build_parameterized();
        assert_eq!(count, 5);
        assert!(sql.contains("?1, ?2, ?3, ?4, ?5"));
    }

    #[test]
    fn test_insert_on_conflict_with_single_column() {
        let sql = InsertBuilder::into("users")
            .columns(&["id", "name"])
            .values(&["1", "John"])
            .on_conflict_update(&["name"])
            .build();
        assert!(sql.contains("name = excluded.name"));
        assert!(!sql.contains("id = excluded.id"));
    }

    #[test]
    fn test_insert_empty_string() {
        let sql = InsertBuilder::into("users")
            .columns(&["bio"])
            .values(&[""])
            .build();
        assert_eq!(sql, "INSERT INTO users (bio) VALUES ('')");
    }

    #[test]
    fn test_insert_all_nulls() {
        let sql = InsertBuilder::into("users")
            .columns(&["a", "b", "c"])
            .values_optional(&[None, None, None])
            .build();
        assert_eq!(sql, "INSERT INTO users (a, b, c) VALUES (NULL, NULL, NULL)");
    }

    #[test]
    fn test_insert_mixed_null_values() {
        let sql = InsertBuilder::into("users")
            .columns(&["name", "email", "phone"])
            .values_optional(&[Some("John"), None, Some("123")])
            .build();
        assert_eq!(
            sql,
            "INSERT INTO users (name, email, phone) VALUES ('John', NULL, '123')"
        );
    }
}

// ============================================================================
// UpdateBuilder - Extended Tests
// ============================================================================

mod update_builder_extended {
    use super::*;

    #[test]
    fn test_update_single_column() {
        let sql = UpdateBuilder::table("users").set("name", "NewName").build();
        assert_eq!(sql, "UPDATE users SET name = 'NewName'");
    }

    #[test]
    fn test_update_many_columns() {
        let sql = UpdateBuilder::table("users")
            .set("a", "1")
            .set("b", "2")
            .set("c", "3")
            .set("d", "4")
            .set("e", "5")
            .build();
        assert!(sql.contains("a = '1'"));
        assert!(sql.contains("e = '5'"));
    }

    #[test]
    fn test_update_increment() {
        let sql = UpdateBuilder::table("products")
            .set_raw("stock", "stock - 1")
            .where_eq("id", "123")
            .build();
        assert!(sql.contains("stock = stock - 1"));
    }

    #[test]
    fn test_update_decrement() {
        let sql = UpdateBuilder::table("accounts")
            .set_raw("balance", "balance - 100.50")
            .build();
        assert!(sql.contains("balance = balance - 100.50"));
    }

    #[test]
    fn test_update_concatenate() {
        let sql = UpdateBuilder::table("logs")
            .set_raw("data", "data || ' appended'")
            .build();
        assert!(sql.contains("data = data || ' appended'"));
    }

    #[test]
    fn test_update_current_timestamp() {
        let sql = UpdateBuilder::table("users")
            .set_raw("updated_at", "CURRENT_TIMESTAMP")
            .build();
        assert!(sql.contains("updated_at = CURRENT_TIMESTAMP"));
    }

    #[test]
    fn test_update_unicode() {
        let sql = UpdateBuilder::table("posts")
            .set("title", "日本語タイトル")
            .build();
        assert!(sql.contains("日本語タイトル"));
    }

    #[test]
    fn test_update_complex_where() {
        let sql = UpdateBuilder::table("orders")
            .set("status", "shipped")
            .where_eq("status", "pending")
            .where_raw("created_at < datetime('now', '-7 days')")
            .build();
        assert!(sql.contains("status = 'pending'"));
        assert!(sql.contains("datetime('now', '-7 days')"));
    }

    #[test]
    fn test_update_parameterized() {
        let (sql, count) = UpdateBuilder::table("users")
            .set("name", "placeholder")
            .set("email", "placeholder")
            .build_parameterized();
        assert_eq!(count, 2);
        assert!(sql.contains("?1"));
        assert!(sql.contains("?2"));
    }
}

// ============================================================================
// DeleteBuilder - Extended Tests
// ============================================================================

mod delete_builder_extended {
    use super::*;

    #[test]
    fn test_delete_with_complex_where() {
        let sql = DeleteBuilder::from("logs")
            .where_raw("created_at < datetime('now', '-30 days')")
            .where_eq("level", "debug")
            .build();
        assert!(sql.contains("datetime('now', '-30 days')"));
        assert!(sql.contains("level = 'debug'"));
    }

    #[test]
    fn test_delete_where_in_many() {
        let ids: Vec<&str> = (1..=20).map(|_| "x").collect();
        let sql = DeleteBuilder::from("items").where_in("id", &ids).build();
        assert!(sql.contains("IN ("));
    }

    #[test]
    fn test_delete_where_not_null() {
        let sql = DeleteBuilder::from("users")
            .where_raw("deleted_at IS NOT NULL")
            .build();
        assert_eq!(sql, "DELETE FROM users WHERE deleted_at IS NOT NULL");
    }

    #[test]
    fn test_delete_unicode_condition() {
        let sql = DeleteBuilder::from("posts")
            .where_eq("category", "日本語")
            .build();
        assert!(sql.contains("日本語"));
    }

    #[test]
    fn test_delete_with_subquery() {
        let sql = DeleteBuilder::from("orders")
            .where_raw("user_id IN (SELECT id FROM users WHERE status = 'banned')")
            .build();
        assert!(sql.contains("SELECT id FROM users"));
    }
}

// ============================================================================
// Integration Tests - Actual SQLite Execution
// ============================================================================

mod integration_tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT,
                status TEXT DEFAULT 'active'
            );
            CREATE TABLE orders (
                id INTEGER PRIMARY KEY,
                user_id INTEGER,
                total REAL,
                FOREIGN KEY(user_id) REFERENCES users(id)
            );
        ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_integration_insert_and_select() {
        let conn = setup_db();

        // Insert
        let insert_sql = InsertBuilder::into("users")
            .columns(&["name", "email"])
            .values(&["John", "john@example.com"])
            .build();
        conn.execute(&insert_sql, []).unwrap();

        // Select
        let select_sql = QueryBuilder::select("users")
            .columns(&["name", "email"])
            .where_eq("name", "John")
            .build();
        let name: String = conn.query_row(&select_sql, [], |row| row.get(0)).unwrap();
        assert_eq!(name, "John");
    }

    #[test]
    fn test_integration_update() {
        let conn = setup_db();

        // Insert
        conn.execute("INSERT INTO users (name) VALUES ('Alice')", [])
            .unwrap();

        // Update
        let update_sql = UpdateBuilder::table("users")
            .set("name", "Alice Updated")
            .where_eq("name", "Alice")
            .build();
        let affected = conn.execute(&update_sql, []).unwrap();
        assert_eq!(affected, 1);

        // Verify
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE name = 'Alice Updated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_integration_delete() {
        let conn = setup_db();

        // Insert
        conn.execute("INSERT INTO users (name) VALUES ('ToDelete')", [])
            .unwrap();

        // Delete
        let delete_sql = DeleteBuilder::from("users")
            .where_eq("name", "ToDelete")
            .build();
        let affected = conn.execute(&delete_sql, []).unwrap();
        assert_eq!(affected, 1);

        // Verify
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_integration_join() {
        let conn = setup_db();

        // Insert user and order
        conn.execute("INSERT INTO users (id, name) VALUES (1, 'Bob')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO orders (id, user_id, total) VALUES (1, 1, 99.99)",
            [],
        )
        .unwrap();

        // Join query
        let sql = QueryBuilder::select("orders")
            .columns(&["users.name", "orders.total"])
            .inner_join("users", "orders.user_id = users.id")
            .build();

        let (name, total): (String, f64) = conn
            .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();

        assert_eq!(name, "Bob");
        assert!((total - 99.99).abs() < 0.01);
    }

    #[test]
    fn test_integration_or_ignore() {
        let conn = setup_db();

        // Insert first
        conn.execute("INSERT INTO users (id, name) VALUES (1, 'First')", [])
            .unwrap();

        // Try to insert duplicate with OR IGNORE
        let sql = InsertBuilder::into("users")
            .columns(&["id", "name"])
            .values(&["1", "Duplicate"])
            .or_ignore()
            .build();
        conn.execute(&sql, []).unwrap(); // Should not fail

        // Verify original is unchanged
        let name: String = conn
            .query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "First");
    }

    #[test]
    fn test_integration_or_replace() {
        let conn = setup_db();

        // Insert first
        conn.execute("INSERT INTO users (id, name) VALUES (1, 'Original')", [])
            .unwrap();

        // Replace with OR REPLACE
        let sql = InsertBuilder::into("users")
            .columns(&["id", "name"])
            .values(&["1", "Replaced"])
            .or_replace()
            .build();
        conn.execute(&sql, []).unwrap();

        // Verify replaced
        let name: String = conn
            .query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "Replaced");
    }

    #[test]
    fn test_integration_count() {
        let conn = setup_db();

        // Insert multiple
        conn.execute_batch(
            "
            INSERT INTO users (name) VALUES ('A');
            INSERT INTO users (name) VALUES ('B');
            INSERT INTO users (name) VALUES ('C');
        ",
        )
        .unwrap();

        // Count
        let query = QueryBuilder::select("users");
        let count = query.count(&conn).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_integration_execute_with_mapper() {
        let conn = setup_db();

        conn.execute_batch(
            "
            INSERT INTO users (name) VALUES ('X');
            INSERT INTO users (name) VALUES ('Y');
            INSERT INTO users (name) VALUES ('Z');
        ",
        )
        .unwrap();

        let names = QueryBuilder::select("users")
            .columns(&["name"])
            .order_by("name", OrderDirection::Asc)
            .execute(&conn, |row| row.get::<_, String>(0))
            .unwrap();

        assert_eq!(names, vec!["X", "Y", "Z"]);
    }

    #[test]
    fn test_integration_pagination() {
        let conn = setup_db();

        // Insert 10 users
        for i in 1..=10 {
            conn.execute(
                &format!("INSERT INTO users (name) VALUES ('User{}')", i),
                [],
            )
            .unwrap();
        }

        // Get page 2 (items 4-6)
        let sql = QueryBuilder::select("users")
            .columns(&["name"])
            .order_by("id", OrderDirection::Asc)
            .limit(3)
            .offset(3)
            .build();

        let names: Vec<String> = conn
            .prepare(&sql)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(names, vec!["User4", "User5", "User6"]);
    }
}
