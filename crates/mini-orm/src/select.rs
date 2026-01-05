//! Type-safe SELECT query builder
//!
//! Build SELECT queries with compile-time validated columns and tables.

use crate::typed::{Column, ColumnRef, Expr, JoinOn, Order, Table, TableId};
use anyhow::Result;
use rusqlite::{Connection, Row};

// ============================================================================
// Join Type
// ============================================================================

/// Type of JOIN operation
#[derive(Debug, Clone, Copy)]
pub enum Join {
    Inner,
    Left,
    Right,
}

impl Join {
    fn to_sql(&self) -> &'static str {
        match self {
            Join::Inner => "INNER JOIN",
            Join::Left => "LEFT JOIN",
            Join::Right => "RIGHT JOIN",
        }
    }
}

// ============================================================================
// Join Clause
// ============================================================================

/// A join clause in a SELECT query
#[derive(Debug, Clone)]
struct JoinClause {
    join_type: Join,
    table: TableId,
    on: JoinOn,
}

// ============================================================================
// Order Clause
// ============================================================================

/// An ORDER BY clause
#[derive(Debug, Clone)]
struct OrderClause {
    column: ColumnRef,
    direction: Order,
}

// ============================================================================
// Select Builder
// ============================================================================

/// Type-safe SELECT query builder
#[derive(Debug, Clone)]
pub struct Select {
    table: TableId,
    columns: Vec<ColumnRef>,
    joins: Vec<JoinClause>,
    filters: Vec<Expr>,
    order_by: Vec<OrderClause>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl Select {
    /// Start a SELECT query from a table
    pub fn from<T: Table>(_table: T) -> Self {
        Self {
            table: T::TABLE,
            columns: Vec::new(),
            joins: Vec::new(),
            filters: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }
    }

    /// Start a SELECT query from a table ID
    pub fn from_table(table: TableId) -> Self {
        Self {
            table,
            columns: Vec::new(),
            joins: Vec::new(),
            filters: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }
    }

    /// Specify columns to select
    pub fn columns<T, Tbl>(mut self, cols: &[&Column<T, Tbl>]) -> Self {
        for col in cols {
            self.columns.push(ColumnRef::from_column(*col));
        }
        self
    }

    /// Add a single column to select
    pub fn column<T, Tbl>(mut self, col: &Column<T, Tbl>) -> Self {
        self.columns.push(ColumnRef::from_column(col));
        self
    }

    /// Add an INNER JOIN
    pub fn join<T: Table>(mut self, _table: T, on: JoinOn) -> Self {
        self.joins.push(JoinClause {
            join_type: Join::Inner,
            table: T::TABLE,
            on,
        });
        self
    }

    /// Add a LEFT JOIN
    pub fn left_join<T: Table>(mut self, _table: T, on: JoinOn) -> Self {
        self.joins.push(JoinClause {
            join_type: Join::Left,
            table: T::TABLE,
            on,
        });
        self
    }

    /// Add a RIGHT JOIN
    pub fn right_join<T: Table>(mut self, _table: T, on: JoinOn) -> Self {
        self.joins.push(JoinClause {
            join_type: Join::Right,
            table: T::TABLE,
            on,
        });
        self
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

    /// Add ORDER BY clause
    pub fn order<T, Tbl>(mut self, col: &Column<T, Tbl>, dir: Order) -> Self {
        self.order_by.push(OrderClause {
            column: ColumnRef::from_column(col),
            direction: dir,
        });
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
        let mut sql = String::from("SELECT ");

        // Columns
        if self.columns.is_empty() {
            sql.push('*');
        } else {
            let cols: Vec<String> = self.columns.iter().map(|c| c.to_sql()).collect();
            sql.push_str(&cols.join(", "));
        }

        // FROM
        sql.push_str(" FROM ");
        sql.push_str(self.table.as_str());

        // JOINs
        for join in &self.joins {
            sql.push(' ');
            sql.push_str(join.join_type.to_sql());
            sql.push(' ');
            sql.push_str(join.table.as_str());
            sql.push_str(" ON ");
            sql.push_str(&join.on.to_sql());
        }

        // WHERE
        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            let conditions: Vec<String> = self.filters.iter().map(|e| e.to_sql()).collect();
            sql.push_str(&conditions.join(" AND "));
        }

        // ORDER BY
        if !self.order_by.is_empty() {
            sql.push_str(" ORDER BY ");
            let orders: Vec<String> = self
                .order_by
                .iter()
                .map(|o| format!("{} {}", o.column.to_sql(), o.direction.to_sql()))
                .collect();
            sql.push_str(&orders.join(", "));
        }

        // LIMIT
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {}", n));
        }

        // OFFSET
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
        let mut sql = String::from("SELECT COUNT(*) FROM ");
        sql.push_str(self.table.as_str());

        // JOINs
        for join in &self.joins {
            sql.push(' ');
            sql.push_str(join.join_type.to_sql());
            sql.push(' ');
            sql.push_str(join.table.as_str());
            sql.push_str(" ON ");
            sql.push_str(&join.on.to_sql());
        }

        // WHERE
        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            let conditions: Vec<String> = self.filters.iter().map(|e| e.to_sql()).collect();
            sql.push_str(&conditions.join(" AND "));
        }

