//! Type-safe schema objects for mini-orm
//!
//! These types ensure compile-time validation of table and column references.
//! All identifiers are schema objects, not strings.

use std::marker::PhantomData;

// ============================================================================
// Schema Identity Types
// ============================================================================

/// Table identifier - created by DbTable derive macro
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableId(#[doc(hidden)] pub &'static str);

impl TableId {
    /// Get the SQL table name
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Column identifier - created by DbTable derive macro
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnId(#[doc(hidden)] pub &'static str);

impl ColumnId {
    /// Get the SQL column name
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// A typed column reference with table association
#[derive(Debug, Clone, Copy)]
pub struct Column<T, Table = ()> {
    table: TableId,
    name: ColumnId,
    _marker: PhantomData<(T, Table)>,
}

impl<T, Table> Column<T, Table> {
    /// Create a new column (only called by derive macro)
    #[doc(hidden)]
    pub const fn new(table: TableId, name: ColumnId) -> Self {
        Self {
            table,
            name,
            _marker: PhantomData,
        }
    }

    /// Get the table ID
    pub fn table(&self) -> TableId {
        self.table
    }

    /// Get the column ID
    pub fn name(&self) -> ColumnId {
        self.name
    }

    /// Get fully qualified name (table.column)
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.table.as_str(), self.name.as_str())
    }

    /// Get just the column name (for use in parameterized SQL)
    pub fn col_name(&self) -> &'static str {
        self.name.as_str()
    }

    /// Get just the table name
    pub fn table_name(&self) -> &'static str {
        self.table.as_str()
    }
}

// ============================================================================
// Column Reference (for expressions)
// ============================================================================

/// A reference to a column in an expression
#[derive(Debug, Clone)]
pub struct ColumnRef {
    pub(crate) table: TableId,
    pub(crate) column: ColumnId,
}

impl ColumnRef {
    /// Create from a typed column
    pub fn from_column<T, Table>(col: &Column<T, Table>) -> Self {
        Self {
            table: col.table,
            column: col.name,
        }
    }

    /// Get the SQL representation
    pub fn to_sql(&self) -> String {
        format!("{}.{}", self.table.as_str(), self.column.as_str())
    }

    /// Get just the column name (for simple queries)
    pub fn column_only(&self) -> &str {
        self.column.as_str()
    }
}

// ============================================================================
// Values
// ============================================================================

/// A value in an expression
#[derive(Debug, Clone)]
pub enum Value {
    /// String value (will be quoted)
    Text(String),
    /// Integer value
    Int(i64),
    /// Float value
    Float(f64),
    /// Boolean value (stored as 0/1)
    Bool(bool),
    /// NULL value
    Null,
    /// Raw SQL expression (use with caution)
    Raw(String),
}

impl Value {
    /// Convert to SQL representation
    pub fn to_sql(&self) -> String {
        match self {
            Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Null => "NULL".to_string(),
            Value::Raw(s) => s.clone(),
        }
    }
}

// Conversions to Value
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}

