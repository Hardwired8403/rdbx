//! The MVCC transaction manager.
//!
//! In Phase 5a the manager supports **one active writer at a time** (the
//! single-writer borrow model of `Database` enforces this at the type
//! level). Multiple readers can run concurrently as long as they hold
//! independent `TransactionManager` handles, which the SQL layer will
//! make possible in Phase 5b.
//!
//! The data model is nonetheless fully multi-version: every write
//! produces a new [`Version`], and any [`Snapshot`] taken before the
//! commit sees the previous version.

use std::collections::HashMap;
use std::collections::HashSet;

use rdbx_core::{Lsn, Result, TxnId};

use crate::snapshot::Snapshot;
use crate::value::{Version, VersionedValue, Visible};

/// Lifecycle of a transaction inside the manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnStatus {
    Active,
    Committed { commit_ts: Lsn },
    Aborted,
}

/// Errors reported by [`TransactionManager::commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError {
    /// The transaction tried to write a key that another committed
    /// transaction wrote after this transaction's snapshot.
    WriteConflict { key: Vec<u8> },
    /// The transaction is not active (already committed or aborted).
    NotActive,
    /// The transaction id is unknown to this manager.
    UnknownTxn,
}

/// Result of a successful commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub commit_ts: Lsn,
}

/// Handle returned by [`TransactionManager::begin`].
#[derive(Debug, Clone)]
pub struct SnapshotHandle {
    pub snapshot: Snapshot,
    pub txn_id: TxnId,
}

/// MVCC transaction manager.
pub struct TransactionManager {
    next_txn_id: u64,
    next_commit_ts: u64,
    statuses: HashMap<TxnId, TxnStatus>,
    /// Pending writes per active transaction.
    pending: HashMap<TxnId, HashMap<Vec<u8>, Option<Vec<u8>>>>,
    /// Versions per key.
    data: HashMap<Vec<u8>, VersionedValue>,
    /// Transactions currently in progress, kept for `xmin`/`xmax`.
    active_set: HashSet<TxnId>,
}

impl TransactionManager {
    /// Fresh manager with no transaction history.
    pub fn new() -> Self {
        Self {
            next_txn_id: 1,
            next_commit_ts: 1,
            statuses: HashMap::new(),
            pending: HashMap::new(),
            data: HashMap::new(),
            active_set: HashSet::new(),
        }
    }

    /// Continue transaction ids from a recovered value.
    pub fn with_next_txn_id(mut self, next: u64) -> Self {
        self.next_txn_id = next.max(1);
        self
    }

    pub fn active_count(&self) -> usize {
        self.active_set.len()
    }

    /// Start a new transaction and return its snapshot.
    ///
    /// The snapshot's `lsn` is the manager's **current commit LSN
    /// watermark**: every version with `commit_ts < lsn` was visible
    /// when the transaction began, and versions with `commit_ts >= lsn`
    /// are invisible to it. This mirrors how a real database derives a
    /// snapshot from the WAL's flushed LSN.
    pub fn begin(&mut self) -> SnapshotHandle {
        let txn_id = TxnId(self.next_txn_id);
        self.next_txn_id += 1;
        self.statuses.insert(txn_id, TxnStatus::Active);
        self.pending.insert(txn_id, HashMap::new());
        self.active_set.insert(txn_id);

        let xmin = self.active_set.iter().copied().min().unwrap_or(TxnId(0));
        let xmax = TxnId(self.next_txn_id);
        let mut active = self.active_set.clone();
        active.remove(&txn_id);
        SnapshotHandle {
            snapshot: Snapshot {
                xmax,
                xmin,
                active,
                lsn: Lsn(self.next_commit_ts),
            },
            txn_id,
        }
    }

    /// Record a write. `None` means delete.
    pub fn write(&mut self, txn_id: TxnId, key: Vec<u8>, value: Option<Vec<u8>>) -> Result<()> {
        match self.statuses.get(&txn_id) {
            Some(TxnStatus::Active) => {}
            Some(_) => {
                return Err(rdbx_core::Error::Internal(format!(
                    "txn {} is not active",
                    txn_id.0
                )));
            }
            None => {
                return Err(rdbx_core::Error::Internal(format!(
                    "unknown txn {}",
                    txn_id.0
                )));
            }
        }
        self.pending
            .get_mut(&txn_id)
            .expect("pending entry for active txn")
            .insert(key, value);
        Ok(())
    }

