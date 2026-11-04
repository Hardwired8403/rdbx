//! # RDBX — High-Performance Embedded Database Engine
//!
//! Phase 3 exposes the WAL-backed transaction layer: a [`Database`] handle
//! with a persistent [`PageManager`], an in-memory [`BufferPool`], a schema
//! [`Catalog`], and a crash-safe [`rdbx_wal::WalManager`].
//!
//! Transactions, MVCC, B+Tree, and SQL arrive in later milestones.

pub mod database;
pub mod transaction;

pub use database::Database;
pub use rdbx_buffer::{BufferPool, BufferStats, Frame, FrameId};
pub use rdbx_core::{
    DEFAULT_BUFFER_POOL_CAPACITY, DEFAULT_PAGE_SIZE, DatabaseOptions, Error, EvictionPolicyKind,
    Lsn, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PageId, Result, SyncMode, TxnId,
};
pub use rdbx_storage::{
    Catalog, FileHeader, MAX_TABLE_NAME_LEN, PAGE_HEADER_SIZE, Page, PageManager, PageType,
    TableDescriptor,
};
pub use rdbx_wal::{RecoveryReport, RecoveryStopReason, WalManager};
pub use transaction::Transaction;
