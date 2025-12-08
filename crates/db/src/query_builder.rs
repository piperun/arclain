//! Generic SQL query builder for any SQLite database
//!
//! Provides a fluent API for building SELECT queries with JOINs,
//! WHERE clauses, ORDER BY, and LIMIT.

use anyhow::Result;
use rusqlite::{Connection, Row};

/// Type of JOIN operation
#[derive(Debug, Clone, Copy)]
pub enum JoinType {
    Inner,
    Left,
    Right,
}

impl JoinType {
    fn as_sql(&self) -> &'static str {
        match self {
            JoinType::Inner => "INNER JOIN",
            JoinType::Left => "LEFT JOIN",
            JoinType::Right => "RIGHT JOIN",
        }
    }
}

/// A JOIN clause in the query
#[derive(Debug, Clone)]
struct JoinClause {
    table: String,
    join_type: JoinType,
    on_condition: String,
}

/// Order direction for ORDER BY
#[derive(Debug, Clone, Copy)]
pub enum OrderDirection {
    Asc,
    Desc,
}

/// Generic query builder for SQLite SELECT statements
#[derive(Debug, Clone)]
pub struct QueryBuilder {
    base_table: String,
    select_columns: Vec<String>,
    joins: Vec<JoinClause>,
    where_clauses: Vec<String>,
    order_by: Option<(String, OrderDirection)>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl QueryBuilder {
    /// Start building a SELECT query from the given table
    pub fn select(table: &str) -> Self {
        Self {
            base_table: table.to_string(),
            select_columns: vec!["*".to_string()],
            joins: Vec::new(),
            where_clauses: Vec::new(),
            order_by: None,
            limit: None,
            offset: None,
        }
    }

    /// Specify which columns to select
    pub fn columns(mut self, cols: &[&str]) -> Self {
        self.select_columns = cols.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add a JOIN clause
    pub fn join(mut self, table: &str, on_condition: &str, join_type: JoinType) -> Self {
        self.joins.push(JoinClause {
            table: table.to_string(),
            join_type,
            on_condition: on_condition.to_string(),
        });
        self
    }

    /// Add an INNER JOIN
    pub fn inner_join(self, table: &str, on_condition: &str) -> Self {
        self.join(table, on_condition, JoinType::Inner)
    }

    /// Add a LEFT JOIN
    pub fn left_join(self, table: &str, on_condition: &str) -> Self {
        self.join(table, on_condition, JoinType::Left)
    }

    /// Add a WHERE clause with equality condition
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

    /// Add ORDER BY clause
    pub fn order_by(mut self, column: &str, direction: OrderDirection) -> Self {
        self.order_by = Some((column.to_string(), direction));
        self
    }

    /// Add LIMIT clause
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// Add OFFSET clause
    pub fn offset(mut self, n: usize) -> Self {
        self.offset = Some(n);
        self
    }

    /// Build the SQL query string
    pub fn build(&self) -> String {
        let mut sql = format!(
            "SELECT {} FROM {}",
            self.select_columns.join(", "),
            self.base_table
        );

        // Add JOINs
        for join in &self.joins {
            sql.push_str(&format!(
                " {} {} ON {}",
                join.join_type.as_sql(),
                join.table,
                join.on_condition
            ));
        }

        // Add WHERE clauses
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }

        // Add ORDER BY
        if let Some((column, direction)) = &self.order_by {
            let dir = match direction {
                OrderDirection::Asc => "ASC",
                OrderDirection::Desc => "DESC",
            };
            sql.push_str(&format!(" ORDER BY {} {}", column, dir));
        }

        // Add LIMIT
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {}", n));
        }

        // Add OFFSET
        if let Some(n) = self.offset {
            sql.push_str(&format!(" OFFSET {}", n));
        }

        sql
    }

    /// Execute the query and map results
    pub fn execute<T, F>(&self, conn: &Connection, mapper: F) -> Result<Vec<T>>
    where
        F: Fn(&Row<'_>) -> rusqlite::Result<T>,
    {
        let sql = self.build();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], mapper)?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Execute and return count
    pub fn count(&self, conn: &Connection) -> Result<usize> {
        let mut builder = self.clone();
        builder.select_columns = vec!["COUNT(*)".to_string()];
        builder.order_by = None;
        builder.limit = None;
        builder.offset = None;

        let sql = builder.build();
        let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_select() {
        let query = QueryBuilder::select("users").build();
        assert_eq!(query, "SELECT * FROM users");
    }

    #[test]
    fn test_select_with_columns() {
        let query = QueryBuilder::select("users")
            .columns(&["id", "name", "email"])
            .build();
        assert_eq!(query, "SELECT id, name, email FROM users");
    }

    #[test]
    fn test_select_with_where() {
        let query = QueryBuilder::select("users").where_eq("id", "123").build();
        assert_eq!(query, "SELECT * FROM users WHERE id = '123'");
    }

    #[test]
    fn test_select_with_join() {
        let query = QueryBuilder::select("cache_index")
            .columns(&["key", "title"])
            .left_join(
                "dlsite_metadata_cache",
                "cache_index.product_id = dlsite_metadata_cache.product_id",
            )
            .where_eq("product_id", "RJ123456")
            .build();

        assert_eq!(
            query,
            "SELECT key, title FROM cache_index LEFT JOIN dlsite_metadata_cache ON cache_index.product_id = dlsite_metadata_cache.product_id WHERE product_id = 'RJ123456'"
        );
    }

    #[test]
    fn test_select_with_order_and_limit() {
        let query = QueryBuilder::select("users")
            .order_by("created_at", OrderDirection::Desc)
            .limit(10)
            .offset(20)
            .build();
        assert_eq!(
            query,
            "SELECT * FROM users ORDER BY created_at DESC LIMIT 10 OFFSET 20"
        );
    }
}
