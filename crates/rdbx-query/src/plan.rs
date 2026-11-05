//! Logical and physical plans.

use rdbx_core::PageId;

use crate::value::Value;

/// Column index within a row.
pub type ColumnId = usize;

/// Sort direction for a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Sort key for `Sort`.
#[derive(Debug, Clone)]
pub struct SortKey {
    pub column: ColumnId,
    pub dir: SortDir,
}

/// Binary operations in expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Scalar expressions evaluated against a row.
#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Value),
    Column(ColumnId),
    Binary(Box<Expr>, BinOp, Box<Expr>),
}

/// Aggregate functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    Count,
    Sum(ColumnId),
    Min(ColumnId),
    Max(ColumnId),
    Avg(ColumnId),
}

/// Aggregate plan with grouping keys and aggregate outputs.
#[derive(Debug, Clone)]
pub struct AggregateSpec {
    pub kind: Aggregate,
    pub output: usize,
}

/// Logical plan.
#[derive(Debug, Clone)]
pub enum LogicalPlan {
    Scan {
        root: PageId,
    },
    IndexScan {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
    },
    Filter {
        input: Box<LogicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<LogicalPlan>,
        columns: Vec<ColumnId>,
    },
    Sort {
        input: Box<LogicalPlan>,
        keys: Vec<SortKey>,
    },
    Limit {
        input: Box<LogicalPlan>,
        n: usize,
    },
    Insert {
        root: PageId,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
    },
    Delete {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
    },
    Update {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        value: Vec<u8>,
    },
    UpdateSet {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        assignments: Vec<(usize, Expr)>,
    },
    Count {
        input: Box<LogicalPlan>,
        column: Option<usize>,
    },
    Aggregate {
        input: Box<LogicalPlan>,
        group_by: Vec<usize>,
        aggregates: Vec<AggregateSpec>,
    },
}

/// Physical plan.
#[derive(Debug, Clone)]
pub enum PhysicalPlan {
    SeqScan {
        root: PageId,
        projection: Option<Vec<ColumnId>>,
    },
    IndexScan {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        projection: Option<Vec<ColumnId>>,
    },
    Filter {
        input: Box<PhysicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<PhysicalPlan>,
        columns: Vec<ColumnId>,
    },
    Sort {
        input: Box<PhysicalPlan>,
        keys: Vec<SortKey>,
    },
    Limit {
        input: Box<PhysicalPlan>,
        n: usize,
    },
    Insert {
        root: PageId,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
    },
    Delete {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
    },
    Update {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        value: Vec<u8>,
    },
    UpdateSet {
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        assignments: Vec<(usize, Expr)>,
    },
    Count {
        input: Box<PhysicalPlan>,
        column: Option<usize>,
    },
    Aggregate {
        input: Box<PhysicalPlan>,
        group_by: Vec<usize>,
        aggregates: Vec<AggregateSpec>,
    },
}
