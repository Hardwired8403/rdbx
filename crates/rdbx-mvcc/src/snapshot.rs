//! Transaction snapshots.

use std::collections::HashSet;

use rdbx_core::{Lsn, TxnId};

/// A consistent point-in-time view of the database.
///
/// A snapshot records the set of transactions that were active when it
/// was taken. Visibility rule for a version `v`:
///
/// - `v.txn_id` must have committed, and
/// - `v.commit_ts < self.xmax`, and
/// - `v.txn_id` must not appear in `self.active`.
///
/// A transaction's own writes are always visible to itself, regardless
/// of these rules; the manager handles that by comparing `own_txn_id`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Highest txn_id that was allocated at snapshot time + 1.
    pub xmax: TxnId,
    /// Lowest txn_id still active at snapshot time (older = committed or
    /// not yet begun).
    pub xmin: TxnId,
    /// Transactions that were in progress at snapshot time.
    pub active: HashSet<TxnId>,
    /// The manager's LSN at snapshot time; informational only.
    pub lsn: Lsn,
}

impl Snapshot {
    /// Snapshot taken before any transaction began; sees the empty
    /// database.
    pub fn empty() -> Self {
        Self {
            xmax: TxnId(0),
            xmin: TxnId(0),
            active: HashSet::new(),
            lsn: Lsn(0),
        }
    }

    /// True if a version committed by `txn_id` (with `commit_ts`) is
    /// visible through this snapshot.
    pub fn sees_committed(&self, txn_id: TxnId, commit_ts: Lsn) -> bool {
        txn_id.0 < self.xmax.0 && !self.active.contains(&txn_id) && commit_ts.0 < self.lsn.0
    }
}
