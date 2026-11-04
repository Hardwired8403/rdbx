//! Phase 2 integration tests: buffer pool, catalog, and full-Database wiring.

use std::path::PathBuf;

use rdbx::{
    BufferPool, Database, DatabaseOptions, EvictionPolicyKind, PageId, PageManager, PageType,
};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-p2-{tag}-{}-{}.rdbx",
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

fn make_pool(capacity: usize, policy: EvictionPolicyKind) -> (PageManager, BufferPool, PathBuf) {
    let path = unique_path("pool");
    let opts = DatabaseOptions {
        buffer_pool_capacity: capacity,
        buffer_pool_policy: policy,
        ..Default::default()
    };
    let mgr = PageManager::create(&path, &opts).unwrap();
    let pool = BufferPool::new(capacity, mgr.page_size(), policy);
    (mgr, pool, path)
}

#[test]
fn catalog_persists_across_reopen() {
    let path = unique_path("catalog");
    let root;
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        root = db.create_table("users").unwrap();
        db.create_table("orders").unwrap();
        assert_eq!(db.tables().len(), 2);
        db.flush().unwrap();
    }
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.tables().len(), 2);
        assert_eq!(db.table("users").unwrap().root_page, root);
        assert!(db.table("orders").is_some());
        assert!(db.table("missing").is_none());
        db.drop_table("users").unwrap();
    }
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        assert_eq!(db.tables().len(), 1);
        assert!(db.table("users").is_none());
        assert!(db.table("orders").is_some());
    }
    cleanup(&path);
}

#[test]
fn create_table_rejects_duplicates() {
    let path = unique_path("dup");
    let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
    db.create_table("a").unwrap();
    assert!(db.create_table("a").is_err());
    cleanup(&path);
}

#[test]
fn buffer_pool_hit_miss_and_pin() {
    let (mut mgr, mut pool, path) = make_pool(4, EvictionPolicyKind::Lru);
    let pid = mgr.allocate_page(PageType::Data).unwrap();

    let fid = pool.fetch(&mut mgr, pid).unwrap();
    assert_eq!(pool.stats().misses, 1);
    assert_eq!(pool.stats().hits, 0);
    pool.unpin(fid).unwrap();

    let fid = pool.fetch(&mut mgr, pid).unwrap();
    assert_eq!(pool.stats().misses, 1);
    assert_eq!(pool.stats().hits, 1);
    pool.unpin(fid).unwrap();

    cleanup(&path);
}

#[test]
fn pinned_frames_are_not_evicted() {
    let (mut mgr, mut pool, path) = make_pool(2, EvictionPolicyKind::Lru);

    let p1 = pool.new_page(&mut mgr, PageType::Data).unwrap();
    let p2 = pool.new_page(&mut mgr, PageType::Data).unwrap();
    assert_eq!(pool.resident_page_count(), 2);

    let third_pid = mgr.allocate_page(PageType::Data).unwrap();
    let err = pool.fetch(&mut mgr, third_pid).err();
    assert!(err.is_some(), "expected 'all frames pinned' error");

    pool.unpin(p1).unwrap();
    let fid = pool.fetch(&mut mgr, third_pid).unwrap();
    pool.unpin(fid).unwrap();
    pool.unpin(p2).unwrap();
    drop(pool);
    drop(mgr);
    cleanup(&path);
}

#[test]
fn dirty_page_flushed_on_eviction() {
    let (mut mgr, mut pool, path) = make_pool(2, EvictionPolicyKind::Lru);
    let pid = pool.new_page(&mut mgr, PageType::Data).unwrap();
    {
        let page = pool.get_mut(pid).unwrap();
        page.payload[..5].copy_from_slice(b"hello");
    }
    pool.unpin(pid).unwrap();

    for _ in 0..3 {
        let f = pool.new_page(&mut mgr, PageType::Data).unwrap();
        pool.unpin(f).unwrap();
    }
    assert!(pool.stats().evictions >= 1);
    assert!(pool.stats().dirty_flushes >= 1);
    assert_eq!(pool.pinned_count(), 0);

    pool.flush_all(&mut mgr).unwrap();
    drop(pool);
    mgr.close().unwrap();
    drop(mgr);

    let opts = DatabaseOptions::default();
    let mut mgr = PageManager::open(&path, &opts).unwrap();
    let page = mgr.read_page(PageId(2)).unwrap();
    assert_eq!(&page.payload[..5], b"hello");
    cleanup(&path);
}

#[test]
fn lru_evicts_oldest_access() {
    let (mut mgr, mut pool, path) = make_pool(2, EvictionPolicyKind::Lru);
    let p1 = pool.new_page(&mut mgr, PageType::Data).unwrap();
    pool.unpin(p1).unwrap();
    let p2 = pool.new_page(&mut mgr, PageType::Data).unwrap();
    pool.unpin(p2).unwrap();

    // Touch page 2 (most recent now).
    let f = pool.fetch(&mut mgr, PageId(2)).unwrap();
    pool.unpin(f).unwrap();

    // Allocate a new page: should evict page 3 (least recently used).
    let p3 = pool.new_page(&mut mgr, PageType::Data).unwrap();
    pool.unpin(p3).unwrap();
    assert!(
        pool.by_page_contains(PageId(2)),
        "page 2 should still be resident"
    );
    assert!(
        !pool.by_page_contains(PageId(3)),
        "page 3 should have been evicted"
    );

    drop(pool);
    drop(mgr);
    cleanup(&path);
}

#[test]
fn clock_runs_without_panic_under_pressure() {
    let (mut mgr, mut pool, path) = make_pool(4, EvictionPolicyKind::Clock);
    for _ in 0..20 {
        let f = pool.new_page(&mut mgr, PageType::Data).unwrap();
        pool.unpin(f).unwrap();
    }
    assert!(pool.stats().evictions >= 16);
    assert!(pool.resident_page_count() <= 4);
    drop(pool);
    drop(mgr);
    cleanup(&path);
}
