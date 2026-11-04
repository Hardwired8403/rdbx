# Buffer Pool

The RDBX buffer pool caches decoded `Page` objects in memory. It sits between
the storage engine (`PageManager`) and higher layers (B+Tree, query engine)
and is the only component that should touch hot data pages in the future.

## Model

- **Frame** — one slot in the pool, holding a decoded page, its `page_id`
  (when resident), a `pin_count`, and a `dirty` flag.
- **Pin** — an outstanding reference. A frame with `pin_count > 0` cannot be
  evicted. Callers must `unpin` after use, otherwise the pool deadlocks with
  "all frames pinned".
- **Dirty** — set by `get_mut`. Dirty pages are written back on eviction or
  on `flush_all`.

## API (`rdbx-buffer::BufferPool`)

| Method | Purpose |
|---|---|
| `new(capacity, page_size, policy)` | Construct with `capacity` pre-allocated frames |
| `fetch(mgr, pid)` | Get a pinned frame (hit or miss) |
| `new_page(mgr, type)` | Allocate in manager + insert as pinned & dirty |
| `free_page(mgr, pid)` | Evict any cached copy, then free in manager |
| `get(fid)` / `get_mut(fid)` | Borrow a pinned page |
| `unpin(fid)` | Release one pin |
| `flush_all(mgr)` | Write every dirty frame |
| `flush_frame(mgr, fid)` | Write one dirty frame |
| `stats()` | Hit/miss/eviction counters |

The pool borrows `&mut PageManager` per call — it does **not** own the file.

## Eviction policies

- **LRU** (default) — `VecDeque<FrameId>`, O(n) per touch. Fine for Phase 2
  capacities. A proper intrusive linked list is a future optimisation.
- **Clock** (second-chance) — `Vec<bool>` + hand, O(1) amortised.

Both are trait objects behind `EvictionPolicy`; adding a new policy only
requires implementing `on_insert`, `on_access`, `on_remove`, `select_victim`.

## Invariants

1. `pin_count > 0` ⇒ frame is not evictable.
2. `dirty` ⇒ frame was mutated via `get_mut` since the last write.
3. `dirty` ⇒ `page_id.is_some()` (dirty frames always hold a live page).
4. A freed page has no resident frame after `free_page` returns.
5. `fetch` on a hit increments `pin_count`; `unpin` decrements it. Over-unpin
   and over-pin are both reported as errors, never silently ignored.

## Known limitations (Phase 2)

- Not thread-safe. Phase 5 introduces MVCC and concurrency; the pool will be
  wrapped in a latch manager then.
- LRU is O(n) per touch — measured via Criterion before we switch to a more
  sophisticated structure.
- No background flusher. `flush_all` is caller-driven.

