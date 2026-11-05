//! Persistent storage for SQL table schemas.
//!
//! A dedicated B+Tree at `__rdbx_schema__` (internal table) maps
//! `schema_id (16-byte uuid)` -> serialized [`TableSchema`]. Each user
//! table's `TableDescriptor.schema_hash` field holds the uuid that
//! points at its schema blob.

use std::sync::{Arc, Mutex};

use rdbx_btree::{BTree, BTreeStore};
use rdbx_buffer::BufferPool;
use rdbx_core::{PageId, Result};
use rdbx_storage::PageManager;

pub(crate) struct SchemaStore {
    root: Mutex<PageId>,
    inner: Mutex<Option<SchemaStoreInner>>,
}

pub(crate) struct SchemaStoreInner {
    pub manager: Arc<Mutex<PageManager>>,
    pub pool: Arc<Mutex<BufferPool>>,
}

impl SchemaStore {
    pub fn new(root: PageId) -> Self {
        Self {
            root: Mutex::new(root),
            inner: Mutex::new(None),
        }
    }

    pub fn bind(&self, manager: Arc<Mutex<PageManager>>, pool: Arc<Mutex<BufferPool>>) {
        *self.inner.lock().unwrap() = Some(SchemaStoreInner { manager, pool });
    }

    pub fn current_root(&self) -> PageId {
        *self.root.lock().unwrap()
    }

    fn with_btree<R>(&self, f: impl FnOnce(&mut BTree, &mut dyn BTreeStore) -> R) -> R {
        let mut guard = self.inner.lock().unwrap();
        let inner = guard.as_mut().expect("SchemaStore not bound");
        let mut store = BoundStore {
            manager: inner.manager.clone(),
            pool: inner.pool.clone(),
        };
        let mut root_guard = self.root.lock().unwrap();
        let old_root = *root_guard;
        let mut tree = BTree::open(old_root);
        let result = f(&mut tree, &mut store);
        let new_root = tree.root();
        if new_root != old_root {
            *root_guard = new_root;
        }
        result
    }

    pub fn load(&self, schema_id: &[u8; 16]) -> Result<Option<Vec<u8>>> {
        self.with_btree(|tree, store| tree.search(store, schema_id))
    }

    pub fn store(&self, schema_id: [u8; 16], blob: Vec<u8>) -> Result<()> {
        self.with_btree(|tree, store| tree.insert(store, schema_id.to_vec(), blob))
    }

    #[allow(dead_code)]
    pub fn remove(&self, schema_id: &[u8; 16]) -> Result<()> {
        self.with_btree(|tree, store| {
            tree.delete(store, schema_id)?;
            Ok(())
        })
    }
}

struct BoundStore {
    manager: Arc<Mutex<PageManager>>,
    pool: Arc<Mutex<BufferPool>>,
}

impl BTreeStore for BoundStore {
    fn read_page(&mut self, pid: PageId) -> Result<rdbx_storage::Page> {
        let mut mgr = self.manager.lock().unwrap();
        let mut pool = self.pool.lock().unwrap();
        let fid = pool.fetch(&mut mgr, pid)?;
        let page = pool.get(fid)?.clone();
        pool.unpin(fid)?;
        Ok(page)
    }
    fn write_page(&mut self, pid: PageId, page: &rdbx_storage::Page) -> Result<()> {
        let mut mgr = self.manager.lock().unwrap();
        let mut pool = self.pool.lock().unwrap();
        let fid = pool.fetch(&mut mgr, pid)?;
        {
            let p = pool.get_mut(fid)?;
            p.payload.copy_from_slice(&page.payload);
            p.header.lsn = page.header.lsn;
        }
        pool.unpin(fid)?;
        Ok(())
    }
    fn allocate_page(&mut self, page_type: rdbx_storage::PageType) -> Result<PageId> {
        let mut mgr = self.manager.lock().unwrap();
        mgr.allocate_page(page_type)
    }
    fn page_size(&self) -> usize {
        self.manager.lock().unwrap().page_size()
    }
}
