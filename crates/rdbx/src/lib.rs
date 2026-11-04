//! # RDBX — High-Performance Embedded Database Engine
//!
//! Phase 2 exposes the storage + buffer foundation: a [`Database`] handle
//! backed by a persistent [`PageManager`] and an in-memory [`BufferPool`],
//! with a schema catalog on page 1.
//!
//! Transactions, WAL, B+Tree, and SQL arrive in later milestones.

pub mod database;

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
