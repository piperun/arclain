//! SQL builders for INSERT, UPDATE, and DELETE operations
//!
//! Provides fluent APIs for building mutation queries.

use anyhow::Result;
use rusqlite::{params_from_iter, Connection, ToSql};

// ============================================================================
// InsertBuilder
// ============================================================================

/// Builder for INSERT statements
#[derive(Debug, Clone)]
pub struct InsertBuilder {
    table: String,
    columns: Vec<String>,
    values: Vec<Vec<String>>,
    on_conflict: Option<OnConflict>,
}

/// Conflict resolution strategy
#[derive(Debug, Clone)]
pub enum OnConflict {
    /// Do nothing on conflict
    Ignore,
    /// Replace the existing row
    Replace,
    /// Update specific columns on conflict
    Update(Vec<String>),
}

impl InsertBuilder {
    /// Start building an INSERT query for the given table
    pub fn into(table: &str) -> Self {
        Self {
            table: table.to_string(),
            columns: Vec::new(),
            values: Vec::new(),
            on_conflict: None,
        }
    }

    /// Specify columns to insert
    pub fn columns(mut self, cols: &[&str]) -> Self {
        self.columns = cols.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add a row of values (as strings - will be quoted)
    pub fn values(mut self, vals: &[&str]) -> Self {
        self.values
            .push(vals.iter().map(|s| s.to_string()).collect());
        self
    }

    /// Add a row of values with explicit NULL handling
    pub fn values_optional(mut self, vals: &[Option<&str>]) -> Self {
        self.values.push(
            vals.iter()
                .map(|v| match v {
                    Some(s) => s.to_string(),
                    None => "NULL".to_string(),
                })
                .collect(),
        );
        self
    }

    /// Set conflict resolution to IGNORE
    pub fn or_ignore(mut self) -> Self {
        self.on_conflict = Some(OnConflict::Ignore);
        self
    }

    /// Set conflict resolution to REPLACE
    pub fn or_replace(mut self) -> Self {
        self.on_conflict = Some(OnConflict::Replace);
        self
    }

    /// Set conflict resolution to UPDATE specific columns
    pub fn on_conflict_update(mut self, cols: &[&str]) -> Self {
        self.on_conflict = Some(OnConflict::Update(
            cols.iter().map(|s| s.to_string()).collect(),
        ));
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let conflict_clause = match &self.on_conflict {
            Some(OnConflict::Ignore) => "OR IGNORE ",
            Some(OnConflict::Replace) => "OR REPLACE ",
            _ => "",
        };

        let mut sql = format!(
            "INSERT {}INTO {} ({})",
            conflict_clause,
            self.table,
            self.columns.join(", ")
        );

        if !self.values.is_empty() {
            let value_rows: Vec<String> = self
                .values
                .iter()
                .map(|row| {
                    let quoted: Vec<String> = row
                        .iter()
                        .map(|v| {
                            if v == "NULL" {
                                "NULL".to_string()
                            } else {
                                format!("'{}'", v.replace('\'', "''"))
                            }
                        })
                        .collect();
                    format!("({})", quoted.join(", "))
                })
                .collect();
            sql.push_str(" VALUES ");
            sql.push_str(&value_rows.join(", "));
        }

        // Handle ON CONFLICT ... DO UPDATE
        if let Some(OnConflict::Update(cols)) = &self.on_conflict {
            sql.push_str(" ON CONFLICT DO UPDATE SET ");
            let updates: Vec<String> = cols
                .iter()
                .map(|c| format!("{} = excluded.{}", c, c))
                .collect();
            sql.push_str(&updates.join(", "));
        }

        sql
    }

    /// Build with placeholders for parameterized execution
    pub fn build_parameterized(&self) -> (String, usize) {
        let conflict_clause = match &self.on_conflict {
            Some(OnConflict::Ignore) => "OR IGNORE ",
            Some(OnConflict::Replace) => "OR REPLACE ",
            _ => "",
        };

        let placeholders: Vec<String> = (1..=self.columns.len())
            .map(|i| format!("?{}", i))
            .collect();

        let mut sql = format!(
            "INSERT {}INTO {} ({}) VALUES ({})",
            conflict_clause,
            self.table,
            self.columns.join(", "),
            placeholders.join(", ")
        );

        if let Some(OnConflict::Update(cols)) = &self.on_conflict {
            sql.push_str(" ON CONFLICT DO UPDATE SET ");
            let updates: Vec<String> = cols
                .iter()
                .map(|c| format!("{} = excluded.{}", c, c))
                .collect();
            sql.push_str(&updates.join(", "));
        }

        (sql, self.columns.len())
    }

    /// Execute the insert with provided parameters
    pub fn execute<P>(&self, conn: &Connection, params: &[P]) -> Result<i64>
    where
        P: ToSql,
    {
        let (sql, _) = self.build_parameterized();
        conn.execute(&sql, params_from_iter(params.iter()))?;
        Ok(conn.last_insert_rowid())
    }
}

// ============================================================================
// UpdateBuilder
// ============================================================================

/// Builder for UPDATE statements
#[derive(Debug, Clone)]
pub struct UpdateBuilder {
    table: String,
    sets: Vec<(String, String)>,
    where_clauses: Vec<String>,
}

impl UpdateBuilder {
    /// Start building an UPDATE query for the given table
    pub fn table(table: &str) -> Self {
        Self {
            table: table.to_string(),
            sets: Vec::new(),
            where_clauses: Vec::new(),
        }
    }

