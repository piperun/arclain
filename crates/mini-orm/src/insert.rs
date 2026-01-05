//! Type-safe INSERT query builder
//!
//! Build INSERT queries with compile-time validated columns and tables.

use crate::typed::{Column, ColumnRef, Table, TableId, Value};
use anyhow::Result;
use rusqlite::Connection;

// ============================================================================
// Conflict Resolution
// ============================================================================

/// Conflict resolution strategy for INSERT
#[derive(Debug, Clone)]
pub enum Conflict {
    /// Abort on conflict (default)
    Abort,
    /// Ignore the conflicting row
    Ignore,
    /// Replace the existing row
    Replace,
    /// Update specified columns on conflict
    Update(Vec<ColumnRef>),
}

// ============================================================================
// Column-Value Pair
// ============================================================================

/// A column-value assignment
#[derive(Debug, Clone)]
struct Assignment {
    column: ColumnRef,
    value: Value,
}

// ============================================================================
// Insert Builder
// ============================================================================

/// Type-safe INSERT query builder
#[derive(Debug, Clone)]
pub struct Insert {
    table: TableId,
    assignments: Vec<Assignment>,
    conflict: Conflict,
}

impl Insert {
    /// Start an INSERT query for a table
    pub fn into<T: Table>(_table: T) -> Self {
        Self {
            table: T::TABLE,
            assignments: Vec::new(),
            conflict: Conflict::Abort,
        }
    }

    /// Set a column value
    pub fn set<T, Tbl, V: Into<Value>>(mut self, col: &Column<T, Tbl>, value: V) -> Self {
        self.assignments.push(Assignment {
            column: ColumnRef::from_column(col),
            value: value.into(),
        });
        self
    }

    /// Set a column to NULL
    pub fn set_null<T, Tbl>(mut self, col: &Column<T, Tbl>) -> Self {
        self.assignments.push(Assignment {
            column: ColumnRef::from_column(col),
            value: Value::Null,
        });
        self
    }

    /// Use OR IGNORE on conflict
    pub fn or_ignore(mut self) -> Self {
        self.conflict = Conflict::Ignore;
        self
    }

    /// Use OR REPLACE on conflict
    pub fn or_replace(mut self) -> Self {
        self.conflict = Conflict::Replace;
        self
    }

    /// Use ON CONFLICT DO UPDATE for a specific column
    pub fn on_conflict_update_col<T, Tbl>(mut self, col: &Column<T, Tbl>) -> Self {
        match &mut self.conflict {
            Conflict::Update(ref mut cols) => {
                cols.push(ColumnRef::from_column(col));
            }
            _ => {
                self.conflict = Conflict::Update(vec![ColumnRef::from_column(col)]);
            }
        }
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let conflict_clause = match &self.conflict {
            Conflict::Abort => "",
            Conflict::Ignore => "OR IGNORE ",
            Conflict::Replace => "OR REPLACE ",
            Conflict::Update(_) => "",
        };

        let columns: Vec<String> = self
            .assignments
            .iter()
            .map(|a| a.column.column_only().to_string())
            .collect();

        let values: Vec<String> = self.assignments.iter().map(|a| a.value.to_sql()).collect();

        let mut sql = format!(
            "INSERT {}INTO {} ({}) VALUES ({})",
            conflict_clause,
            self.table.as_str(),
            columns.join(", "),
            values.join(", ")
        );

        if let Conflict::Update(cols) = &self.conflict {
            sql.push_str(" ON CONFLICT DO UPDATE SET ");
            let updates: Vec<String> = cols
                .iter()
                .map(|c| format!("{} = excluded.{}", c.column_only(), c.column_only()))
                .collect();
            sql.push_str(&updates.join(", "));
        }

        sql
    }

    /// Execute the insert
    pub fn execute(&self, conn: &Connection) -> Result<i64> {
        let sql = self.build();
        conn.execute(&sql, [])?;
        Ok(conn.last_insert_rowid())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typed::{ColumnId, TableId};

    struct Users;
    impl Table for Users {
        const TABLE: TableId = TableId("users");
    }

    impl Users {
        const ID: Column<i32, Users> = Column::new(TableId("users"), ColumnId("id"));
        const NAME: Column<String, Users> = Column::new(TableId("users"), ColumnId("name"));
        const EMAIL: Column<Option<String>, Users> =
            Column::new(TableId("users"), ColumnId("email"));
    }

    #[test]
    fn test_insert_basic() {
        let sql = Insert::into(Users)
            .set(&Users::NAME, "John")
            .set(&Users::EMAIL, "john@example.com")
            .build();
        assert_eq!(
            sql,
            "INSERT INTO users (name, email) VALUES ('John', 'john@example.com')"
        );
    }

    #[test]
    fn test_insert_with_null() {
        let sql = Insert::into(Users)
            .set(&Users::NAME, "John")
            .set_null(&Users::EMAIL)
            .build();
        assert_eq!(sql, "INSERT INTO users (name, email) VALUES ('John', NULL)");
    }

    #[test]
    fn test_insert_or_ignore() {
        let sql = Insert::into(Users)
            .set(&Users::NAME, "John")
            .or_ignore()
            .build();
        assert_eq!(sql, "INSERT OR IGNORE INTO users (name) VALUES ('John')");
    }

    #[test]
    fn test_insert_or_replace() {
        let sql = Insert::into(Users)
            .set(&Users::ID, 1)
            .set(&Users::NAME, "John")
            .or_replace()
            .build();
        assert_eq!(
            sql,
            "INSERT OR REPLACE INTO users (id, name) VALUES (1, 'John')"
        );
    }

    #[test]
    fn test_insert_on_conflict_update() {
        let sql = Insert::into(Users)
            .set(&Users::ID, 1)
            .set(&Users::NAME, "John")
            .set(&Users::EMAIL, "john@example.com")
            .on_conflict_update_col(&Users::NAME)
            .on_conflict_update_col(&Users::EMAIL)
            .build();
        assert!(sql.contains("ON CONFLICT DO UPDATE SET"));
        assert!(sql.contains("name = excluded.name"));
        assert!(sql.contains("email = excluded.email"));
    }

    #[test]
    fn test_insert_escapes_quotes() {
        let sql = Insert::into(Users).set(&Users::NAME, "O'Brien").build();
        assert_eq!(sql, "INSERT INTO users (name) VALUES ('O''Brien')");
    }
}