impl From<i32> for Value {
    fn from(n: i32) -> Self {
        Value::Int(n as i64)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

// ============================================================================
// Expressions
// ============================================================================

/// SQL expression for WHERE clauses
#[derive(Debug, Clone)]
pub enum Expr {
    /// column = value
    Equal(ColumnRef, Value),
    /// column != value
    NotEq(ColumnRef, Value),
    /// column > value
    Greater(ColumnRef, Value),
    /// column < value
    Less(ColumnRef, Value),
    /// column >= value
    GreatEq(ColumnRef, Value),
    /// column <= value
    LessEq(ColumnRef, Value),
    /// column LIKE pattern
    Like(ColumnRef, Value),
    /// column IS NULL
    IsNull(ColumnRef),
    /// column IS NOT NULL
    NotNull(ColumnRef),
    /// column IN (values...)
    InList(ColumnRef, Vec<Value>),
    /// column BETWEEN low AND high
    Between(ColumnRef, Value, Value),
    /// expr AND expr
    And(Box<Expr>, Box<Expr>),
    /// expr OR expr
    Or(Box<Expr>, Box<Expr>),
    /// NOT expr
    Not(Box<Expr>),
}

impl Expr {
    /// Convert to SQL string
    pub fn to_sql(&self) -> String {
        match self {
            Expr::Equal(col, val) => format!("{} = {}", col.to_sql(), val.to_sql()),
            Expr::NotEq(col, val) => format!("{} != {}", col.to_sql(), val.to_sql()),
            Expr::Greater(col, val) => format!("{} > {}", col.to_sql(), val.to_sql()),
            Expr::Less(col, val) => format!("{} < {}", col.to_sql(), val.to_sql()),
            Expr::GreatEq(col, val) => format!("{} >= {}", col.to_sql(), val.to_sql()),
            Expr::LessEq(col, val) => format!("{} <= {}", col.to_sql(), val.to_sql()),
            Expr::Like(col, val) => format!("{} LIKE {}", col.to_sql(), val.to_sql()),
            Expr::IsNull(col) => format!("{} IS NULL", col.to_sql()),
            Expr::NotNull(col) => format!("{} IS NOT NULL", col.to_sql()),
            Expr::InList(col, vals) => {
                let list: Vec<String> = vals.iter().map(|v| v.to_sql()).collect();
                format!("{} IN ({})", col.to_sql(), list.join(", "))
            }
            Expr::Between(col, low, high) => {
                format!(
                    "{} BETWEEN {} AND {}",
                    col.to_sql(),
                    low.to_sql(),
                    high.to_sql()
                )
            }
            Expr::And(left, right) => format!("({} AND {})", left.to_sql(), right.to_sql()),
            Expr::Or(left, right) => format!("({} OR {})", left.to_sql(), right.to_sql()),
            Expr::Not(inner) => format!("NOT ({})", inner.to_sql()),
        }
    }

    /// Combine with AND
    pub fn and(self, other: Expr) -> Expr {
        Expr::And(Box::new(self), Box::new(other))
    }

    /// Combine with OR
    pub fn or(self, other: Expr) -> Expr {
        Expr::Or(Box::new(self), Box::new(other))
    }
}

// ============================================================================
// Parameterized Expressions (use ?N placeholders)
// ============================================================================

/// SQL expression using parameterized placeholders (?1, ?2, etc.)
///
/// Unlike `Expr` which inlines values, `ParamExpr` generates SQL with
/// placeholders for safe parameterized queries.
///
/// # Example
/// ```ignore
/// let expr = Users::name.eq_param(1);
/// assert_eq!(expr.to_sql(), "users.name = ?1");
/// ```
#[derive(Debug, Clone)]
pub enum ParamExpr {
    /// column = ?N
    EqParam(ColumnRef, u32),
    /// column != ?N
    NeParam(ColumnRef, u32),
    /// column > ?N
    GtParam(ColumnRef, u32),
    /// column < ?N
    LtParam(ColumnRef, u32),
    /// column >= ?N
    GeParam(ColumnRef, u32),
    /// column <= ?N
    LeParam(ColumnRef, u32),
    /// column LIKE ?N
    LikeParam(ColumnRef, u32),
    /// column IS NULL
    IsNull(ColumnRef),
    /// column IS NOT NULL
    NotNull(ColumnRef),
    /// expr AND expr
    And(Box<ParamExpr>, Box<ParamExpr>),
    /// expr OR expr
    Or(Box<ParamExpr>, Box<ParamExpr>),
}

impl ParamExpr {
    /// Convert to SQL string with placeholders
    pub fn to_sql(&self) -> String {
        match self {
            ParamExpr::EqParam(col, n) => format!("{} = ?{}", col.to_sql(), n),
            ParamExpr::NeParam(col, n) => format!("{} != ?{}", col.to_sql(), n),
            ParamExpr::GtParam(col, n) => format!("{} > ?{}", col.to_sql(), n),
            ParamExpr::LtParam(col, n) => format!("{} < ?{}", col.to_sql(), n),
            ParamExpr::GeParam(col, n) => format!("{} >= ?{}", col.to_sql(), n),
            ParamExpr::LeParam(col, n) => format!("{} <= ?{}", col.to_sql(), n),
            ParamExpr::LikeParam(col, n) => format!("{} LIKE ?{}", col.to_sql(), n),
            ParamExpr::IsNull(col) => format!("{} IS NULL", col.to_sql()),
            ParamExpr::NotNull(col) => format!("{} IS NOT NULL", col.to_sql()),
            ParamExpr::And(l, r) => format!("({} AND {})", l.to_sql(), r.to_sql()),
            ParamExpr::Or(l, r) => format!("({} OR {})", l.to_sql(), r.to_sql()),
        }
    }

    /// Combine with AND
    pub fn and(self, other: ParamExpr) -> ParamExpr {
        ParamExpr::And(Box::new(self), Box::new(other))
    }

    /// Combine with OR
    pub fn or(self, other: ParamExpr) -> ParamExpr {
        ParamExpr::Or(Box::new(self), Box::new(other))
    }
}

// ============================================================================
// Column Expression Methods
// ============================================================================

impl<T, Table> Column<T, Table> {
    /// column = value
    pub fn equal<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::Equal(ColumnRef::from_column(self), value.into())
    }

    /// column != value
    pub fn not_eq<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::NotEq(ColumnRef::from_column(self), value.into())
    }

    /// column IS NULL
    pub fn is_null(&self) -> Expr {
        Expr::IsNull(ColumnRef::from_column(self))
    }

