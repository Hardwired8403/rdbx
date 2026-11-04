//! Phase 5b integration tests: persistence, multi-writer, GC.

use std::path::{Path, PathBuf};

use rdbx::{Database, DatabaseOptions, Visible};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-p5b-{tag}-{}-{}.rdbx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn wal_dir(path: &Path) -> PathBuf {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("rdbx");
    path.parent()
        .map(|parent| parent.join(format!("{file_name}.wal")))
        .unwrap_or_else(|| PathBuf::from(format!("{file_name}.wal")))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(wal_dir(path));
}

#[test]
fn mvcc_versions_persist_across_reopen() {
    let path = unique_path("persist");
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let mut txn = db.begin_transaction().unwrap();
        txn.mvcc_write(b"alice".to_vec(), Some(b"42".to_vec()))
            .unwrap();
        txn.commit().unwrap();
        db.close().unwrap();
    }
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let mut txn = db.begin_transaction().unwrap();
        assert_eq!(
            txn.mvcc_read(b"alice").unwrap(),
            Visible::Present(b"42".to_vec()),
        );
        txn.rollback().unwrap();
    }
    cleanup(&path);
}

#[test]
fn multi_writer_disjoint_keys_concurrently() {
    let path = unique_path("mw");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    {
        let mut a = db.begin_transaction().unwrap();
        let mut b = db.begin_transaction().unwrap();
        a.mvcc_write(b"x".to_vec(), Some(b"1".to_vec())).unwrap();
        b.mvcc_write(b"y".to_vec(), Some(b"2".to_vec())).unwrap();
        a.commit().unwrap();
        b.commit().unwrap();
    }
    {
        let mut r = db.begin_transaction().unwrap();
        assert_eq!(r.mvcc_read(b"x").unwrap(), Visible::Present(b"1".to_vec()));
        assert_eq!(r.mvcc_read(b"y").unwrap(), Visible::Present(b"2".to_vec()));
        r.rollback().unwrap();
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn write_conflict_rolls_back_second_writer() {
    let path = unique_path("conflict");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    let mut a = db.begin_transaction().unwrap();
    let mut b = db.begin_transaction().unwrap();
    a.mvcc_write(b"k".to_vec(), Some(b"a".to_vec())).unwrap();
    b.mvcc_write(b"k".to_vec(), Some(b"b".to_vec())).unwrap();
    a.commit().unwrap();
    let result = b.commit();
    assert!(result.is_err(), "second commit should fail");
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn gc_prunes_old_versions() {
    let path = unique_path("gc");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    for round in 0u8..5 {
        let mut txn = db.begin_transaction().unwrap();
        txn.mvcc_write(b"k".to_vec(), Some(vec![round])).unwrap();
        txn.commit().unwrap();
    }
    let removed = db.gc().unwrap();
    assert!(removed > 0, "GC should prune at least one version");
    let mut r = db.begin_transaction().unwrap();
    assert_eq!(r.mvcc_read(b"k").unwrap(), Visible::Present(vec![4]));
    r.rollback().unwrap();
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn gc_waits_for_active_transactions() {
    let path = unique_path("gc_active");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    for round in 0u8..3 {
        let mut txn = db.begin_transaction().unwrap();
        txn.mvcc_write(b"k".to_vec(), Some(vec![round])).unwrap();
        txn.commit().unwrap();
    }
    let _reader = db.begin_transaction().unwrap(); // active snapshot
    let removed = db.gc().unwrap();
    assert_eq!(removed, 0, "GC must not run while a transaction is active");
    drop(_reader);
    db.close().unwrap();
    cleanup(&path);
}
