# B+Tree

RDBX stores indexes as B+Trees. The implementation lives in the
`rdbx-btree` crate and is storage-agnostic via the [`BTreeStore`] trait:
in RDBX the tree is driven by `Transaction`, so every mutation flows
through the WAL and the buffer pool automatically.

## Node layout

Both node kinds share an 8-byte header placed at the start of the page's
**payload** (the 24-byte `PageHeader` from Phase 1 sits before it):

| offset | size | field |
|---|---|---|
| 0 | 1 | type (1 = leaf, 2 = internal) |
| 1 | 1 | flags (currently 0) |
| 2 | 2 | entry count (u16, LE) |
| 4 | 4 | reserved (0) |

After the header:

- **Leaf**: a sequence of entries, each
  `key_len (u16, LE) | key | val_len (u32, LE) | val`.
- **Internal**: an 8-byte `leftmost_child` page id, then entries each
  `key_len (u16, LE) | key | child_pid (u64, LE)`.

Separators in an internal node follow **Layout A**: with entries
`[(k_0, c_1), (k_1, c_2), …]` and leftmost child `c_0`, the subtree at
`c_i` covers the key range bounded below by `k_{i-1}` (inclusive) and
above by `k_i` (exclusive); `c_0` has no lower bound, and the rightmost
child has no upper bound.

Splits preserve Layout A exactly.

## Operations

### Search

Descend from root following `InternalNode::find_child_index` until a leaf
is reached; binary search the leaf's entries. `O(log n)` disk reads per
lookup.

### Insert

Recursive descent. On overflow:

- Leaf splits in half; the first key of the new right half is **copied**
  into the parent as a separator (the key still lives in both leaves).
- Internal node splits in half; the median separator **moves** into the
  parent (removed from the internal node).
- If the root splits, a new internal root is allocated and `self.root` is
  updated.

Duplicate keys overwrite the existing value.

### Delete

Recursive descent. On underflow (empty non-root leaf), the parent tries
to **merge** the child with an adjacent sibling. Merge succeeds only when
the combined node fits within one page; if it doesn't, the child is left
empty (correct but wasteful).

The merged-away sibling page is **orphaned**: it is not freed because
transactional page deallocation lands in Phase 5.

Redistribution (borrowing a single entry from a sibling) is not
implemented; the tree is always structurally correct but may become
sparser than optimal after many deletes.

### Range scan

Recursive traversal of the tree, pruning subtrees whose separator range
does not intersect the requested `[lo, hi)` interval. No `next_leaf`
linked list is used: the tree structure alone drives iteration, so a
corrupted `next_leaf` pointer cannot corrupt a scan.

## Page allocation

`BTreeStore::allocate_page` delegates to `PageManager::allocate_page`,
which writes a zeroed page to disk immediately and bumps `page_count`.
**Allocation is not WAL-logged**: if the transaction aborts, the page
remains orphaned on disk (its content may be restored to zeros by undo,
but `page_count` and the free-list are unchanged). Phase 5 will make
allocation transactional.

## Root page

`Database::create_table` initialises the root as an empty leaf. The root
page id is stored in `TableDescriptor.root_page`. To use a table as a
B+Tree:

```rust
let root = db.create_table("users")?;
let mut txn = db.begin_transaction()?;
let mut tree = BTree::open(root);
tree.insert(&mut txn, b"alice".to_vec(), b"42".to_vec())?;
txn.commit()?;
```

## Known limitations (Phase 4)

- **Page allocation is not transactional**: aborted transactions may\
  orphan pages.
- **No redistribution**: underflow is handled by merge-only; a sibling\
  that would need to donate a single entry is skipped.
- **Orphaned merge pages**: sibling pages replaced by a merge stay\
  allocated until Phase 5 introduces transactional free.
- **Key comparator**: lexicographic `Vec<u8>` comparison only. Typed\
  key encoding (e.g., big-endian integers) is the SQL layer's\
  responsibility (Phase 7).
- **No prefix compression**: full keys are stored on every entry.\
  Benchmarks in Phase 6 will decide whether this matters.

