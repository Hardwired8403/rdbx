//! Query engine for RDBX.
//!
//! The engine is split into three layers:
//!
//! * [`value`] — typed scalar values and rows.
//! * [`plan`] — logical plan (what to do) and physical plan (how to do it).
//! * [`exec`] — pull-based iterators implementing each physical operator.
//!
//! [`optimizer`] turns a logical plan into a physical plan using simple
//! cost heuristics; there is no SQL, no catalog, no transactions. The
//! caller supplies a [`rdbx_btree::BTreeStore`] and a root page id, and
//! gets an iterator of [`Row`] values back.

pub mod codec;
pub mod exec;
pub mod optimizer;
pub mod plan;
pub mod value;

pub use exec::{Execution, execute};
pub use optimizer::optimize;
pub use plan::{Aggregate, BinOp, ColumnId, Expr, LogicalPlan, PhysicalPlan, SortKey};
pub use value::{Row, Value};
