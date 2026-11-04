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
| MVCC (in-memory snapshot isolation) | 5a | ✅ implemented |
| MVCC persistence + multi-writer + GC | 5b | ✅ implemented |
| Query engine / optimiser | 6 | ⏳ planned |
| SQL parser + executor | 7 | ⏳ planned |
| CLI / productisation | 8 | ⏳ planned |

## Isolation guarantees

| Property | Status |
|---|---|
| Snapshot isolation (MVCC) | ✅ implemented |
| Write-write conflict detection (first-updater-wins) | ✅ implemented |
| Persistent MVCC versions (separate B+Tree) | ✅ implemented |
| Multi-writer concurrency (`Arc<Mutex<>>` per subsystem) | ✅ implemented |
| Version garbage collection | ✅ implemented (called on checkpoint / explicit `gc()`) |
| Serializable isolation | ❌ not implemented |

RDBX does not claim stronger isolation than this table.

## Quick start

```rust
use rdbx::{Database, DatabaseOptions, Visible};

fn main() -> rdbx::Result<()> {
    let db = Database::open("./data", DatabaseOptions::default())?;

    // Write through MVCC:
    let mut txn = db.begin_transaction()?;
    txn.mvcc_write(b"users/alice".to_vec(), Some(b"42".to_vec()))?;
    txn.commit()?;

    // Read through a snapshot:
    let mut txn = db.begin_transaction()?;
    assert_eq!(
        txn.mvcc_read(b"users/alice")?,
        Visible::Present(b"42".to_vec()),
    );
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
├── rdbx-mvcc     — MVCC model + version store abstraction
├── rdbx          — public Database handle + transactions
└── rdbx-bench    — Criterion benchmarks
docs/
├── storage/BUFFER_POOL.md
├── storage/CATALOG.md
├── storage/BTREE.md
├── transactions/WAL.md
└── transactions/MVCC.md
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

- MVCC uses a separate B+Tree for versions; user tables remain single-version
(Phase 6 will route user data through MVCC versions too).
- Row-level MVCC in the user B+Tree is not implemented.
- Buffer pool is thread-safe via coarse-grained Mutex, not per-page latches.
- B+Tree page allocation is not WAL-logged: aborted transactions may
orphan pages.
- No redistribution in the B+Tree; underflow is handled by merge-only.
- Keys are opaque bytes with lexicographic comparison; typed encoding is
the SQL layer's responsibility (Phase 7).
- Catalog fits in one page and is not transactional.
- Recovery has no compensation log records (CLRs); aborted transactions
are undone again on each recovery pass.
- Serializable isolation is not implemented.
- `next_commit_ts` restore at `Database::open` scans the entire MVCC tree; acceptable for Phase 5b, will be optimized in Phase 6.
- `__rdbx_mvcc__` is an internal table stored in the catalog and filtered
out of `Database::tables()` / `Database::table()`.

## Documentation

- [`docs/storage/BUFFER_POOL.md`](docs/storage/BUFFER_POOL.md)
- [`docs/storage/CATALOG.md`](docs/storage/CATALOG.md)
- [`docs/storage/BTREE.md`](docs/storage/BTREE.md)
- [`docs/transactions/WAL.md`](docs/transactions/WAL.md)
- [`docs/transactions/MVCC.md`](docs/transactions/MVCC.md)