//! Phase 1 integration tests: create, open, persist, reopen, free-list.

use std::path::PathBuf;

use rdbx::{Database, DatabaseOptions, PageId, PageType};

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
}

#[test]
fn create_and_reopen_preserves_pages() {
    let path = unique_path("reopen");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.page_count(), 2); // pages 0 and 1
        let pid = db.page_manager().allocate_page(PageType::Data).unwrap();
        assert_eq!(pid, PageId(2));

        let mut page = db.page_manager().read_page(pid).unwrap();
        page.payload[..5].copy_from_slice(b"hello");
        db.page_manager().write_page(&page).unwrap();
        db.flush().unwrap();
        assert_eq!(db.page_count(), 3);
    }

    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.page_count(), 3);
        let page = db.page_manager().read_page(PageId(2)).unwrap();
        assert_eq!(&page.payload[..5], b"hello");
    }
    cleanup(&path);
}

#[test]
fn free_list_recycles_pages() {
    let path = unique_path("freelist");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let p0 = db.page_manager().allocate_page(PageType::Data).unwrap();
        let p1 = db.page_manager().allocate_page(PageType::Data).unwrap();
        let p2 = db.page_manager().allocate_page(PageType::Data).unwrap();
        assert!(p0 < p1 && p1 < p2);
        assert_eq!(db.page_count(), 5);

        db.page_manager().free_page(p1).unwrap();
        assert_eq!(db.free_page_count(), 1);

        let reused = db.page_manager().allocate_page(PageType::Index).unwrap();
        assert_eq!(reused, p1); // LIFO free list reuses most-recently freed page
        assert_eq!(db.free_page_count(), 0);
        assert_eq!(db.page_count(), 5); // no growth
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
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let pid = db.page_manager().allocate_page(PageType::Data).unwrap();
        let mut page = db.page_manager().read_page(pid).unwrap();
        page.payload[0..4].copy_from_slice(b"data");
        db.page_manager().write_page(&page).unwrap();
        db.flush().unwrap();
    }
    // Flip a byte inside the payload directly on disk.
    let page_size = Database::open(&path, DatabaseOptions::default())
        .unwrap()
        .page_size();
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = 2 * page_size + 30; // inside page 2's payload
    bytes[offset] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
    let err = db.page_manager().read_page(PageId(2)).err();
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
    {
        let mut db = Database::open(&path, opts.clone()).unwrap();
        assert_eq!(db.page_size(), 4096);
        let pid = db.page_manager().allocate_page(PageType::Data).unwrap();
        let mut page = db.page_manager().read_page(pid).unwrap();
        page.payload[..3].copy_from_slice(b"abc");
        db.page_manager().write_page(&page).unwrap();
    }
    {
        // Reopen with default options: page_size in file wins over default.
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.page_size(), 4096);
        let page = db.page_manager().read_page(PageId(2)).unwrap();
        assert_eq!(&page.payload[..3], b"abc");
    }
    cleanup(&path);
}
