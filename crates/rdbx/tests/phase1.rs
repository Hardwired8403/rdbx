//! Phase 1 integration tests: create, open, persist, reopen, free-list.
//!
//! Note: after Phase 5b the database reserves an additional page for the
//! MVCC version tree, so the first data page is not `PageId(2)` anymore.
//! Tests here avoid hard-coding page ids and instead capture the values
//! they need from the API itself.

use std::path::PathBuf;

use rdbx::{Database, DatabaseOptions, PageType};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-phase1-{tag}-{}-{}.rdbx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn cleanup(p: &PathBuf) {
    let _ = std::fs::remove_file(p);
    // Remove the sibling `.wal` directory too; Database::open creates it.
    let file_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("rdbx");
    if let Some(parent) = p.parent() {
        let _ = std::fs::remove_dir_all(parent.join(format!("{file_name}.wal")));
    }
}

#[test]
fn create_and_reopen_preserves_pages() {
    let path = unique_path("reopen");
    let baseline;
    let pid;
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        baseline = db.page_count();
        // Pages 0 (file header) and 1 (catalog) are always reserved.
        assert!(baseline >= 2, "expected at least 2 reserved pages");
        pid = db.page_manager().allocate_page(PageType::Data).unwrap();
        assert_eq!(pid.0, baseline);

        let mut page = db.page_manager().read_page(pid).unwrap();
        page.payload[..5].copy_from_slice(b"hello");
        db.page_manager().write_page(&page).unwrap();
        db.flush().unwrap();
        assert_eq!(db.page_count(), baseline + 1);
    }

    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.page_count(), baseline + 1);
        let page = db.page_manager().read_page(pid).unwrap();
        assert_eq!(&page.payload[..5], b"hello");
    }
    cleanup(&path);
}

#[test]
fn free_list_recycles_pages() {
    let path = unique_path("freelist");
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let baseline = db.page_count();
        let p0 = db.page_manager().allocate_page(PageType::Data).unwrap();
        let p1 = db.page_manager().allocate_page(PageType::Data).unwrap();
        let p2 = db.page_manager().allocate_page(PageType::Data).unwrap();
        assert!(p0 < p1 && p1 < p2);
        assert_eq!(db.page_count(), baseline + 3);

        db.page_manager().free_page(p1).unwrap();
        assert_eq!(db.free_page_count(), 1);

        let reused = db.page_manager().allocate_page(PageType::Index).unwrap();
        assert_eq!(reused, p1); // LIFO free list reuses most-recently freed page
        assert_eq!(db.free_page_count(), 0);
        assert_eq!(db.page_count(), baseline + 3); // no growth
    }
    cleanup(&path);
}

#[test]
fn reject_existing_file() {
    let path = unique_path("nodup");
    let _ = std::fs::write(&path, b"preexisting");
    let err = Database::open(&path, DatabaseOptions::default()).err();
    assert!(err.is_some());
    cleanup(&path);
}

#[test]
fn checksum_detects_corruption() {
    let path = unique_path("corrupt");
    let pid;
    let page_size;
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        pid = db.page_manager().allocate_page(PageType::Data).unwrap();
        let mut page = db.page_manager().read_page(pid).unwrap();
        page.payload[0..4].copy_from_slice(b"data");
        db.page_manager().write_page(&page).unwrap();
        db.flush().unwrap();
        page_size = db.page_size();
    }
    // Flip one byte inside page `pid`'s payload (offset past the 24-byte header).
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = (pid.0 as usize) * page_size + 24 + 6;
    bytes[offset] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    let err = db.page_manager().read_page(pid).err();
    assert!(matches!(err, Some(rdbx::Error::ChecksumMismatch { .. })));
    cleanup(&path);
}

#[test]
fn different_page_size_roundtrip() {
    let path = unique_path("smallpage");
    let opts = DatabaseOptions {
        page_size: 4096,
        ..Default::default()
    };
    let pid;
    {
        let db = Database::open(&path, opts.clone()).unwrap();
        assert_eq!(db.page_size(), 4096);
        pid = db.page_manager().allocate_page(PageType::Data).unwrap();
        let mut page = db.page_manager().read_page(pid).unwrap();
        page.payload[..3].copy_from_slice(b"abc");
        db.page_manager().write_page(&page).unwrap();
    }
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.page_size(), 4096);
        let page = db.page_manager().read_page(pid).unwrap();
        assert_eq!(&page.payload[..3], b"abc");
    }
    cleanup(&path);
}

#[test]
fn storage_returns_borrowable_pair() {
    let path = unique_path("borrow");
    let db = Database::open(&path, DatabaseOptions::default()).unwrap();
    let (mut mgr, mut pool) = db.storage();
    let fid = pool.new_page(&mut mgr, PageType::Data).unwrap();
    pool.unpin(fid).unwrap();
    drop(pool);
    drop(mgr);
    drop(db);
    cleanup(&path);
}
