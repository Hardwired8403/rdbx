# Catalog

The catalog is the persistent list of tables. It lives entirely in page 1
(`PageId::META`), whose payload stores an explicitly-serialised list of table
descriptors.

## On-disk layout (page 1 payload)

Little-endian, no padding, no Rust struct layout dependence.
offset size field
`0 8 catalog_version : u64 (incremented on every flush)
8 4 table_count : u32
12 4 reserved : u32 (must be zero)
16 … entries, each:
offset size field
0 8 table_id : u64
8 8 root_page : u64
16 16 schema_hash : [u8; 16]
32 2 name_len : u16
34 N name_utf8 : [u8; name_len]
`name_len ≤ 255`. The whole encoded payload must fit inside
`page_size − PAGE_HEADER_SIZE` bytes; otherwise `Catalog::flush` returns
`Error::Internal("catalog page overflow")`.
## Behaviour

- `Catalog::load(mgr)` reads page 1 and decodes. An all-zero page (fresh
  database, before the first catalog write) is decoded as an empty catalog.
- `Catalog::flush(mgr)` bumps `version`, encodes, and overwrites page 1.
- `Catalog` is **not** cached in the buffer pool: it is a single page and
  changes infrequently, so bypassing the pool keeps the two views in sync.
## Phase 2 limitations

- No indexes, no columns — the root page is a zeroed data page. Phase 4
  replaces it with a B+Tree root; Phase 7 adds the full schema.
- No overflow: if the catalog exceeds one page, `flush` fails. Larger
  catalogs require a spill chain (Phase 4+).
- Not transactional: a crash between `create_table`'s `allocate_page` and
  `catalog.flush` leaves an orphan page. WAL in Phase 3 makes it atomic.