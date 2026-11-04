//! `VersionStore` implementation that persists MVCC versions in a
//! dedicated B+Tree rooted at `mvcc_root`.
//!
//! The B+Tree's root page can change on split; `BTreeVersionStore`
//! tracks the current root internally, and `Database::flush` propagates
//! it into the catalog so the next `Database::open` finds the correct
//! tree.

use std::sync::Mutex;

use rdbx_btree::BTree;
use rdbx_core::{PageId, Result};
use rdbx_mvcc::VersionStore;

pub struct BTreeVersionStore {
    root: Mutex<PageId>,
    inner: Mutex<Option<StoreInner>>,
}

pub(crate) struct StoreInner {
    pub manager: std::sync::Arc<Mutex<rdbx_storage::PageManager>>,
    pub pool: std::sync::Arc<Mutex<rdbx_buffer::BufferPool>>,
}

impl BTreeVersionStore {
    pub fn new(root: PageId) -> Self {
        Self {
            root: Mutex::new(root),
            inner: Mutex::new(None),
        }
    }

    pub(crate) fn bind(
        &self,
        manager: std::sync::Arc<Mutex<rdbx_storage::PageManager>>,
        pool: std::sync::Arc<Mutex<rdbx_buffer::BufferPool>>,
    ) {
        let mut guard = self.inner.lock().unwrap();
        *guard = Some(StoreInner { manager, pool });
    }

    fn with_btree<R>(&self, f: impl FnOnce(&mut BTree, &mut dyn rdbx_btree::BTreeStore) -> R) -> R {
        let mut guard = self.inner.lock().unwrap();
        let inner = guard.as_mut().expect("BTreeVersionStore not bound");
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
}

struct BoundStore {
    manager: std::sync::Arc<Mutex<rdbx_storage::PageManager>>,
    pool: std::sync::Arc<Mutex<rdbx_buffer::BufferPool>>,
}

impl rdbx_btree::BTreeStore for BoundStore {
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

impl VersionStore for BTreeVersionStore {
    fn load(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.with_btree(|tree, store| tree.search(store, key))
    }

    fn store(&mut self, key: &[u8], value: Vec<u8>) -> Result<()> {
        self.with_btree(|tree, store| tree.insert(store, key.to_vec(), value))
    }

    fn remove(&mut self, key: &[u8]) -> Result<()> {
        self.with_btree(|tree, store| {
            tree.delete(store, key)?;
            Ok(())
        })
    }

    fn range_all(&mut self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.with_btree(|tree, store| tree.range(store, None, None))
    }

    fn current_root(&self) -> Option<PageId> {
        Some(*self.root.lock().unwrap())
    }
}
