//! Abstract syntax tree for the supported SQL dialect.

/// Supported column types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    Integer,
    BigInt,
    Text,
    Blob,
    Bool,
    Float,
    Double,
}

impl DataType {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "INTEGER" | "INT" => DataType::Integer,
            "BIGINT" => DataType::BigInt,
            "TEXT" | "VARCHAR" | "CHAR" => DataType::Text,
            "BLOB" => DataType::Blob,
            "BOOLEAN" | "BOOL" => DataType::Bool,
            "FLOAT" | "REAL" => DataType::Float,
            "DOUBLE" => DataType::Double,
            _ => return None,
        })
    }
}

/// Column-level constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    PrimaryKey,
    Unique,
    NotNull,
}

/// One column in `CREATE TABLE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub name: String,
    pub ty: DataType,
    pub constraints: Vec<Constraint>,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Like,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Blob(Vec<u8>),
    Column(String),
    Binary(Box<Expr>, BinOp, Box<Expr>),
    Unary(UnaryOp, Box<Expr>),
    /// Function call. Only `COUNT` is currently recognised.
    Call(String, Vec<Expr>),
}

/// A select-list item.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    /// `*`
    Wildcard,
    /// One expression, optionally aliased.
    Expr { expr: Expr, alias: Option<String> },
}

/// A single SQL statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    CreateTable {
        name: String,
        columns: Vec<ColumnDef>,
    },
    DropTable {
        name: String,
    },
    CreateIndex {
        table: String,
        columns: Vec<String>,
        name: Option<String>,
    },
    DropIndex {
        name: String,
    },
    Insert {
        table: String,
        columns: Option<Vec<String>>,
        values: Vec<Vec<Expr>>,
    },
    Select {
        items: Vec<SelectItem>,
        table: String,
        filter: Option<Expr>,
        order_by: Vec<(String, bool)>, // (column, ascending)
        limit: Option<usize>,
    },
    Update {
        table: String,
        assignments: Vec<(String, Expr)>,
        filter: Option<Expr>,
    },
    Delete {
        table: String,
        filter: Option<Expr>,
    },
    Begin,
    Commit,
    Rollback,
}
