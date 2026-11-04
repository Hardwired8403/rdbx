//! Crash recovery: analysis, redo, undo.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use rdbx_core::{Error, Lsn, Result, TxnId};
use rdbx_storage::{Page, PageManager};

use crate::manager::WalManager;
use crate::record::{RecordKind, WalRecord};
use crate::segment::SegmentFile;

/// Why recovery stopped scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStopReason {
    /// Reached end of segments, no anomalies.
    Clean,
    /// Trailing bytes formed an incomplete record.
    TruncatedTail,
    /// A record failed CRC validation.
    BadChecksum,
}

/// Diagnostic summary of a recovery pass.
#[derive(Debug, Clone)]
pub struct RecoveryReport {
    pub records_scanned: usize,
    pub committed_txns: usize,
    pub aborted_txns: usize,
    pub uncommitted_txns: usize,
    pub pages_redone: usize,
    pub pages_undone: usize,
    pub last_valid_lsn: Lsn,
    /// Highest `TxnId` observed in the WAL, or 0 when the WAL is empty.
    ///
    /// `Database::open` uses this to seed its `next_txn_id` counter so that
    /// a new process never reuses a TxnId that recovery has already
    /// classified as committed in the same WAL.
    pub max_txn_id: u64,
    pub stop_reason: RecoveryStopReason,
}

/// Run recovery against `wal`, mutating `mgr` pages in place.
///
/// Uses the ARIES-lite model without compensation log records (CLRs):
///
/// - **Committed** transactions (with a `Commit` record) are redone and
///   not undone.
/// - **Losers** — every transaction without a `Commit` record, whether
///   it has an `Abort` record or not — have their updates undone.
///
/// Undoing aborted transactions again is not redundant: the in-place
/// rollback performed by `Transaction::rollback` writes only to the
/// buffer pool, which may not have reached disk before a crash. Applying
/// a before image twice is idempotent.
pub fn recover(wal: &mut WalManager, mgr: &mut PageManager) -> Result<RecoveryReport> {
    let (records, stop_reason) = scan_records(wal)?;
    let last_valid_lsn = records.last().map(|r| r.lsn).unwrap_or(Lsn(0));
    let max_txn_id = records.iter().map(|r| r.txn_id.0).max().unwrap_or(0);

    // Analysis: classify transactions.
    //
    // - committed:   has a Commit record
    // - aborted:     has an Abort record
    // - uncommitted: has neither (crashed mid-transaction)
    // - losers:      present - committed (aborted ∪ uncommitted); their
    //                updates must be rolled back during Undo
    let mut committed: HashSet<TxnId> = HashSet::new();
    let mut aborted: HashSet<TxnId> = HashSet::new();
    let mut present: HashSet<TxnId> = HashSet::new();
    for rec in &records {
        present.insert(rec.txn_id);
        match rec.kind {
            RecordKind::Commit => {
                committed.insert(rec.txn_id);
            }
            RecordKind::Abort => {
                aborted.insert(rec.txn_id);
            }
            _ => {}
        }
    }
    let uncommitted: HashSet<TxnId> = present
        .iter()
        .filter(|t| !committed.contains(*t) && !aborted.contains(*t))
        .copied()
        .collect();
    let losers: HashSet<TxnId> = present
        .iter()
        .filter(|t| !committed.contains(*t))
        .copied()
        .collect();

    // Redo: replay every Update record in LSN order, regardless of txn
    // state. This brings the page file back to the "logged state"; any
    // effect that must not survive is removed during Undo.
    let mut pages_redone = 0;
    for rec in &records {
        if rec.kind != RecordKind::Update {
            continue;
        }
        let current = mgr.read_page(rec.page_id)?;
        if current.header.lsn.0 >= rec.lsn.0 {
            continue;
        }
        if rec.after.len() != current.payload.len() {
            return Err(Error::InvalidFile(format!(
                "WAL: after image size {} does not match page payload {}",
                rec.after.len(),
                current.payload.len()
            )));
        }
        let mut new_page = Page::new(rec.page_id, current.header.page_type, mgr.page_size());
        new_page.payload.copy_from_slice(&rec.after);
        new_page.header.lsn = rec.lsn;
        mgr.write_page(&new_page)?;
        pages_redone += 1;
    }

    // Undo: reverse-order Update records of losers (aborted + uncommitted).
    let mut pages_undone = 0;
    let mut undo_candidates: Vec<&WalRecord> = records
        .iter()
        .filter(|r| r.kind == RecordKind::Update && losers.contains(&r.txn_id))
        .collect();
    undo_candidates.sort_by_key(|r| Reverse(r.lsn.0));
    for rec in undo_candidates {
        let mut page = mgr.read_page(rec.page_id)?;
        if rec.before.len() != page.payload.len() {
            return Err(Error::InvalidFile(format!(
                "WAL: before image size {} does not match page payload {}",
                rec.before.len(),
                page.payload.len()
            )));
        }
        page.payload.copy_from_slice(&rec.before);
        page.header.lsn = Lsn(rec.lsn.0.saturating_sub(1));
        mgr.write_page(&page)?;
        pages_undone += 1;
    }
    mgr.flush()?;

    Ok(RecoveryReport {
        records_scanned: records.len(),
        committed_txns: committed.len(),
        aborted_txns: aborted.len(),
        uncommitted_txns: uncommitted.len(),
        pages_redone,
        pages_undone,
        last_valid_lsn,
        max_txn_id,
        stop_reason,
    })
}

fn scan_records(wal: &WalManager) -> Result<(Vec<WalRecord>, RecoveryStopReason)> {
    let segments = wal.segment_info()?;
    let mut records = Vec::new();
    let mut reason = RecoveryStopReason::Clean;

    for info in segments {
        let mut seg = SegmentFile::open(&info.path)?;
        let bytes = seg.read_records_bytes()?;
        let mut offset = 0usize;
        loop {
            match WalRecord::decode_from(&bytes, offset) {
                Ok(Some((rec, consumed))) => {
                    records.push(rec);
                    offset += consumed;
                }
                Ok(None) => {
                    if offset < bytes.len() {
                        reason = RecoveryStopReason::TruncatedTail;
                    }
                    break;
                }
                Err(_) => {
                    reason = RecoveryStopReason::BadChecksum;
                    break;
                }
            }
        }
        if reason != RecoveryStopReason::Clean {
            break;
        }
    }

    records.sort_by_key(|r| r.lsn.0);
    Ok((records, reason))
}

// Kept for future use (analysis side-tables); silences dead-code warnings
// in Phase 3 builds that don't reference it yet.
#[allow(dead_code)]
type _Unused = HashMap<TxnId, usize>;
