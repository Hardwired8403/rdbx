//! Demo: buffer pool with two policies and observable hit/miss stats.

use rdbx::{Database, DatabaseOptions, EvictionPolicyKind, PageId, PageType};

const MSG: &[u8] = b"Hello, buffer pool!";

fn main() -> rdbx::Result<()> {
    for policy in [EvictionPolicyKind::Lru, EvictionPolicyKind::Clock] {
        let path = std::env::temp_dir().join(format!("rdbx-bp-{policy:?}.rdbx"));
        let _ = std::fs::remove_file(&path);
        let opts = DatabaseOptions {
            buffer_pool_capacity: 4,
            buffer_pool_policy: policy,
            ..Default::default()
        };
        let mut db = Database::open(&path, opts)?;
        println!("policy = {:?}", db.storage().1.policy_name());

        // Write into a page, flush, evict, fetch back.
        let pid;
        {
            let (mgr, pool) = db.storage();
            let fid = pool.new_page(mgr, PageType::Data)?;
            let page = pool.get_mut(fid)?;
            page.payload[..MSG.len()].copy_from_slice(MSG);
            pid = page.header.page_id;
            pool.unpin(fid)?;
        }

        // Fill the pool to force eviction of `pid`.
        {
            let (mgr, pool) = db.storage();
            for _ in 0..6 {
                let f = pool.new_page(mgr, PageType::Data)?;
                pool.unpin(f)?;
            }
        }

        // Fetch it back (should miss, read from disk).
        {
            let (mgr, pool) = db.storage();
            let fid = pool.fetch(mgr, pid)?;
            let page = pool.get(fid)?;
            let text = std::str::from_utf8(&page.payload[..MSG.len()])
                .map_err(|e| rdbx::Error::Internal(format!("bad UTF-8: {e}")))?;
            println!("  read back: {text}");
            pool.unpin(fid)?;
            let stats = pool.stats();
            println!(
                "  hits={} misses={} evictions={} dirty_flushes={} hit_ratio={:.2}",
                stats.hits,
                stats.misses,
                stats.evictions,
                stats.dirty_flushes,
                stats.hit_ratio()
            );
        }

        // Also exercise PageId explicitly (unused import guard):
        let _ = PageId(0);
        db.close()?;
    }
    Ok(())
}