    /// Set a column to a value (will be quoted)
    pub fn set(mut self, column: &str, value: &str) -> Self {
        self.sets.push((
            column.to_string(),
            format!("'{}'", value.replace('\'', "''")),
        ));
        self
    }

    /// Set a column to NULL
    pub fn set_null(mut self, column: &str) -> Self {
        self.sets.push((column.to_string(), "NULL".to_string()));
        self
    }

    /// Set a column to a raw SQL expression (no quoting)
    pub fn set_raw(mut self, column: &str, expr: &str) -> Self {
        self.sets.push((column.to_string(), expr.to_string()));
        self
    }

    /// Add a WHERE equality condition
    pub fn where_eq(mut self, column: &str, value: &str) -> Self {
        self.where_clauses
            .push(format!("{} = '{}'", column, value.replace('\'', "''")));
        self
    }

    /// Add a raw WHERE clause
    pub fn where_raw(mut self, condition: &str) -> Self {
        self.where_clauses.push(condition.to_string());
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let sets: Vec<String> = self
            .sets
            .iter()
            .map(|(c, v)| format!("{} = {}", c, v))
            .collect();

        let mut sql = format!("UPDATE {} SET {}", self.table, sets.join(", "));

        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }

        sql
    }

    /// Build with placeholders for parameterized execution
    pub fn build_parameterized(&self) -> (String, usize) {
        let sets: Vec<String> = self
            .sets
            .iter()
            .enumerate()
            .map(|(i, (c, _))| format!("{} = ?{}", c, i + 1))
            .collect();

        let mut sql = format!("UPDATE {} SET {}", self.table, sets.join(", "));

        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }

        (sql, self.sets.len())
    }

    /// Execute the update
    pub fn execute(&self, conn: &Connection) -> Result<usize> {
        let sql = self.build();
        let affected = conn.execute(&sql, [])?;
        Ok(affected)
    }
}

// ============================================================================
// DeleteBuilder
// ============================================================================

/// Builder for DELETE statements
#[derive(Debug, Clone)]
pub struct DeleteBuilder {
    table: String,
    where_clauses: Vec<String>,
}

impl DeleteBuilder {
    /// Start building a DELETE query for the given table
    pub fn from(table: &str) -> Self {
        Self {
            table: table.to_string(),
            where_clauses: Vec::new(),
        }
    }

    /// Add a WHERE equality condition
    pub fn where_eq(mut self, column: &str, value: &str) -> Self {
        self.where_clauses
            .push(format!("{} = '{}'", column, value.replace('\'', "''")));
        self
    }

    /// Add a raw WHERE clause
    pub fn where_raw(mut self, condition: &str) -> Self {
        self.where_clauses.push(condition.to_string());
        self
    }

    /// Add a WHERE IN clause
    pub fn where_in(mut self, column: &str, values: &[&str]) -> Self {
        let quoted: Vec<String> = values
            .iter()
            .map(|v| format!("'{}'", v.replace('\'', "''")))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", column, quoted.join(", ")));
        self
    }

