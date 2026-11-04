//! The MVCC transaction manager.

use std::collections::HashMap;
use std::collections::HashSet;

use rdbx_core::{Lsn, PageId, Result, TxnId};

use crate::codec;
use crate::snapshot::Snapshot;
use crate::value::{Version, VersionedValue, Visible};

/// Persistent storage adapter for MVCC versions.
///
/// Implemented in `rdbx` by a wrapper around `BTree` + `Transaction`. The
/// trait intentionally stores raw bytes so `rdbx-mvcc` does not depend
/// on `rdbx-btree` or `rdbx-storage`.
pub trait VersionStore: Send {
    fn load(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>>;
    fn store(&mut self, key: &[u8], value: Vec<u8>) -> Result<()>;
    fn remove(&mut self, key: &[u8]) -> Result<()>;
    fn range_all(&mut self) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;
    /// If the store has an internal root that can change (e.g. B+Tree
    /// split), this returns the current value. `None` for stores whose
    /// root is fixed.
    fn current_root(&self) -> Option<PageId> {
        None
    }
}

/// In-memory fallback used by unit tests and by the `TransactionManager`
/// when no persistent store is attached.
pub struct InMemoryVersionStore {
    data: HashMap<Vec<u8>, Vec<u8>>,
}

impl InMemoryVersionStore {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }
}

impl Default for InMemoryVersionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl VersionStore for InMemoryVersionStore {
    fn load(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(self.data.get(key).cloned())
    }

    fn store(&mut self, key: &[u8], value: Vec<u8>) -> Result<()> {
        self.data.insert(key.to_vec(), value);
        Ok(())
    }

    fn remove(&mut self, key: &[u8]) -> Result<()> {
        self.data.remove(key);
        Ok(())
    }

