//! MVCC transaction manager for RDBX.
//!
//! Phase 5b: persisted versions, snapshot isolation, GC.

pub mod codec;
pub mod manager;
pub mod snapshot;
pub mod value;

pub use manager::{
    CommitError, CommitOutcome, InMemoryVersionStore, SnapshotHandle, TransactionManager,
    TxnStatus, VersionStore,
};
pub use snapshot::Snapshot;
pub use value::{Version, VersionedValue, Visible};
