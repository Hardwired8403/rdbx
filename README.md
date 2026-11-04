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
| Query engine (logical/physical plans, optimiser) | 6 | ✅ implemented |
| SQL parser + executor | 7 | ⏳ planned |
| CLI / productisation | 8 | ⏳ planned |


## Project layout

```text
crates/
├── rdbx-core     — types, errors, configuration (no rdbx deps)
├── rdbx-storage  — page format, file header, page manager, catalog
├── rdbx-buffer   — buffer pool, LRU / Clock eviction
├── rdbx-wal      — write-ahead log, crash recovery
├── rdbx-btree    — B+Tree index (storage-agnostic via BTreeStore)
├── rdbx-mvcc     — MVCC model + version store abstraction
├── rdbx-query — query engine: value/plan/exec/optimizer
├── rdbx          — public Database handle + transactions
└── rdbx-bench    — Criterion benchmarks
docs/
├── storage/BUFFER_POOL.md
├── storage/CATALOG.md
├── storage/BTREE.md
├── transactions/WAL.md
└── transactions/MVCC.md
```


## Query engine quick start

```rust
use rdbx_query::{execute, optimize, LogicalPlan};
use rdbx_query::plan::{Expr, BinOp};
use rdbx_query::value::Value;

// Describe what you want as a logical plan...
let plan = LogicalPlan::Filter {
    input: Box::new(LogicalPlan::Scan { root }),
    predicate: Expr::Binary(
        Box::new(Expr::Column(0)),
        BinOp::Ge,
        Box::new(Expr::Literal(Value::Int(100))),
    ),
};

// Optimise (index scan, predicate pushdown) and execute.
let mut exec = execute(optimize(plan), &mut btree_store)?;
while let Some(row) = exec.fetch_next()? {
    println!("{:?}", row);
}
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

- Query engine: aggregation is declared but not executed yet; Sort
materializes all rows in memory; no cost-based join.
- MVCC: user tables remain single-version (versions live in a
separate internal tree). Serializable isolation is not implemented.
- Storage: B+Tree page allocation is not WAL-logged; aborted
transactions may orphan pages.
- B+Tree: no redistribution on underflow, merge-only.
- Catalog: fits in one page and is not transactional.
- WAL: no CLRs; aborted transactions are undone again on each
recovery pass.
- `next_commit_ts` restore at `Database::open` is O(N) over the
MVCC tree.

## Documentation

- [`docs/storage/BUFFER_POOL.md`](docs/storage/BUFFER_POOL.md)
- [`docs/storage/CATALOG.md`](docs/storage/CATALOG.md)
- [`docs/storage/BTREE.md`](docs/storage/BTREE.md)
- [`docs/transactions/WAL.md`](docs/transactions/WAL.md)
- [`docs/transactions/MVCC.md`](docs/transactions/MVCC.md)