    /// Add a WHERE IS NULL clause
    pub fn where_null(mut self, column: &str) -> Self {
        self.where_clauses.push(format!("{} IS NULL", column));
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let mut sql = format!("DELETE FROM {}", self.table);

        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
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
mod tests {
    use super::*;

    // --- InsertBuilder Tests ---

    #[test]
    fn test_insert_basic() {
        let sql = InsertBuilder::into("users")
            .columns(&["name", "email"])
            .values(&["John", "john@example.com"])
            .build();
        assert_eq!(
            sql,
            "INSERT INTO users (name, email) VALUES ('John', 'john@example.com')"
        );
    }

    #[test]
    fn test_insert_multiple_rows() {
        let sql = InsertBuilder::into("users")
            .columns(&["name"])
            .values(&["Alice"])
            .values(&["Bob"])
            .build();
        assert_eq!(sql, "INSERT INTO users (name) VALUES ('Alice'), ('Bob')");
    }

    #[test]
    fn test_insert_or_ignore() {
        let sql = InsertBuilder::into("users")
            .columns(&["id", "name"])
            .values(&["1", "John"])
            .or_ignore()
            .build();
        assert_eq!(
            sql,
            "INSERT OR IGNORE INTO users (id, name) VALUES ('1', 'John')"
        );
    }

    #[test]
    fn test_insert_or_replace() {
        let sql = InsertBuilder::into("users")
            .columns(&["id", "name"])
            .values(&["1", "John"])
            .or_replace()
            .build();
        assert_eq!(
            sql,
            "INSERT OR REPLACE INTO users (id, name) VALUES ('1', 'John')"
        );
    }

    #[test]
    fn test_insert_on_conflict_update() {
        let sql = InsertBuilder::into("users")
            .columns(&["id", "name", "email"])
            .values(&["1", "John", "john@example.com"])
            .on_conflict_update(&["name", "email"])
            .build();
        assert!(sql.contains("ON CONFLICT DO UPDATE SET"));
        assert!(sql.contains("name = excluded.name"));
    }

    #[test]
    fn test_insert_with_null() {
        let sql = InsertBuilder::into("users")
            .columns(&["name", "email"])
            .values_optional(&[Some("John"), None])
            .build();
        assert_eq!(sql, "INSERT INTO users (name, email) VALUES ('John', NULL)");
    }

    #[test]
    fn test_insert_escapes_quotes() {
        let sql = InsertBuilder::into("users")
            .columns(&["name"])
            .values(&["O'Brien"])
            .build();
        assert_eq!(sql, "INSERT INTO users (name) VALUES ('O''Brien')");
    }

    #[test]
    fn test_insert_parameterized() {
        let (sql, count) = InsertBuilder::into("users")
            .columns(&["name", "email"])
            .build_parameterized();
        assert_eq!(sql, "INSERT INTO users (name, email) VALUES (?1, ?2)");
        assert_eq!(count, 2);
    }

    // --- UpdateBuilder Tests ---

    #[test]
    fn test_update_basic() {
        let sql = UpdateBuilder::table("users")
            .set("name", "Jane")
            .where_eq("id", "1")
            .build();
        assert_eq!(sql, "UPDATE users SET name = 'Jane' WHERE id = '1'");
    }

    #[test]
    fn test_update_multiple_columns() {
        let sql = UpdateBuilder::table("users")
            .set("name", "Jane")
            .set("email", "jane@example.com")
            .where_eq("id", "1")
            .build();
        assert_eq!(
            sql,
            "UPDATE users SET name = 'Jane', email = 'jane@example.com' WHERE id = '1'"
        );
    }

    #[test]
    fn test_update_with_null() {
        let sql = UpdateBuilder::table("users")
            .set_null("deleted_at")
            .where_eq("id", "1")
            .build();
        assert_eq!(sql, "UPDATE users SET deleted_at = NULL WHERE id = '1'");
    }

    #[test]
    fn test_update_with_raw_expression() {
        let sql = UpdateBuilder::table("users")
            .set_raw("login_count", "login_count + 1")
            .where_eq("id", "1")
            .build();
        assert_eq!(
            sql,
            "UPDATE users SET login_count = login_count + 1 WHERE id = '1'"
        );
    }

    #[test]
    fn test_update_multiple_where() {
        let sql = UpdateBuilder::table("users")
            .set("status", "active")
            .where_eq("role", "admin")
            .where_raw("created_at < '2024-01-01'")
            .build();
        assert!(sql.contains("WHERE role = 'admin' AND created_at < '2024-01-01'"));
    }

    #[test]
    fn test_update_no_where() {
        let sql = UpdateBuilder::table("users")
            .set("status", "pending")
            .build();
        assert_eq!(sql, "UPDATE users SET status = 'pending'");
    }

    // --- DeleteBuilder Tests ---

    #[test]
    fn test_delete_basic() {
        let sql = DeleteBuilder::from("users").where_eq("id", "1").build();
        assert_eq!(sql, "DELETE FROM users WHERE id = '1'");
    }

    #[test]
    fn test_delete_multiple_where() {
        let sql = DeleteBuilder::from("users")
            .where_eq("status", "inactive")
            .where_raw("last_login < '2023-01-01'")
            .build();
        assert!(sql.contains("WHERE status = 'inactive' AND last_login < '2023-01-01'"));
    }

    #[test]
    fn test_delete_where_in() {
        let sql = DeleteBuilder::from("users")
            .where_in("id", &["1", "2", "3"])
            .build();
        assert_eq!(sql, "DELETE FROM users WHERE id IN ('1', '2', '3')");
    }

    #[test]
    fn test_delete_where_null() {
        let sql = DeleteBuilder::from("users").where_null("email").build();
        assert_eq!(sql, "DELETE FROM users WHERE email IS NULL");
    }

    #[test]
    fn test_delete_no_where() {
        let sql = DeleteBuilder::from("temp_data").build();
        assert_eq!(sql, "DELETE FROM temp_data");
    }

    #[test]
    fn test_delete_escapes_quotes() {
        let sql = DeleteBuilder::from("users")
            .where_eq("name", "O'Brien")
            .build();
        assert_eq!(sql, "DELETE FROM users WHERE name = 'O''Brien'");
    }
}
