//! Versioned values.

use rdbx_core::{Lsn, TxnId};

use crate::snapshot::Snapshot;

/// One committed version of a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Transaction that wrote this version.
    pub txn_id: TxnId,
    /// LSN of the commit (monotonic).
    pub commit_ts: Lsn,
    /// `true` means the key was deleted; `value` is then empty.
    pub tombstone: bool,
    pub value: Vec<u8>,
}

/// All versions of one key. Sorted by `commit_ts` ascending.
#[derive(Debug, Clone, Default)]
pub struct VersionedValue {
    pub versions: Vec<Version>,
}

/// Result of a visibility lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visible {
    /// Key is absent in the snapshot.
    Absent,
    /// Key exists with this value.
    Present(Vec<u8>),
    /// Key was deleted before the snapshot.
    Deleted,
}

impl VersionedValue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a version. `commit_ts` must be greater than every existing
    /// version's `commit_ts`; otherwise returns the version back to the
    /// caller. Invariant is enforced by the manager, not re-verified here.
    pub fn push(&mut self, version: Version) {
        self.versions.push(version);
    }

    /// Return the version visible to `snapshot`, or `None` if the key
    /// did not exist at snapshot time.
    pub fn visible_to(&self, snapshot: &Snapshot, own_txn_id: TxnId) -> Visible {
        // Walk versions newest-first; first one visible wins.
        for v in self.versions.iter().rev() {
            let visible = v.txn_id == own_txn_id || snapshot.sees_committed(v.txn_id, v.commit_ts);
            if !visible {
                continue;
            }
            if v.tombstone {
                return Visible::Deleted;
            }
            return Visible::Present(v.value.clone());
        }
        Visible::Absent
    }

    /// Latest committed version's `commit_ts`, or `Lsn(0)` if none.
    pub fn latest_commit_ts(&self) -> Lsn {
        self.versions
            .iter()
            .map(|v| v.commit_ts)
            .max()
            .unwrap_or(Lsn(0))
    }
}
