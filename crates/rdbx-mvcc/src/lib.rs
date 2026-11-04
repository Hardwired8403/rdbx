//! MVCC transaction manager for RDBX.
//!
//! Phase 5a implements **snapshot isolation** in memory. The layer is
//! deliberately independent from `rdbx-storage` and `rdbx-wal` so the
//! isolation model can be tested in isolation. Phase 5b will persist
//! versions through the B+Tree and add multi-writer concurrency; see
//! `docs/transactions/MVCC.md` for the exact scope.
//!
//! A [`VersionStore`] holds every committed version of every key. A
//! [`Snapshot`] fixes a set of visible transactions and a read watermark;
//! [`VersionStore::visible`] filters versions by that snapshot.

pub mod manager;
pub mod snapshot;
pub mod value;

pub use manager::{CommitError, CommitOutcome, SnapshotHandle, TransactionManager, TxnStatus};
pub use snapshot::Snapshot;
pub use value::{Version, VersionedValue, Visible};