    /// Read a key through `snapshot`. The snapshot's own txn_id is
    /// `own_txn_id` so its uncommitted writes are visible.
    pub fn read(&self, snapshot: &Snapshot, own_txn_id: TxnId, key: &[u8]) -> Visible {
        // Own pending writes take precedence over the version history.
        if let Some(writes) = self.pending.get(&own_txn_id)
            && let Some(val) = writes.get(key)
        {
            return match val {
                Some(v) => Visible::Present(v.clone()),
                None => Visible::Deleted,
            };
        }
        match self.data.get(key) {
            Some(versions) => versions.visible_to(snapshot, own_txn_id),
            None => Visible::Absent,
        }
    }

    /// Try to commit. Detects write-write conflicts: a key is rejected
    /// when another transaction that this snapshot did **not** see has
    /// already committed a version of it.
    pub fn commit(
        &mut self,
        txn_id: TxnId,
        snapshot: &Snapshot,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        let status = self
            .statuses
            .get(&txn_id)
            .copied()
            .ok_or(CommitError::UnknownTxn)?;
        if status != TxnStatus::Active {
            return Err(CommitError::NotActive);
        }

        let writes = self.pending.get(&txn_id).cloned().unwrap_or_default();
        // Conflict check: for every key we wrote, look at every other
        // transaction's versions. A version conflicts with `snapshot`
        // when the writer was *not* visible at snapshot time:
        //
        //   visible_at_snapshot(v) = v.txn_id < snapshot.xmax
        //                            && !snapshot.active.contains(v.txn_id)
        //                            && v.commit_ts < snapshot.lsn
        //
        // Any committed version that was not visible means somebody else
        // changed this key while we were running; abort this commit.
        for key in writes.keys() {
            let Some(versions) = self.data.get(key) else {
                continue;
            };
            for v in &versions.versions {
                if v.txn_id == txn_id {
                    continue;
                }
                let visible_at_snapshot = v.txn_id.0 < snapshot.xmax.0
                    && !snapshot.active.contains(&v.txn_id)
                    && v.commit_ts.0 < snapshot.lsn.0;
                if !visible_at_snapshot {
                    return Err(CommitError::WriteConflict { key: key.clone() });
                }
            }
        }

        let commit_ts = Lsn(self.next_commit_ts);
        self.next_commit_ts += 1;

        for (key, value) in writes {
            let v = Version {
                txn_id,
                commit_ts,
                tombstone: value.is_none(),
                value: value.unwrap_or_default(),
            };
            self.data.entry(key).or_default().push(v);
        }

        self.statuses
            .insert(txn_id, TxnStatus::Committed { commit_ts });
        self.pending.remove(&txn_id);
        self.active_set.remove(&txn_id);

        Ok(CommitOutcome { commit_ts })
    }

    pub fn abort(&mut self, txn_id: TxnId) -> Result<()> {
        let status = self
            .statuses
            .get(&txn_id)
            .copied()
            .ok_or_else(|| rdbx_core::Error::Internal(format!("unknown txn {}", txn_id.0)))?;
        if status != TxnStatus::Active {
            return Err(rdbx_core::Error::Internal(format!(
                "txn {} is not active",
                txn_id.0
            )));
        }
        self.statuses.insert(txn_id, TxnStatus::Aborted);
        self.pending.remove(&txn_id);
        self.active_set.remove(&txn_id);
        Ok(())
    }

    pub fn status(&self, txn_id: TxnId) -> Option<TxnStatus> {
        self.statuses.get(&txn_id).copied()
    }

    /// Oldest active transaction id, or `TxnId(0)` when idle. Useful for
    /// future GC.
    pub fn xmin(&self) -> TxnId {
        self.active_set.iter().copied().min().unwrap_or(TxnId(0))
    }
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new()
    }
}
