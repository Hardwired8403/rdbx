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
| WAL (segment rotation, sync modes, checkpoints) | 3 | ✅ implemented |
| Crash recovery (redo/undo, steal + no-force) | 3 | ✅ implemented |
| Single-writer transactions (WAL-backed) | 3 | ✅ implemented |
| B+Tree index | 4 | ⏳ planned |
| MVCC / multi-writer transactions | 5 | ⏳ planned |
| Query engine / optimiser | 6 | ⏳ planned |
| SQL parser + executor | 7 | ⏳ planned |
| CLI / productisation | 8 | ⏳ planned |

## Quick start

```rust
use rdbx::{Database, DatabaseOptions};

fn main() -> rdbx::Result<()> {
    let mut db = Database::open("./data", DatabaseOptions::default())?;
    db.create_table("users")?;

    let root = db.table("users").unwrap().root_page;
    let mut txn = db.begin_transaction()?;
    let mut page = txn.read_page(root)?;
    page.payload[..5].copy_from_slice(b"HELLO");
    txn.write_page(root, &page)?;
    txn.commit()?;

    db.close()
}
```
## Project layout
```sh
crates/
├── rdbx-core     — types, errors, configuration (no rdbx deps)
├── rdbx-storage  — page format, file header, page manager, catalog
├── rdbx-buffer   — buffer pool, LRU / Clock eviction
├── rdbx-wal      — write-ahead log, crash recovery
├── rdbx          — public Database handle + transactions
└── rdbx-bench    — Criterion benchmarks
docs/
├── storage/BUFFER_POOL.md
├── storage/CATALOG.md
└── transactions/WAL.md
```
## Testing
``sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p rdbx-bench
``

## Durability matrix

| `SyncMode`       | Every `Commit` fsync'd | Behaviour on crash                                |
|------------------|----------------------|---------------------------------------------------|
| `Full` (default) | yes                  | last committed txn preserved                     |
| `Normal`         | no, at flush/close   | last few commits may be lost; DB stays consistent |
| `Off`            | no                   | recent commits may be lost; for ephemeral data   |

RDBX does **not** claim stronger durability than this table.

## Known limitations

- Single-writer: a transaction holds `&mut Database`. MVCC arrives in Phase 5.
- Buffer pool is single-threaded.
- No B+Tree yet — table root is a zeroed data page (Phase 4).
- Catalog fits in one page and is not transactional.
- Recovery has no compensation log records (CLRs); aborted transactions\
  are undone again on each recovery pass. Correct but slightly wasteful.
- `next_txn_id` is recovered from the WAL, not persisted in the data file\
  (safe because recovery only classifies txns present in the current WAL).

## Documentation

- [`docs/storage/BUFFER_POOL.md`](docs/storage/BUFFER_POOL.md)
- [`docs/storage/CATALOG.md`](docs/storage/CATALOG.md)
- [`docs/transactions/WAL.md`](docs/transactions/WAL.md)