    /// column IS NOT NULL
    pub fn not_null(&self) -> Expr {
        Expr::NotNull(ColumnRef::from_column(self))
    }

    /// column IN (values...)
    pub fn in_list<V: Into<Value>>(&self, values: Vec<V>) -> Expr {
        Expr::InList(
            ColumnRef::from_column(self),
            values.into_iter().map(|v| v.into()).collect(),
        )
    }

    // ========================================================================
    // Parameterized versions (generate ?N placeholders)
    // ========================================================================

    /// column = ?N (parameterized)
    pub fn eq_param(&self, n: u32) -> ParamExpr {
        ParamExpr::EqParam(ColumnRef::from_column(self), n)
    }

    /// column != ?N (parameterized)
    pub fn ne_param(&self, n: u32) -> ParamExpr {
        ParamExpr::NeParam(ColumnRef::from_column(self), n)
    }

    /// column IS NULL (parameterized version)
    pub fn is_null_param(&self) -> ParamExpr {
        ParamExpr::IsNull(ColumnRef::from_column(self))
    }

    /// column IS NOT NULL (parameterized version)
    pub fn not_null_param(&self) -> ParamExpr {
        ParamExpr::NotNull(ColumnRef::from_column(self))
    }

    /// column LIKE ?N (parameterized)
    pub fn like_param(&self, n: u32) -> ParamExpr {
        ParamExpr::LikeParam(ColumnRef::from_column(self), n)
    }

    /// column > ?N (parameterized)
    pub fn gt_param(&self, n: u32) -> ParamExpr {
        ParamExpr::GtParam(ColumnRef::from_column(self), n)
    }

    /// column < ?N (parameterized)
    pub fn lt_param(&self, n: u32) -> ParamExpr {
        ParamExpr::LtParam(ColumnRef::from_column(self), n)
    }

    /// column >= ?N (parameterized)
    pub fn ge_param(&self, n: u32) -> ParamExpr {
        ParamExpr::GeParam(ColumnRef::from_column(self), n)
    }

    /// column <= ?N (parameterized)
    pub fn le_param(&self, n: u32) -> ParamExpr {
        ParamExpr::LeParam(ColumnRef::from_column(self), n)
    }
}

// Numeric comparisons
impl<Table> Column<i32, Table> {
    /// column > value
    pub fn greater<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::Greater(ColumnRef::from_column(self), value.into())
    }

    /// column < value
    pub fn less<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::Less(ColumnRef::from_column(self), value.into())
    }

    /// column >= value
    pub fn great_eq<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::GreatEq(ColumnRef::from_column(self), value.into())
    }

    /// column <= value
    pub fn less_eq<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::LessEq(ColumnRef::from_column(self), value.into())
    }

    /// column BETWEEN low AND high
    pub fn between<V: Into<Value>>(&self, low: V, high: V) -> Expr {
        Expr::Between(ColumnRef::from_column(self), low.into(), high.into())
    }
}

impl<Table> Column<i64, Table> {
    pub fn greater<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::Greater(ColumnRef::from_column(self), value.into())
    }

    pub fn less<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::Less(ColumnRef::from_column(self), value.into())
    }

    pub fn great_eq<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::GreatEq(ColumnRef::from_column(self), value.into())
    }

    pub fn less_eq<V: Into<Value>>(&self, value: V) -> Expr {
        Expr::LessEq(ColumnRef::from_column(self), value.into())
    }

    pub fn between<V: Into<Value>>(&self, low: V, high: V) -> Expr {
        Expr::Between(ColumnRef::from_column(self), low.into(), high.into())
    }
}

// String-like operations
impl<Table> Column<String, Table> {
    /// column LIKE pattern
    pub fn like(&self, pattern: &str) -> Expr {
        Expr::Like(
            ColumnRef::from_column(self),
            Value::Text(pattern.to_string()),
        )
    }
}

impl<Table> Column<Option<String>, Table> {
    /// column LIKE pattern
    pub fn like(&self, pattern: &str) -> Expr {
        Expr::Like(
            ColumnRef::from_column(self),
            Value::Text(pattern.to_string()),
        )
    }
}

// ============================================================================
// Join Conditions
// ============================================================================

/// A join condition between two columns
#[derive(Debug, Clone)]
pub struct JoinOn {
    pub(crate) left: ColumnRef,
    pub(crate) right: ColumnRef,
}

impl JoinOn {
    /// Convert to SQL ON clause
    pub fn to_sql(&self) -> String {
        format!("{} = {}", self.left.to_sql(), self.right.to_sql())
    }
}

impl<T, Table> Column<T, Table> {
    /// Create a join condition: this_column = other_column
    pub fn equals_col<U, OtherTable>(&self, other: &Column<U, OtherTable>) -> JoinOn {
        JoinOn {
            left: ColumnRef::from_column(self),
            right: ColumnRef::from_column(other),
        }
    }
}

