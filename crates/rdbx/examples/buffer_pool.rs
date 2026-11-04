//! Demo: buffer pool with two policies and observable hit/miss stats.

use rdbx::{Database, DatabaseOptions, EvictionPolicyKind, PageType};

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
        let db = Database::open(&path, opts)?;
        {
            let (_mgr, pool) = db.storage();
            println!("policy = {:?}", pool.policy_name());
        }

        let pid;
        {
            let (mut mgr, mut pool) = db.storage();
            let fid = pool.new_page(&mut mgr, PageType::Data)?;
            let page = pool.get_mut(fid)?;
            page.payload[..MSG.len()].copy_from_slice(MSG);
            pid = page.header.page_id;
            pool.unpin(fid)?;
        }

        {
            let (mut mgr, mut pool) = db.storage();
            for _ in 0..6 {
                let f = pool.new_page(&mut mgr, PageType::Data)?;
                pool.unpin(f)?;
            }
        }

        {
            let (mut mgr, mut pool) = db.storage();
            let fid = pool.fetch(&mut mgr, pid)?;
            let page = pool.get(fid)?;
            let text = std::str::from_utf8(&page.payload[..MSG.len()])
                .map_err(|e| rdbx::Error::Internal(format!("bad UTF-8: {e}")))?;
            println!("  read back: {text}");
            pool.unpin(fid)?;
            let s = pool.stats();
            println!(
                "  hits={} misses={} evictions={} dirty_flushes={} hit_ratio={:.2}",
                s.hits,
                s.misses,
                s.evictions,
                s.dirty_flushes,
                s.hit_ratio()
            );
        }

        db.close()?;
    }
    Ok(())
}
