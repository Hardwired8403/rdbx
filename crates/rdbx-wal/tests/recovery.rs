//! Crash recovery tests.

use std::path::PathBuf;

use rdbx_core::{DatabaseOptions, Lsn, PageId, SyncMode, TxnId};
use rdbx_storage::{PageManager, PageType};
use rdbx_wal::{RecordKind, WalManager, recover};

fn unique_dir(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-rec-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn cleanup(p: &PathBuf) {
    let _ = std::fs::remove_dir_all(p);
}

fn fresh_manager(dir: &PathBuf) -> PageManager {
    let db_path = dir.join("db.rdbx");
    let _ = std::fs::create_dir_all(dir);
    PageManager::create(&db_path, &DatabaseOptions::default()).unwrap()
}

fn patch(mgr: &mut PageManager, pid: PageId, payload: &[u8], lsn: Lsn) {
    let mut page = mgr.read_page(pid).unwrap();
    assert!(payload.len() <= page.payload.len());
    page.payload[..payload.len()].copy_from_slice(payload);
    page.header.lsn = lsn;
    mgr.write_page(&page).unwrap();
}

#[test]
fn redo_applies_committed_update() {
    let dir = unique_dir("redo");
    std::fs::create_dir_all(&dir).unwrap();
    let mut mgr = fresh_manager(&dir);
    let pid = mgr.allocate_page(PageType::Data).unwrap();
    patch(&mut mgr, pid, b"INIT", Lsn(0));
    mgr.flush().unwrap();

    let wal_dir = dir.join("wal");
    let mut wal = WalManager::open(&wal_dir, SyncMode::Full).unwrap();
    let t = TxnId(1);
    wal.append(t, RecordKind::Begin, PageId(0), vec![], vec![])
        .unwrap();
    let before = mgr.read_page(pid).unwrap().payload.clone();
    let mut after = before.clone();
    after[..5].copy_from_slice(b"AFTER");
    wal.append(t, RecordKind::Update, pid, before, after)
        .unwrap();
    wal.append(t, RecordKind::Commit, PageId(0), vec![], vec![])
        .unwrap();
    wal.flush().unwrap();

    drop(mgr);

    let db_path = dir.join("db.rdbx");
    let mut mgr = PageManager::open(&db_path, &DatabaseOptions::default()).unwrap();
    let mut wal = WalManager::open(&wal_dir, SyncMode::Full).unwrap();
    let report = recover(&mut wal, &mut mgr).unwrap();
    assert_eq!(report.committed_txns, 1);
    assert_eq!(report.uncommitted_txns, 0);
    assert!(report.pages_redone >= 1);

    let page = mgr.read_page(pid).unwrap();
    assert_eq!(&page.payload[..5], b"AFTER");
    cleanup(&dir);
}

#[test]
fn undo_discards_uncommitted_update() {
    let dir = unique_dir("undo");
    std::fs::create_dir_all(&dir).unwrap();
    let mut mgr = fresh_manager(&dir);
    let pid = mgr.allocate_page(PageType::Data).unwrap();
    patch(&mut mgr, pid, b"INIT", Lsn(0));
    mgr.flush().unwrap();

    let wal_dir = dir.join("wal");
    let mut wal = WalManager::open(&wal_dir, SyncMode::Full).unwrap();
    let t = TxnId(1);
    wal.append(t, RecordKind::Begin, PageId(0), vec![], vec![])
        .unwrap();
    let before = mgr.read_page(pid).unwrap().payload.clone();
    let mut after = before.clone();
    after[..5].copy_from_slice(b"AFTER");
    wal.append(t, RecordKind::Update, pid, before, after)
        .unwrap();
    wal.flush().unwrap();

    // Simulate a "steal": the Update reached the data file before the
    // transaction committed, so recovery must roll it back.
    let mut page = mgr.read_page(pid).unwrap();
    page.payload[..5].copy_from_slice(b"AFTER");
    page.header.lsn = Lsn(2);
    mgr.write_page(&page).unwrap();
    mgr.flush().unwrap();
    drop(mgr);

    let db_path = dir.join("db.rdbx");
    let mut mgr = PageManager::open(&db_path, &DatabaseOptions::default()).unwrap();
    let mut wal = WalManager::open(&wal_dir, SyncMode::Full).unwrap();
    let report = recover(&mut wal, &mut mgr).unwrap();
    assert_eq!(report.uncommitted_txns, 1);
    assert!(report.pages_undone >= 1);

    let page = mgr.read_page(pid).unwrap();
    assert_eq!(&page.payload[..4], b"INIT");
    cleanup(&dir);
}

/// An aborted transaction must still be undone during recovery. We cannot
/// assume that the in-place rollback performed by `Transaction::rollback`
/// reached disk before a crash, and redoing an `Update` on top of an
/// already-rolled-back page would resurrect the aborted effect without
/// this undo.
#[test]
fn aborted_txn_is_undone_in_recovery() {
    let dir = unique_dir("abort");
    std::fs::create_dir_all(&dir).unwrap();
    let mut mgr = fresh_manager(&dir);
    let pid = mgr.allocate_page(PageType::Data).unwrap();
    patch(&mut mgr, pid, b"INIT", Lsn(0));
    mgr.flush().unwrap();

    let wal_dir = dir.join("wal");
    let mut wal = WalManager::open(&wal_dir, SyncMode::Full).unwrap();
    let t = TxnId(1);
    wal.append(t, RecordKind::Begin, PageId(0), vec![], vec![])
        .unwrap();
    let before = mgr.read_page(pid).unwrap().payload.clone();
    let mut after = before.clone();
    after[..5].copy_from_slice(b"AFTER");
    wal.append(t, RecordKind::Update, pid, before, after)
        .unwrap();
    wal.append(t, RecordKind::Abort, PageId(0), vec![], vec![])
        .unwrap();
    wal.flush().unwrap();

    let report = recover(&mut wal, &mut mgr).unwrap();
    assert_eq!(report.aborted_txns, 1);
    assert_eq!(report.uncommitted_txns, 0);
    assert!(
        report.pages_undone >= 1,
        "aborted txn's update must be undone during recovery"
    );

    let page = mgr.read_page(pid).unwrap();
    assert_eq!(&page.payload[..4], b"INIT");
    cleanup(&dir);
}