// ============================================================================
// Order Direction
// ============================================================================

/// Sort direction for ORDER BY
#[derive(Debug, Clone, Copy)]
pub enum Order {
    Asc,
    Desc,
}

impl Order {
    pub fn to_sql(&self) -> &'static str {
        match self {
            Order::Asc => "ASC",
            Order::Desc => "DESC",
        }
    }
}

// ============================================================================
// Table Trait
// ============================================================================

/// Trait for types that represent database tables
pub trait Table {
    /// The table identifier
    const TABLE: TableId;

    /// Get the table name
    fn table_name() -> &'static str {
        Self::TABLE.as_str()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(non_upper_case_globals)]
mod tests {
    use super::*;

    // Mock table for testing
    struct Users;
    impl Table for Users {
        const TABLE: TableId = TableId("users");
    }

    impl Users {
        const id: Column<i32, Users> = Column::new(TableId("users"), ColumnId("id"));
        const name: Column<String, Users> = Column::new(TableId("users"), ColumnId("name"));
        const email: Column<Option<String>, Users> =
            Column::new(TableId("users"), ColumnId("email"));
        const age: Column<i32, Users> = Column::new(TableId("users"), ColumnId("age"));
    }

    struct Orders;
    impl Table for Orders {
        const TABLE: TableId = TableId("orders");
    }

    impl Orders {
        const id: Column<i32, Orders> = Column::new(TableId("orders"), ColumnId("id"));
        const user_id: Column<i32, Orders> = Column::new(TableId("orders"), ColumnId("user_id"));
    }

    #[test]
    fn test_column_qualified_name() {
        assert_eq!(Users::id.qualified(), "users.id");
        assert_eq!(Users::name.qualified(), "users.name");
    }

    #[test]
    fn test_equal_expr() {
        let expr = Users::name.equal("John");
        assert_eq!(expr.to_sql(), "users.name = 'John'");
    }

    #[test]
    fn test_not_eq_expr() {
        let expr = Users::name.not_eq("John");
        assert_eq!(expr.to_sql(), "users.name != 'John'");
    }

    #[test]
    fn test_is_null_expr() {
        let expr = Users::email.is_null();
        assert_eq!(expr.to_sql(), "users.email IS NULL");
    }

    #[test]
    fn test_not_null_expr() {
        let expr = Users::email.not_null();
        assert_eq!(expr.to_sql(), "users.email IS NOT NULL");
    }

    #[test]
    fn test_greater_expr() {
        let expr = Users::age.greater(18);
        assert_eq!(expr.to_sql(), "users.age > 18");
    }

    #[test]
    fn test_less_expr() {
        let expr = Users::age.less(65);
        assert_eq!(expr.to_sql(), "users.age < 65");
    }

    #[test]
    fn test_between_expr() {
        let expr = Users::age.between(18, 65);
        assert_eq!(expr.to_sql(), "users.age BETWEEN 18 AND 65");
    }

    #[test]
    fn test_like_expr() {
        let expr = Users::name.like("%john%");
        assert_eq!(expr.to_sql(), "users.name LIKE '%john%'");
    }

    #[test]
    fn test_in_list_expr() {
        let expr = Users::id.in_list(vec![1, 2, 3]);
        assert_eq!(expr.to_sql(), "users.id IN (1, 2, 3)");
    }

    #[test]
    fn test_and_expr() {
        let expr = Users::name.equal("John").and(Users::age.greater(18));
        assert_eq!(expr.to_sql(), "(users.name = 'John' AND users.age > 18)");
    }

    #[test]
    fn test_or_expr() {
        let expr = Users::name.equal("John").or(Users::name.equal("Jane"));
        assert_eq!(
            expr.to_sql(),
            "(users.name = 'John' OR users.name = 'Jane')"
        );
    }

    #[test]
    fn test_join_on() {
        let join = Users::id.equals_col(&Orders::user_id);
        assert_eq!(join.to_sql(), "users.id = orders.user_id");
    }

    #[test]
    fn test_orders_id_qualified() {
        // Tests Orders::id to ensure it's used
        assert_eq!(Orders::id.qualified(), "orders.id");
    }

    #[test]
    fn test_value_escaping() {
        let expr = Users::name.equal("O'Brien");
        assert_eq!(expr.to_sql(), "users.name = 'O''Brien'");
    }

    #[test]
    fn test_value_null() {
        let val: Value = None::<String>.into();
        assert_eq!(val.to_sql(), "NULL");
    }

    #[test]
    fn test_value_bool() {
        assert_eq!(Value::Bool(true).to_sql(), "1");
        assert_eq!(Value::Bool(false).to_sql(), "0");
    }
}