    fn range_all(&mut self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut out: Vec<_> = self
            .data
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnStatus {
    Active,
    Committed { commit_ts: Lsn },
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError {
    WriteConflict { key: Vec<u8> },
    NotActive,
    UnknownTxn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub commit_ts: Lsn,
}

#[derive(Debug, Clone)]
pub struct SnapshotHandle {
    pub snapshot: Snapshot,
    pub txn_id: TxnId,
}

pub struct TransactionManager {
    next_txn_id: u64,
    next_commit_ts: u64,
    statuses: HashMap<TxnId, TxnStatus>,
    pending: HashMap<TxnId, HashMap<Vec<u8>, Option<Vec<u8>>>>,
    active_set: HashSet<TxnId>,
    /// Hot cache. Misses fall back to `store`.
    cache: HashMap<Vec<u8>, VersionedValue>,
    store: Box<dyn VersionStore>,
}

impl TransactionManager {
    /// New manager backed by an in-memory store (Phase 5a behaviour).
    pub fn new() -> Self {
        Self::with_store(Box::new(InMemoryVersionStore::new()))
    }

    /// New manager backed by a caller-provided store.
    pub fn with_store(store: Box<dyn VersionStore>) -> Self {
        Self {
            next_txn_id: 1,
            next_commit_ts: 1,
            statuses: HashMap::new(),
            pending: HashMap::new(),
            active_set: HashSet::new(),
            cache: HashMap::new(),
            store,
        }
    }

    pub fn with_next_txn_id(mut self, next: u64) -> Self {
        self.next_txn_id = next.max(1);
        self
    }

    /// Restore `next_commit_ts` after a restart. Must be `> max` of all
    /// persisted commit_ts values.
    pub fn with_next_commit_ts(mut self, next: u64) -> Self {
        self.next_commit_ts = next.max(1);
        self
    }

    pub fn next_commit_ts_watermark(&self) -> u64 {
        self.next_commit_ts
    }

    pub fn active_count(&self) -> usize {
        self.active_set.len()
    }

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

    pub fn read(&mut self, snapshot: &Snapshot, own_txn_id: TxnId, key: &[u8]) -> Result<Visible> {
        if let Some(writes) = self.pending.get(&own_txn_id)
            && let Some(val) = writes.get(key)
        {
            return Ok(match val {
                Some(v) => Visible::Present(v.clone()),
                None => Visible::Deleted,
            });
        }
        let versions = self.load_or_cache(key)?;
        Ok(match versions {
            Some(vv) => vv.visible_to(snapshot, own_txn_id),
            None => Visible::Absent,
        })
    }

    fn load_or_cache(&mut self, key: &[u8]) -> Result<Option<VersionedValue>> {
        if let Some(vv) = self.cache.get(key) {
            return Ok(Some(vv.clone()));
        }
        let Some(bytes) = self.store.load(key)? else {
            return Ok(None);
        };
        let vv = codec::decode(&bytes)?;
        self.cache.insert(key.to_vec(), vv.clone());
        Ok(Some(vv))
    }

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
        for key in writes.keys() {
            let versions = self
                .load_or_cache(key)
                .map_err(|_| CommitError::WriteConflict { key: key.clone() })?;
            let Some(vv) = versions else {
                continue;
            };
            for v in &vv.versions {
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
            let mut vv = self
                .load_or_cache(&key)
                .map_err(|_| CommitError::WriteConflict { key: key.clone() })?
                .unwrap_or_default();
            vv.push(Version {
                txn_id,
                commit_ts,
                tombstone: value.is_none(),
                value: value.unwrap_or_default(),
            });
            let encoded = codec::encode(&vv);
            self.store
                .store(&key, encoded)
                .map_err(|_| CommitError::WriteConflict { key: key.clone() })?;
            self.cache.insert(key, vv);
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

    pub fn xmin(&self) -> TxnId {
        self.active_set.iter().copied().min().unwrap_or(TxnId(0))
    }

    /// Garbage collect old versions. A version `v` is removable when:
    ///
    /// - `v.commit_ts < xmin` (every active snapshot would skip it), and
    /// - a newer visible version exists that a snapshot at `xmin` would
    ///   also see (either another version of the same key committed
    ///   after `v` but before `xmin`, or a tombstone).
    ///
    /// We keep at least one version per key so future snapshots do not
    /// lose the key entirely. Returns the number of versions removed.
    pub fn gc(&mut self) -> Result<usize> {
        // If any transaction is active, GC is unsafe: it might remove a
        // version visible to a snapshot we cannot see.
        if !self.active_set.is_empty() {
            return Ok(0);
        }
        let xmin_lsn = self.next_commit_ts;
        let mut removed = 0usize;

        let keys: Vec<Vec<u8>> = self
            .store
            .range_all()?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for key in keys {
            let Some(vv) = self.load_or_cache(&key)? else {
                continue;
            };
            if vv.versions.len() <= 1 {
                continue;
            }
            // Keep the newest version <= xmin and everything after it.
            // Drop strictly older versions.
            let mut kept: Vec<Version> = Vec::new();
            let mut saw_visible = false;
            for v in vv.versions.iter().rev() {
                if v.commit_ts.0 >= xmin_lsn {
                    // Post-snapshot version; keep it.
                    kept.push(v.clone());
                    continue;
                }
                if !saw_visible {
                    // First version before xmin: this is what an old
                    // snapshot would see. Keep it.
                    kept.push(v.clone());
                    saw_visible = true;
                } else {
                    // Everything older than the first pre-xmin version is
                    // invisible to any future snapshot. Drop it.
                    removed += 1;
                }
            }
            kept.reverse();
            if kept.len() < vv.versions.len() {
                let new_vv = VersionedValue { versions: kept };
                let encoded = codec::encode(&new_vv);
                self.store.store(&key, encoded)?;
                self.cache.insert(key, new_vv);
            }
        }
        Ok(removed)
    }
    /// Borrow the store for root inspection. Used by `Database::flush`.
    pub fn store(&self) -> &dyn VersionStore {
        &*self.store
    }
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new()
    }
}
