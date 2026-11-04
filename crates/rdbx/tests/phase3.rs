//! Phase 3 integration tests: WAL-backed transactions end-to-end.

use std::path::{Path, PathBuf};

use rdbx::{Database, DatabaseOptions};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-p3-{tag}-{}-{}.rdbx",
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

fn cleanup_db_and_wal(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(wal_dir(path));
}

#[test]
fn commit_persists_and_reopens() {
    let path = unique_path("commit");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        db.create_table("users").unwrap();
        let pid = db.table("users").unwrap().root_page;
        let mut txn = db.begin_transaction().unwrap();
        let mut page = txn.read_page(pid).unwrap();
        page.payload[..5].copy_from_slice(b"HELLO");
        txn.write_page(pid, &page).unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let pid = db.table("users").unwrap().root_page;
        let page = db.read_page(pid).unwrap();
        assert_eq!(&page.payload[..5], b"HELLO");
    }
    cleanup_db_and_wal(&path);
}

#[test]
fn rollback_restores_original() {
    let path = unique_path("rollback");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        db.create_table("t").unwrap();
        let pid = db.table("t").unwrap().root_page;
        let mut txn = db.begin_transaction().unwrap();
        let mut page = txn.read_page(pid).unwrap();
        page.payload[..4].copy_from_slice(b"INIT");
        txn.write_page(pid, &page).unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let pid = db.table("t").unwrap().root_page;
        let mut txn = db.begin_transaction().unwrap();
        let mut page = txn.read_page(pid).unwrap();
        page.payload[..4].copy_from_slice(b"NEW!");
        txn.write_page(pid, &page).unwrap();
        txn.rollback().unwrap();
        let page = db.read_page(pid).unwrap();
        assert_eq!(&page.payload[..4], b"INIT");
    }
    cleanup_db_and_wal(&path);
}

/// True crash simulation: spawn ourselves as a child process, which opens
/// the database, writes an uncommitted page, and calls `std::process::exit(0)`.
/// `exit` does not run destructors, so no `Abort` record is written and no
/// dirty pages are flushed. WAL still holds `Begin + Update` (durable in
/// `SyncMode::Full`), and recovery must undo the update on the next open.
#[test]
fn crash_between_write_and_commit_is_undone() {
    if std::env::var("RDBX_CRASH_CHILD").is_ok() {
        crash_child_body();
        // Unreachable in practice: `crash_child_body` exits via `process::exit`.
        return;
    }

    let path = unique_path("crash_undo");
    // 1. Set the baseline: table exists, page contains "INIT", committed.
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        db.create_table("t").unwrap();
        let pid = db.table("t").unwrap().root_page;
        let mut txn = db.begin_transaction().unwrap();
        let mut page = txn.read_page(pid).unwrap();
        page.payload[..4].copy_from_slice(b"INIT");
        txn.write_page(pid, &page).unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }

    // 2. Child: begin, write "NEW!", exit without commit.
    let exe = std::env::current_exe().expect("current_exe");
    let status = std::process::Command::new(exe)
        .arg("--exact")
        .arg("crash_between_write_and_commit_is_undone")
        .arg("--nocapture")
        .env("RDBX_CRASH_CHILD", "1")
        .env("RDBX_CRASH_DB_PATH", path.as_os_str())
        .status()
        .expect("spawn child");
    assert!(status.success(), "child process failed: {status:?}");

    // 3. Parent: reopen, recovery undoes the uncommitted update.
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let pid = db.table("t").unwrap().root_page;
        let page = db.read_page(pid).unwrap();
        assert_eq!(
            &page.payload[..4],
            b"INIT",
            "recovery must undo the uncommitted write"
        );
    }
    cleanup_db_and_wal(&path);
}

fn crash_child_body() {
    let path = std::env::var_os("RDBX_CRASH_DB_PATH").expect("RDBX_CRASH_DB_PATH");
    let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
    let pid = db.table("t").unwrap().root_page;
    let mut txn = db.begin_transaction().unwrap();
    let mut page = txn.read_page(pid).unwrap();
    page.payload[..4].copy_from_slice(b"NEW!");
    txn.write_page(pid, &page).unwrap();
    // No commit, no rollback, no drop, no flush — simulate power loss.
    // The parent will reopen and expect "INIT" back.
    std::process::exit(0);
}

#[test]
fn checkpoint_truncates_wal() {
    let path = unique_path("ckpt");
    let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.create_table("t").unwrap();
    let pid = db.table("t").unwrap().root_page;
    for i in 0..4u8 {
        let mut txn = db.begin_transaction().unwrap();
        let mut page = txn.read_page(pid).unwrap();
        page.payload[0] = i;
        txn.write_page(pid, &page).unwrap();
        txn.commit().unwrap();
    }
    let wd = wal_dir(&path);
    let mut non_empty_wal = false;
    for entry in std::fs::read_dir(&wd).unwrap() {
        let e = entry.unwrap();
        if std::fs::metadata(e.path()).unwrap().len() > 64 {
            non_empty_wal = true;
            break;
        }
    }
    assert!(
        non_empty_wal,
        "WAL should contain records before checkpoint"
    );

    db.checkpoint().unwrap();
    for entry in std::fs::read_dir(&wd).unwrap() {
        let e = entry.unwrap();
        let meta = std::fs::metadata(e.path()).unwrap();
        assert!(
            meta.len() <= 64,
            "WAL segment should be header-only after checkpoint, got {} bytes",
            meta.len()
        );
    }
    drop(db);
    cleanup_db_and_wal(&path);
}
