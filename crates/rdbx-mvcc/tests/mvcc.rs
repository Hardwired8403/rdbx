//! MVCC isolation tests.

use rdbx_core::TxnId;
use rdbx_mvcc::manager::TxnStatus;
use rdbx_mvcc::{CommitError, Snapshot, TransactionManager, Visible};

#[test]
fn read_your_own_writes() {
    let mut tm = TransactionManager::new();
    let h = tm.begin();
    tm.write(h.txn_id, b"a".to_vec(), Some(b"1".to_vec()))
        .unwrap();
    assert_eq!(
        tm.read(&h.snapshot, h.txn_id, b"a"),
        Visible::Present(b"1".to_vec())
    );
}

#[test]
fn snapshot_does_not_see_later_commits() {
    let mut tm = TransactionManager::new();
    // Reader opens a snapshot at commit_ts watermark 1.
    let reader = tm.begin();

    // Writer begins, writes, commits — commit_ts = 1.
    let writer = tm.begin();
    tm.write(writer.txn_id, b"a".to_vec(), Some(b"new".to_vec()))
        .unwrap();
    let outcome = tm.commit(writer.txn_id, &writer.snapshot).unwrap();
    assert_eq!(outcome.commit_ts.0, 1);

    // Reader at snapshot lsn = 1 must not see a commit with ts = 1.
    assert_eq!(
        tm.read(&reader.snapshot, reader.txn_id, b"a"),
        Visible::Absent,
    );

    // A new snapshot at lsn = 2 does see it.
    let later = tm.begin();
    assert_eq!(
        tm.read(&later.snapshot, later.txn_id, b"a"),
        Visible::Present(b"new".to_vec()),
    );
}

#[test]
fn delete_via_tombstone() {
    let mut tm = TransactionManager::new();
    let h = tm.begin();
    tm.write(h.txn_id, b"a".to_vec(), Some(b"1".to_vec()))
        .unwrap();
    tm.commit(h.txn_id, &h.snapshot).unwrap();

    let d = tm.begin();
    tm.write(d.txn_id, b"a".to_vec(), None).unwrap();
    tm.commit(d.txn_id, &d.snapshot).unwrap();

    let later = tm.begin();
    assert_eq!(
        tm.read(&later.snapshot, later.txn_id, b"a"),
        Visible::Deleted
    );
}

#[test]
fn write_write_conflict_detected() {
    let mut tm = TransactionManager::new();
    // Two transactions open overlapping snapshots before either commits.
    let a = tm.begin();
    let b = tm.begin();

    tm.write(a.txn_id, b"k".to_vec(), Some(b"a".to_vec()))
        .unwrap();
    tm.commit(a.txn_id, &a.snapshot).unwrap();

    tm.write(b.txn_id, b"k".to_vec(), Some(b"b".to_vec()))
        .unwrap();
    let err = tm.commit(b.txn_id, &b.snapshot).unwrap_err();
    assert!(
        matches!(err, CommitError::WriteConflict { .. }),
        "expected WriteConflict, got {err:?}"
    );
}

#[test]
fn concurrent_disjoint_writes_succeed() {
    let mut tm = TransactionManager::new();
    let a = tm.begin();
    let b = tm.begin();
    tm.write(a.txn_id, b"x".to_vec(), Some(b"1".to_vec()))
        .unwrap();
    tm.write(b.txn_id, b"y".to_vec(), Some(b"2".to_vec()))
        .unwrap();
    tm.commit(a.txn_id, &a.snapshot).unwrap();
    tm.commit(b.txn_id, &b.snapshot).unwrap();

    let later = tm.begin();
    assert_eq!(
        tm.read(&later.snapshot, later.txn_id, b"x"),
        Visible::Present(b"1".to_vec())
    );
    assert_eq!(
        tm.read(&later.snapshot, later.txn_id, b"y"),
        Visible::Present(b"2".to_vec())
    );
}

#[test]
fn aborted_txn_leaves_no_trace() {
    let mut tm = TransactionManager::new();
    let h = tm.begin();
    tm.write(h.txn_id, b"k".to_vec(), Some(b"v".to_vec()))
        .unwrap();
    tm.abort(h.txn_id).unwrap();
    assert_eq!(tm.status(h.txn_id), Some(TxnStatus::Aborted));

    let later = tm.begin();
    assert_eq!(
        tm.read(&later.snapshot, later.txn_id, b"k"),
        Visible::Absent
    );
}

#[test]
fn commit_after_abort_rejected() {
    let mut tm = TransactionManager::new();
    let h = tm.begin();
    tm.abort(h.txn_id).unwrap();
    let err = tm.commit(h.txn_id, &h.snapshot).unwrap_err();
    assert_eq!(err, CommitError::NotActive);
}

#[test]
fn unknown_txn_rejected() {
    let mut tm = TransactionManager::new();
    let snap = Snapshot::empty();
    let err = tm.commit(TxnId(999), &snap).unwrap_err();
    assert_eq!(err, CommitError::UnknownTxn);
}

#[test]
fn own_write_visible_even_when_snapshot_lsn_is_old() {
    let mut tm = TransactionManager::new();
    let h = tm.begin();
    tm.write(h.txn_id, b"k".to_vec(), Some(b"self".to_vec()))
        .unwrap();
    // A snapshot at lsn = 1 would normally not see any commit; own writes
    // bypass that check.
    assert_eq!(
        tm.read(&h.snapshot, h.txn_id, b"k"),
        Visible::Present(b"self".to_vec())
    );
}

#[test]
fn versions_accumulate_and_visibility_picks_latest() {
    let mut tm = TransactionManager::new();
    for round in 0u8..5 {
        let h = tm.begin();
        tm.write(h.txn_id, b"k".to_vec(), Some(vec![round]))
            .unwrap();
        tm.commit(h.txn_id, &h.snapshot).unwrap();
    }
    let final_snap = tm.begin();
    assert_eq!(
        tm.read(&final_snap.snapshot, final_snap.txn_id, b"k"),
        Visible::Present(vec![4])
    );
}
