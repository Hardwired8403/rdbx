//! SQL front-end for RDBX.
//!
//! The crate is a pure compiler: it turns `&str` into a typed AST, and
//! exposes a [`plan`] function that lowers an AST statement into a
//! [`rdbx_query::LogicalPlan`]. Execution happens in the `rdbx` crate
//! through the existing [`rdbx::Transaction`] — Phase 7 deliberately
//! does not couple the compiler to physical operators.
//!
//! See `docs/sql/SQL.md` for the supported grammar and known
//! limitations.

pub mod ast;
pub mod catalog;
pub mod error;
pub mod executor;
pub mod lexer;
pub mod parser;

pub use ast::{BinOp, ColumnDef, Constraint, DataType, Expr, SelectItem, Statement, UnaryOp};
pub use catalog::{ColumnSchema, SqlCatalog, TableSchema};
pub use error::{SqlError, SqlResult};
pub use executor::compile;
pub use lexer::{Lexer, Token, TokenKind};
pub use parser::Parser;
