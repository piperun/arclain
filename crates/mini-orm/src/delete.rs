//! Type-safe DELETE query builder
//!
//! Build DELETE queries with compile-time validated columns and tables.

use crate::typed::{Expr, Table, TableId};
use anyhow::Result;
use rusqlite::Connection;

// ============================================================================
// Delete Builder
// ============================================================================

/// Type-safe DELETE query builder
#[derive(Debug, Clone)]
pub struct Delete {
    table: TableId,
    filters: Vec<Expr>,
}

impl Delete {
    /// Start a DELETE query for a table
    pub fn from<T: Table>(_table: T) -> Self {
        Self {
            table: T::TABLE,
            filters: Vec::new(),
        }
    }

    /// Add a WHERE filter
    pub fn filter(mut self, expr: Expr) -> Self {
        self.filters.push(expr);
        self
    }

    /// Add multiple WHERE filters (AND)
    pub fn filters(mut self, exprs: Vec<Expr>) -> Self {
        self.filters.extend(exprs);
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let mut sql = format!("DELETE FROM {}", self.table.as_str());

        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            let conditions: Vec<String> = self.filters.iter().map(|e| e.to_sql()).collect();
            sql.push_str(&conditions.join(" AND "));
        }

        sql
    }

    /// Execute the delete
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
    use crate::typed::{Column, ColumnId, TableId};

    struct Users;
    impl Table for Users {
        const TABLE: TableId = TableId("users");
    }

    impl Users {
        const id: Column<i32, Users> = Column::new(TableId("users"), ColumnId("id"));
        const name: Column<String, Users> = Column::new(TableId("users"), ColumnId("name"));
        const status: Column<String, Users> = Column::new(TableId("users"), ColumnId("status"));
    }

    #[test]
    fn test_delete_all() {
        let sql = Delete::from(Users).build();
        assert_eq!(sql, "DELETE FROM users");
    }

    #[test]
    fn test_delete_with_filter() {
        let sql = Delete::from(Users).filter(Users::id.equal(1)).build();
        assert_eq!(sql, "DELETE FROM users WHERE users.id = 1");
    }

    #[test]
    fn test_delete_multiple_filters() {
        let sql = Delete::from(Users)
            .filter(Users::status.equal("inactive"))
            .filter(Users::id.greater(10))
            .build();
        assert!(sql.contains("users.status = 'inactive' AND users.id > 10"));
    }

    #[test]
    fn test_delete_with_null_check() {
        let sql = Delete::from(Users).filter(Users::name.is_null()).build();
        assert_eq!(sql, "DELETE FROM users WHERE users.name IS NULL");
    }

    #[test]
    fn test_delete_with_in_list() {
        let sql = Delete::from(Users)
            .filter(Users::id.in_list(vec![1, 2, 3]))
            .build();
        assert_eq!(sql, "DELETE FROM users WHERE users.id IN (1, 2, 3)");
    }

    #[test]
    fn test_delete_with_or() {
        let sql = Delete::from(Users)
            .filter(
                Users::status
                    .equal("banned")
                    .or(Users::status.equal("deleted")),
            )
            .build();
        assert!(sql.contains("(users.status = 'banned' OR users.status = 'deleted')"));
    }
}
