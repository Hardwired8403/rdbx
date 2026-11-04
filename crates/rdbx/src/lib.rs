//! # RDBX — High-Performance Embedded Database Engine
//!
//! Phase 1 exposes the storage foundation: a `Database` handle with a
//! `PageManager` underneath. Higher-level features (transactions, WAL,
//! B+Tree, SQL) arrive in later milestones.

pub mod database;

pub use database::Database;
pub use rdbx_core::{
    DEFAULT_PAGE_SIZE, DatabaseOptions, Error, Lsn, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PageId, Result,
    SyncMode, TxnId,
};
pub use rdbx_storage::{PAGE_HEADER_SIZE, Page, PageManager, PageType};
