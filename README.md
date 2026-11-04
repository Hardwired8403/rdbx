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
| B+Tree index (search/insert/delete/range) | 4 | ✅ implemented |
| MVCC / multi-writer transactions | 5 | ⏳ planned |
| Query engine / optimiser | 6 | ⏳ planned |
| SQL parser + executor | 7 | ⏳ planned |
| CLI / productisation | 8 | ⏳ planned |

## Quick start

```rust
use rdbx::{BTree, Database, DatabaseOptions};

fn main() -> rdbx::Result<()> {
    let mut db = Database::open("./data", DatabaseOptions::default())?;
    let root = db.create_table("users")?;

    let mut txn = db.begin_transaction()?;
    let mut tree = BTree::open(root);
    tree.insert(&mut txn, b"alice".to_vec(), b"42".to_vec())?;
    tree.insert(&mut txn, b"bob".to_vec(), b"17".to_vec())?;
    txn.commit()?;

    let mut txn = db.begin_transaction()?;
    let tree = BTree::open(root);
    assert_eq!(tree.search(&mut txn, b"alice")?, Some(b"42".to_vec()));
    let all = tree.range(&mut txn, None, None)?;
    println!("{} users", all.len());
    txn.rollback()?;

    db.close()
}
```

## Project layout

```text
crates/
├── rdbx-core     — types, errors, configuration (no rdbx deps)
├── rdbx-storage  — page format, file header, page manager, catalog
├── rdbx-buffer   — buffer pool, LRU / Clock eviction
├── rdbx-wal      — write-ahead log, crash recovery
├── rdbx-btree    — B+Tree index (storage-agnostic via BTreeStore)
├── rdbx          — public Database handle + transactions
└── rdbx-bench    — Criterion benchmarks

docs/
├── storage/
│   ├── BUFFER_POOL.md
│   ├── CATALOG.md
│   └── BTREE.md
└── transactions/
    └── WAL.md
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
- B+Tree page allocation is not WAL-logged: aborted transactions may
orphan pages (Phase 5 fixes this).
- Keys are opaque bytes with lexicographic comparison; typed encoding is
the SQL layer's responsibility (Phase 7).
- Catalog fits in one page and is not transactional.
- Recovery has no compensation log records (CLRs); aborted transactions
are undone again on each recovery pass.
- `next_txn_id` is recovered from the WAL, not persisted in the data file.

## Documentation

- [`docs/storage/BUFFER_POOL.md`](docs/storage/BUFFER_POOL.md)
- [`docs/storage/CATALOG.md`](docs/storage/CATALOG.md)
- [`docs/storage/BTREE.md`](docs/storage/BTREE.md)
- [`docs/transactions/WAL.md`](docs/transactions/WAL.md)