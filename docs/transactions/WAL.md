# Write-Ahead Log (WAL)

RDBX implements a **steal / no-force** ARIES-lite write-ahead log. Every
mutation to a persistent page goes through the WAL before the page itself
can be written back to disk.

## Guarantees

- **Atomicity**: a transaction is committed iff a `Commit` record for its
  `TxnId` is durable on disk. Every transaction without a `Commit` record
  — whether it ended with an explicit `Abort` or simply vanished in a
  crash — has its updates rolled back during recovery.
- **Durability (per SyncMode)**:

  | `SyncMode` | Every `Commit` fsync'd | Behaviour on crash |
  |---|---|---|
  | `Full` | yes | last committed txn is preserved |
  | `Normal` | no, at flush/close | last few commits may be lost; DB remains consistent |
  | `Off` | no | recent commits may be lost; intended for ephemeral data |

  `Full` is the default. We do not claim more than this table.

- **Consistency**: recovery replays history (`Redo`) and then rolls back
  every transaction that did not commit (`Undo`). Pages are only exposed
  to the buffer pool **after** recovery completes.

## On-disk layout

WAL lives in a sibling directory `<db>.wal/`. Each segment is a file
`segment-{segment_id:020}.wal`: `[ 64-byte SegmentHeader ][ WalRecord ] [ WalRecord ] …`

### SegmentHeader (64 bytes, LE)

| off | size | field |
|---|---|---|
| 0  | 8  | magic = `b"RDBXWAL\0"` |
| 8  | 2  | format_version (u16) |
| 10 | 2  | flags (u16, 0) |
| 12 | 4  | reserved (u32, 0) |
| 16 | 8  | base_lsn (u64) |
| 24 | 8  | segment_id (u64) |
| 32 | 16 | file_uuid |
| 48 | 8  | created_at_unix_nanos (u64) |
| 56 | 4  | reserved2 (u32, 0) |
| 60 | 4  | header_crc (CRC-32 of bytes 0..60) |

### WalRecord (variable length, LE)

| off | size | field |
|---|---|---|
| 0  | 4  | total_len (u32) = `40 + before_len + after_len + 4` |
| 4  | 1  | kind (0=Begin, 1=Update, 2=Commit, 3=Abort, 4=Checkpoint) |
| 5  | 3  | reserved (zero) |
| 8  | 8  | lsn (u64) |
| 16 | 8  | txn_id (u64) |
| 24 | 8  | page_id (u64; 0 for non-Update kinds) |
| 32 | 4  | before_len (u32) |
| 36 | 4  | after_len (u32) |
| 40 | N  | before_img |
| .. | M  | after_img |
| .. | 4  | crc (CRC-32 of bytes `0..total_len-4`) |

A truncated trailing record (crash in the middle of an append) is
silently dropped by recovery.

## WAL rule

Before any dirty page is written to disk, all WAL records up to and
including the LSN assigned to that page's mutation must be durable.
RDBX enforces this in `Database::flush` (`wal.flush()` precedes
`pool.flush_all()`) and in `Transaction::commit` (same order).

## Transaction id allocation

`Database::open` seeds its in-memory `next_txn_id` counter from the
**highest `TxnId` present in the WAL** plus one (this comes from
`RecoveryReport::max_txn_id`). Without this, two consecutive processes
would both start at `TxnId(1)`, and recovery would misclassify a fresh
uncommitted transaction as already-committed — silently skipping the
`Undo` phase.

After a `checkpoint`, `next_txn_id` is not reset, but the WAL is empty,
so recovery on the next open seeds the counter from zero again. This is
safe because recovery only classifies transactions present in the
**current** WAL.

## Recovery

1. **Analysis** — scan segments until end / first truncated tail.
   Classify each `TxnId`:
   - *committed*: has a `Commit` record
   - *aborted*: has an `Abort` record
   - *uncommitted*: has neither
   - *losers*: every transaction that is not committed (aborted ∪
     uncommitted); their updates must be undone.
2. **Redo** — for each `Update` record in LSN order, if the on-disk page
   has `header.lsn < record.lsn`, apply the after image and set
   `header.lsn = record.lsn`. This is **idempotent**: re-running recovery
   produces the same state.
3. **Undo** — for each `Update` record belonging to a *loser*, apply the
   before image in reverse LSN order and set `header.lsn` to the previous
   LSN.
4. **Flush** — flush the page manager.

Recovery runs in `Database::open` before the buffer pool and catalog are
created, so the pool never observes a half-applied state.

### Why aborted transactions are undone again

`Transaction::rollback` performs an in-place undo of the touched pages
before appending the `Abort` record. Those pages live in the buffer pool
and reach disk only when the buffer pool is flushed. If a crash happens
between the in-place undo and the next flush, the on-disk page still
carries the *after* image. Recovery's blind redo then re-applies the
`Update`, which would resurrect the aborted effect — unless Undo runs
for aborted transactions too.

Applying a before image twice is idempotent, so the double undo is not a
problem. RDBX does not use compensation log records (CLRs), so this
conservative redo-then-undo strategy is the only way to guarantee that
an aborted transaction never reappears after a crash.

## Checkpoints

`Database::checkpoint` flushes the WAL, flushes every dirty buffer frame,
fsyncs the data file, and truncates all WAL segments. Use this after a
batch of commits to reclaim disk space.

## Known limitations (Phase 3)

- Single-writer: a transaction holds `&mut Database`, so only one writer
  exists at a time. MVCC in Phase 5 removes this restriction.
- No compensation log records (CLRs). Undo re-applies before images
  directly, and aborted transactions are undone again on every recovery
  pass. Correct but slightly wasteful; a later optimisation can add CLRs
  to skip known-undone records.
- Before + after images means roughly 2× page size of WAL per update. A
  later optimisation can replace this with physiological redo + logical
  undo.
- No background flusher. `flush` and `checkpoint` are caller-driven.
- `next_txn_id` is recovered from the WAL, not persisted in the data
  file. After a checkpoint + restart, the counter restarts from 1; this
  is safe only because recovery ignores transactions absent from the
  current WAL.