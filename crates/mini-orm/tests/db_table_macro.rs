//! Integration tests for DbTable derive macro
//!
//! Tests the auto-generation of Table impl and column constants.
//! Uses unit struct syntax which:
//! - Works with Select::from(TableName)
//! - Has no dead_code warnings from unused fields

use mini_orm::{DbTable, Delete, Insert, Select, Table, Update};

// Unit struct with columns defined via attributes - recommended approach
#[derive(DbTable)]
#[table = "products"]
#[column(id: i32)]
#[column(name: String)]
#[column(price: f64)]
#[column(in_stock: bool)]
struct Products;

#[derive(DbTable)]
#[column(id: i32)]
#[column(name: String)]
#[column(product_count: i32)]
struct Categories; // Table name defaults to "categories" (snake_case)

#[test]
fn test_db_table_generates_table_impl() {
    assert_eq!(Products::table_name(), "products");
    assert_eq!(Categories::table_name(), "categories");
}

#[test]
fn test_db_table_generates_columns() {
    assert_eq!(Products::id.qualified(), "products.id");
    assert_eq!(Products::name.qualified(), "products.name");
    assert_eq!(Products::price.qualified(), "products.price");
    assert_eq!(Products::in_stock.qualified(), "products.in_stock");
}

#[test]
fn test_db_table_column_expressions() {
    let expr = Products::name.equal("Widget");
    assert_eq!(expr.to_sql(), "products.name = 'Widget'");
}

#[test]
fn test_db_table_join_between_tables() {
    let join = Products::id.equals_col(&Categories::id);
    assert_eq!(join.to_sql(), "products.id = categories.id");
}

#[test]
fn test_select_from_unit_struct() {
    let sql = Select::from(Products)
        .column(&Products::id)
        .column(&Products::name)
        .filter(Products::in_stock.equal(true))
        .build();

    assert!(sql.contains("products.id"));
    assert!(sql.contains("products.name"));
    assert!(sql.contains("products.in_stock = 1"));
}

#[test]
fn test_insert_with_unit_struct() {
    let sql = Insert::into(Products)
        .set(&Products::name, "Widget")
        .set(&Products::price, 19.99)
        .build();

    assert!(sql.contains("INSERT INTO products"));
    assert!(sql.contains("name"));
    assert!(sql.contains("price"));
}

#[test]
fn test_update_with_unit_struct() {
    let sql = Update::table(Products)
        .set(&Products::price, 29.99)
        .filter(Products::id.equal(1))
        .build();

    assert!(sql.contains("UPDATE products SET"));
    assert!(sql.contains("price = 29.99"));
    assert!(sql.contains("products.id = 1"));
}

#[test]
fn test_delete_with_unit_struct() {
    let sql = Delete::from(Products)
        .filter(Products::in_stock.equal(false))
        .build();

    assert!(sql.contains("DELETE FROM products"));
    assert!(sql.contains("products.in_stock = 0"));
}
