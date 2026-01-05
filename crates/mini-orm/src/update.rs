//! Type-safe UPDATE query builder
//!
//! Build UPDATE queries with compile-time validated columns and tables.

use crate::typed::{Column, ColumnRef, Expr, Table, TableId, Value};
use anyhow::Result;
use rusqlite::Connection;

// ============================================================================
// Set Clause
// ============================================================================

/// A SET assignment in an UPDATE
#[derive(Debug, Clone)]
enum SetValue {
    /// A literal value
    Literal(Value),
    /// A raw SQL expression
    Raw(String),
}

/// A column-value assignment
#[derive(Debug, Clone)]
struct SetClause {
    column: ColumnRef,
    value: SetValue,
}

// ============================================================================
// Update Builder
// ============================================================================

/// Type-safe UPDATE query builder
#[derive(Debug, Clone)]
pub struct Update {
    table: TableId,
    sets: Vec<SetClause>,
    filters: Vec<Expr>,
}

impl Update {
    /// Start an UPDATE query for a table
    pub fn table<T: Table>(_table: T) -> Self {
        Self {
            table: T::TABLE,
            sets: Vec::new(),
            filters: Vec::new(),
        }
    }

    /// Set a column to a value
    pub fn set<T, Tbl, V: Into<Value>>(mut self, col: &Column<T, Tbl>, value: V) -> Self {
        self.sets.push(SetClause {
            column: ColumnRef::from_column(col),
            value: SetValue::Literal(value.into()),
        });
        self
    }

    /// Set a column to NULL
    pub fn set_null<T, Tbl>(mut self, col: &Column<T, Tbl>) -> Self {
        self.sets.push(SetClause {
            column: ColumnRef::from_column(col),
            value: SetValue::Literal(Value::Null),
        });
        self
    }

    /// Set a column to a raw SQL expression
    pub fn set_expr<T, Tbl>(mut self, col: &Column<T, Tbl>, expr: &str) -> Self {
        self.sets.push(SetClause {
            column: ColumnRef::from_column(col),
            value: SetValue::Raw(expr.to_string()),
        });
        self
    }

    /// Add a WHERE filter
    pub fn filter(mut self, expr: Expr) -> Self {
        self.filters.push(expr);
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let sets: Vec<String> = self
            .sets
            .iter()
            .map(|s| {
                let value_sql = match &s.value {
                    SetValue::Literal(v) => v.to_sql(),
                    SetValue::Raw(expr) => expr.clone(),
                };
                format!("{} = {}", s.column.column_only(), value_sql)
            })
            .collect();

        let mut sql = format!("UPDATE {} SET {}", self.table.as_str(), sets.join(", "));

        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            let conditions: Vec<String> = self.filters.iter().map(|e| e.to_sql()).collect();
            sql.push_str(&conditions.join(" AND "));
        }

        sql
    }

    /// Execute the update
    pub fn execute(&self, conn: &Connection) -> Result<usize> {
        let sql = self.build();
        let affected = conn.execute(&sql, [])?;
        Ok(affected)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(non_upper_case_globals)]
mod tests {
    use super::*;
    use crate::typed::{ColumnId, TableId};

    struct Users;
    impl Table for Users {
        const TABLE: TableId = TableId("users");
    }

    impl Users {
        const id: Column<i32, Users> = Column::new(TableId("users"), ColumnId("id"));
        const name: Column<String, Users> = Column::new(TableId("users"), ColumnId("name"));
        const count: Column<i32, Users> = Column::new(TableId("users"), ColumnId("count"));
    }

    #[test]
    fn test_update_basic() {
        let sql = Update::table(Users)
            .set(&Users::name, "Jane")
            .filter(Users::id.equal(1))
            .build();
        assert_eq!(sql, "UPDATE users SET name = 'Jane' WHERE users.id = 1");
    }

    #[test]
    fn test_update_multiple_sets() {
        let sql = Update::table(Users)
            .set(&Users::name, "Jane")
            .set(&Users::count, 10)
            .build();
        assert_eq!(sql, "UPDATE users SET name = 'Jane', count = 10");
    }

    #[test]
    fn test_update_with_null() {
        let sql = Update::table(Users)
            .set_null(&Users::name)
            .filter(Users::id.equal(1))
            .build();
        assert_eq!(sql, "UPDATE users SET name = NULL WHERE users.id = 1");
    }

    #[test]
    fn test_update_with_expression() {
        let sql = Update::table(Users)
            .set_expr(&Users::count, "count + 1")
            .filter(Users::id.equal(1))
            .build();
        assert_eq!(sql, "UPDATE users SET count = count + 1 WHERE users.id = 1");
    }

    #[test]
    fn test_update_no_where() {
        let sql = Update::table(Users).set(&Users::name, "All").build();
        assert_eq!(sql, "UPDATE users SET name = 'All'");
    }

    #[test]
    fn test_update_multiple_filters() {
        let sql = Update::table(Users)
            .set(&Users::name, "Updated")
            .filter(Users::id.greater(10))
            .filter(Users::count.less(5))
            .build();
        assert!(sql.contains("users.id > 10 AND users.count < 5"));
    }
}
