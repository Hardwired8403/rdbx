# MVCC (Multi-Version Concurrency Control)

## Status: Phase 5a

Phase 5a delivers an **in-memory, snapshot-isolation MVCC layer** in the
new `rdbx-mvcc` crate. The layer is not yet integrated with the persistent
B+Tree; it is a self-contained, tested model of the isolation semantics
that Phase 5b will wire through the WAL and the index.

**What Phase 5a guarantees:**

- Every write produces a new immutable [`Version`] in a per-key history.
- Every snapshot taken by a reader sees a consistent point in time.
- A snapshot never sees commits that occurred after it was taken.
- A transaction always sees its own pending writes.
- Two concurrent writers that both write the same key are detected: the
  second commit fails with `CommitError::WriteConflict`.
- Concurrent writes to disjoint keys both succeed.

**What Phase 5a does NOT provide:**

- **Persistence.** Versions live in an in-memory `HashMap`. Nothing in
  `rdbx-mvcc` touches disk.
- **True multi-writer concurrency in `rdbx`.** `Transaction<'a>` still
  holds `&mut Database`, so only one transaction can exist at a time at
  the API level. The isolation model is correct; the type-level plumbing
  to allow many concurrent transactions lands in Phase 5b.
- **MVCC on B+Tree entries.** The B+Tree continues to store one current
  version per key (Phase 4 behaviour). Versioned leaf entries arrive in
  Phase 5b together with the persistence integration.
- **Garbage collection.** Old versions accumulate forever. GC is a Phase
  5b deliverable.
- **Serializable isolation.** We implement snapshot isolation. RDBX does
  not claim stronger guarantees than this table.

## Model

### Version
```rust
Version {
txn_id: TxnId, // writer
commit_ts: Lsn, // monotonic commit LSN
tombstone: bool, // true = key deleted
value: Vec<u8>, // empty when tombstone
}
```

Every version is immutable once committed. A [`VersionedValue`] stores
versions in ascending `commit_ts` order.

### Snapshot
```rust
Snapshot {
xmax: TxnId, // highest allocated txn_id + 1
xmin: TxnId, // lowest active txn_id at snapshot time (0 = none)
active: HashSet<TxnId>,// txns running at snapshot time
lsn: Lsn, // manager LSN when the snapshot was taken
}
```


A version `v` is visible to a snapshot `s` when:

- `v.txn_id < s.xmax`
- `v.txn_id ∉ s.active`
- `v.commit_ts < s.lsn`
- (own writes are always visible regardless of the above, matched by
  `v.txn_id == own_txn_id`)

### Conflict detection

For each key the transaction wrote, if any other transaction with
`commit_ts >= snapshot.lsn` committed a version of that key, the commit
fails with `WriteConflict`. This is **first-updater-wins** snapshot
isolation: two concurrent write-write attempts are detected, and the
later one is aborted by the application.

## Manager API

| Method | Purpose |
|---|---|
| `TransactionManager::new()` | Fresh manager |
| `begin(lsn) -> SnapshotHandle` | Start a transaction, return its snapshot |
| `write(txn_id, key, Option<value>)` | Buffer a write (`None` = delete) |
| `read(snapshot, own_txn_id, key) -> Visible` | Snapshot-filtered read |
| `commit(txn_id, snapshot)` | Commit with conflict detection |
| `abort(txn_id)` | Discard pending writes |
| `status(txn_id)` | Query transaction state |
| `xmin()` | Oldest active txn id (for future GC) |

`Visible` is one of `Absent`, `Present(Vec<u8>)`, `Deleted`.

## Integration roadmap

- **Phase 5b**: persist versions in a dedicated B+Tree columnar format;
  back `TransactionManager::begin` by the WAL so recovery reconstructs
  the active set; allow many `Transaction` handles to coexist by
  wrapping `BufferPool` / `WalManager` in `Arc<RwLock<…>>`.
- **Phase 5b+**: garbage collection using `xmin()`; short version
  histories will be pruned periodically.
- **Future**: serializable isolation via SSI — not attempted.

