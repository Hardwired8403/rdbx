//! Basic WAL append / flush / read / rotate / truncate tests.

use std::path::PathBuf;

use rdbx_core::{Lsn, PageId, SyncMode, TxnId};
use rdbx_wal::{RecordKind, WalManager};

fn unique_dir(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-wal-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn cleanup_dir(p: &PathBuf) {
    let _ = std::fs::remove_dir_all(p);
}

#[test]
fn append_and_read_back() {
    let dir = unique_dir("append");
    let mut wal = WalManager::open(&dir, SyncMode::Full).unwrap();
    let t = TxnId(1);
    wal.append(t, RecordKind::Begin, PageId(0), vec![], vec![])
        .unwrap();
    let l1 = wal
        .append(
            t,
            RecordKind::Update,
            PageId(2),
            b"before".to_vec(),
            b"after".to_vec(),
        )
        .unwrap();
    assert_eq!(l1, Lsn(2));
    wal.append(t, RecordKind::Commit, PageId(0), vec![], vec![])
        .unwrap();
    wal.flush().unwrap();

    let records = wal.read_all().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].kind, RecordKind::Begin);
    assert_eq!(records[1].kind, RecordKind::Update);
    assert_eq!(records[1].before, b"before");
    assert_eq!(records[1].after, b"after");
    assert_eq!(records[2].kind, RecordKind::Commit);
    cleanup_dir(&dir);
}

#[test]
fn reopen_preserves_lsn() {
    let dir = unique_dir("reopen");
    {
        let mut wal = WalManager::open(&dir, SyncMode::Full).unwrap();
        let t = TxnId(1);
        wal.append(t, RecordKind::Begin, PageId(0), vec![], vec![])
            .unwrap();
        wal.append(
            t,
            RecordKind::Update,
            PageId(2),
            b"a".to_vec(),
            b"b".to_vec(),
        )
        .unwrap();
        wal.flush().unwrap();
    }
    let mut wal = WalManager::open(&dir, SyncMode::Full).unwrap();
    let next = wal
        .append(TxnId(2), RecordKind::Begin, PageId(0), vec![], vec![])
        .unwrap();
    assert_eq!(next, Lsn(3));
    assert_eq!(wal.read_all().unwrap().len(), 3);
    cleanup_dir(&dir);
}

#[test]
fn truncate_removes_all_segments() {
    let dir = unique_dir("trunc");
    let mut wal = WalManager::open(&dir, SyncMode::Full).unwrap();
    let t = TxnId(1);
    for _ in 0..5 {
        wal.append(
            t,
            RecordKind::Update,
            PageId(2),
            b"x".to_vec(),
            b"y".to_vec(),
        )
        .unwrap();
    }
    wal.flush().unwrap();
    assert!(!wal.read_all().unwrap().is_empty());
    wal.truncate().unwrap();
    assert!(wal.read_all().unwrap().is_empty());
    cleanup_dir(&dir);
}

#[test]
fn truncated_tail_is_tolerated() {
    let dir = unique_dir("tail");
    {
        let mut wal = WalManager::open(&dir, SyncMode::Full).unwrap();
        let t = TxnId(1);
        wal.append(t, RecordKind::Begin, PageId(0), vec![], vec![])
            .unwrap();
        wal.append(
            t,
            RecordKind::Update,
            PageId(2),
            b"a".to_vec(),
            b"b".to_vec(),
        )
        .unwrap();
        wal.flush().unwrap();
    }
    // Find the single segment and chop a few bytes off its tail.
    let mut segments: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wal"))
        .collect();
    segments.sort();
    let path = &segments[0];
    let bytes = std::fs::read(path).unwrap();
    std::fs::write(path, &bytes[..bytes.len() - 3]).unwrap();

    let wal = WalManager::open(&dir, SyncMode::Full).unwrap();
    let records = wal.read_all().unwrap();
    // The Begin record survived; the truncated Update was dropped.
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, RecordKind::Begin);
    cleanup_dir(&dir);
}
