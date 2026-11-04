//! Core types, errors, and configuration for RDBX.
//!
//! This crate has **no dependencies** on other `rdbx-*` crates, which keeps
//! the dependency graph acyclic and lets any layer depend on the core types.

pub mod config;
pub mod error;
pub mod types;

pub use config::{DEFAULT_PAGE_SIZE, DatabaseOptions, MAX_PAGE_SIZE, MIN_PAGE_SIZE, SyncMode};
pub use error::{Error, Result};
pub use types::{Lsn, PageId, TxnId};
