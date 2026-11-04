# RDBX

A high-performance embedded database engine written in Rust.
**Independently implemented storage engine** — not a wrapper around SQLite,
PostgreSQL, or RocksDB.

## Status

| Layer | Phase | Status |
|---|---|---|
| Cargo workspace | 1 | ✅ implemented |
| Persistent page format (magic, CRC-32, versioned header) | 1 | ✅ implemented |
| `PageManager` (allocate / free / read / write) | 1 | ✅ implemented |
| Free-list with well-formedness check | 2 | ✅ implemented |
| Buffer pool (LRU + Clock, pin/unpin, dirty tracking) | 2 | ✅ implemented |
| Catalog (persistent table descriptors) | 2 | ✅ implemented |
| Read-only mode | 2 | ✅ implemented |
| WAL / crash recovery | 3 | ⏳ planned |
| B+Tree | 4 | ⏳ planned |
| MVCC / transactions | 5 | ⏳ planned |
| Query engine / optimiser | 6 | ⏳ planned |
| SQL parser + executor | 7 | ⏳ planned |
| CLI / productisation | 8 | ⏳ planned |

## Quick start

```rust
use rdbx::{Database, DatabaseOptions};

fn main() -> rdbx::Result<()> {
    let mut db = Database::open("./data", DatabaseOptions::default())?;
    let root = db.create_table("users")?;
    println!("users root page = {root}");

    // Buffered page access:
    let (mgr, pool) = db.storage();
    let fid = pool.fetch(mgr, root)?;
    let _page = pool.get(fid)?;
    pool.unpin(fid)?;

    db.close()
}
```
## Project layout
```sh
crates/
├── rdbx-core     — types, errors, configuration (no rdbx deps)
├── rdbx-storage  — page format, file header, page manager, catalog
├── rdbx-buffer   — buffer pool, LRU / Clock eviction
├── rdbx          — public Database handle
└── rdbx-bench    — Criterion benchmarks
docs/
├── storage/BUFFER_POOL.md
└── storage/CATALOG.md
```
## Testing
``sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p rdbx-bench
``

## Known limitations
- No WAL yet — durability relies on flush(); the Normal / Off sync
modes can lose recent writes on crash.

- Buffer pool is single-threaded (concurrency arrives with MVCC in Phase 5).

- Catalog fits in one page and is not transactional.

- `PageManager::allocate_page` performs one full page write per allocation
(Phase 2 keeps the format clean; Phase 3 WAL makes it atomic).

## Documentation
- docs/storage/BUFFER_POOL.md
- docs/storage/CATALOG.md