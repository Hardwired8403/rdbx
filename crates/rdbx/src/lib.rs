//! # RDBX — High-Performance Embedded Database Engine
//!
//! Phase 5b exposes the persisted MVCC layer: versions live in a
//! dedicated B+Tree, conflict detection runs at commit, and multiple
//! transactions can coexist thanks to shared ownership of the storage
//! subsystems.

pub mod database;
mod mvcc_store;
pub mod transaction;

pub use database::Database;
pub use mvcc_store::BTreeVersionStore;
pub use rdbx_btree::{BTree, BTreeStore, InternalNode, LeafNode, Node};
pub use rdbx_buffer::{BufferPool, BufferStats, Frame, FrameId};
pub use rdbx_core::{
    DEFAULT_BUFFER_POOL_CAPACITY, DEFAULT_PAGE_SIZE, DatabaseOptions, Error, EvictionPolicyKind,
    Lsn, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PageId, Result, SyncMode, TxnId,
};
pub use rdbx_mvcc::{
    CommitError, CommitOutcome, Snapshot, SnapshotHandle, TxnStatus, Version, VersionStore,
    VersionedValue, Visible,
};
pub use rdbx_storage::{
    Catalog, FileHeader, MAX_TABLE_NAME_LEN, PAGE_HEADER_SIZE, Page, PageManager, PageType,
    TableDescriptor,
};
pub use rdbx_wal::{RecoveryReport, RecoveryStopReason, WalManager};
pub use transaction::Transaction;
