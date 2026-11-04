//! # RDBX — High-Performance Embedded Database Engine
//!
//! Phase 4 exposes the B+Tree index on top of the WAL-backed transaction
//! layer. The `rdbx_btree` crate operates on a [`BTreeStore`], implemented
//! here by [`Transaction`] (WAL-backed writes) and by [`Database`] (direct
//! writes used by `create_table`).

pub mod database;
pub mod transaction;

pub use database::Database;
pub use rdbx_btree::{BTree, BTreeStore, InternalNode, LeafNode, Node};
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