        let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
        Ok(count as usize)
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

    // Mock tables
    struct Users;
    impl Table for Users {
        const TABLE: TableId = TableId("users");
    }

    impl Users {
        const id: Column<i32, Users> = Column::new(TableId("users"), ColumnId("id"));
        const name: Column<String, Users> = Column::new(TableId("users"), ColumnId("name"));
        const age: Column<i32, Users> = Column::new(TableId("users"), ColumnId("age"));
    }

    struct Orders;
    impl Table for Orders {
        const TABLE: TableId = TableId("orders");
    }

    impl Orders {
        const id: Column<i32, Orders> = Column::new(TableId("orders"), ColumnId("id"));
        const user_id: Column<i32, Orders> = Column::new(TableId("orders"), ColumnId("user_id"));
        const total: Column<i32, Orders> = Column::new(TableId("orders"), ColumnId("total"));
    }

    #[test]
    fn test_select_all() {
        let sql = Select::from(Users).build();
        assert_eq!(sql, "SELECT * FROM users");
    }

    #[test]
    fn test_select_columns() {
        let sql = Select::from(Users)
            .column(&Users::id)
            .column(&Users::name)
            .build();
        assert_eq!(sql, "SELECT users.id, users.name FROM users");
    }

    #[test]
    fn test_select_with_filter() {
        let sql = Select::from(Users)
            .filter(Users::name.equal("John"))
            .build();
        assert_eq!(sql, "SELECT * FROM users WHERE users.name = 'John'");
    }

    #[test]
    fn test_select_with_multiple_filters() {
        let sql = Select::from(Users)
            .filter(Users::name.equal("John"))
            .filter(Users::age.greater(18))
            .build();
        assert!(sql.contains("users.name = 'John' AND users.age > 18"));
    }

    #[test]
    fn test_select_with_join() {
        let sql = Select::from(Orders)
            .join(Users, Orders::user_id.equals_col(&Users::id))
            .build();
        assert_eq!(
            sql,
            "SELECT * FROM orders INNER JOIN users ON orders.user_id = users.id"
        );
    }

    #[test]
    fn test_select_with_left_join() {
        let sql = Select::from(Users)
            .left_join(Orders, Users::id.equals_col(&Orders::user_id))
            .build();
        assert!(sql.contains("LEFT JOIN orders ON users.id = orders.user_id"));
    }

    #[test]
    fn test_select_with_order() {
        let sql = Select::from(Users).order(&Users::name, Order::Asc).build();
        assert_eq!(sql, "SELECT * FROM users ORDER BY users.name ASC");
    }

    #[test]
    fn test_select_with_multiple_orders() {
        let sql = Select::from(Users)
            .order(&Users::age, Order::Desc)
            .order(&Users::name, Order::Asc)
            .build();
        assert!(sql.contains("ORDER BY users.age DESC, users.name ASC"));
    }

    #[test]
    fn test_select_with_limit() {
        let sql = Select::from(Users).limit(10).build();
        assert_eq!(sql, "SELECT * FROM users LIMIT 10");
    }

    #[test]
    fn test_select_with_offset() {
        let sql = Select::from(Users).limit(10).offset(20).build();
        assert_eq!(sql, "SELECT * FROM users LIMIT 10 OFFSET 20");
    }

    #[test]
    fn test_complex_query() {
        let sql = Select::from(Orders)
            .column(&Orders::id)
            .column(&Orders::total)
            .join(Users, Orders::user_id.equals_col(&Users::id))
            .filter(Users::name.equal("John"))
            .filter(Orders::total.greater(100))
            .order(&Orders::total, Order::Desc)
            .limit(5)
            .build();

        assert!(sql.contains("SELECT orders.id, orders.total"));
        assert!(sql.contains("INNER JOIN users"));
        assert!(sql.contains("users.name = 'John'"));
        assert!(sql.contains("orders.total > 100"));
        assert!(sql.contains("ORDER BY orders.total DESC"));
        assert!(sql.contains("LIMIT 5"));
    }
}
