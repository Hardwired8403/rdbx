//! Top-level `Database` handle (Phase 5b).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rdbx_btree::{BTreeStore, LeafNode, Node};
use rdbx_buffer::BufferPool;
use rdbx_core::{DatabaseOptions, Error, Lsn, PageId, Result, TxnId};
use rdbx_mvcc::{Snapshot, TransactionManager, TxnStatus, Visible};
use rdbx_storage::{Catalog, Page, PageManager, PageType, TableDescriptor};
use rdbx_wal::{RecordKind, WalManager};

use crate::mvcc_store::BTreeVersionStore;
use crate::transaction::Transaction;

pub struct Database {
    path: PathBuf,
    pub(crate) manager: Arc<Mutex<PageManager>>,
    pub(crate) pool: Arc<Mutex<BufferPool>>,
    pub(crate) catalog: Mutex<Catalog>,
    pub(crate) wal: Arc<Mutex<WalManager>>,
    pub(crate) tm: Arc<Mutex<TransactionManager>>,
    pub(crate) mvcc_root: Mutex<PageId>,
    pub(crate) sql_catalog: Mutex<rdbx_sql::catalog::SqlCatalog>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>, opts: DatabaseOptions) -> Result<Self> {
        opts.validate()?;
        let path = path.as_ref().to_path_buf();
        let mut manager = if path.exists() {
            PageManager::open(&path, &opts)?
        } else if opts.create_if_missing {
            PageManager::create(&path, &opts)?
        } else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("database not found: {}", path.display()),
            )));
        };

        let wal_dir = wal_dir_for(&path);
        let mut wal = WalManager::open(&wal_dir, opts.sync_mode)?;
        let report = rdbx_wal::recover(&mut wal, &mut manager)?;
        let next_txn_id = report
            .max_txn_id
            .checked_add(1)
            .ok_or(Error::Overflow("txn id"))?;

        let mut catalog = Catalog::load(&mut manager)?;
        let pool = BufferPool::new(
            opts.buffer_pool_capacity,
            manager.page_size(),
            opts.buffer_pool_policy,
        );

        let mvcc_root = if let Some(t) = catalog.table_by_name("__rdbx_mvcc__") {
            t.root_page
        } else {
            let pid = manager.allocate_page(PageType::Index)?;
            let leaf = LeafNode::new();
            let mut page = Page::new(pid, PageType::Index, manager.page_size());
            Node::Leaf(leaf).write_to(&mut page)?;
            manager.write_page(&page)?;
            let desc = TableDescriptor {
                id: catalog.next_table_id.max(1),
                name: "__rdbx_mvcc__".to_string(),
                root_page: pid,
                schema_hash: [0u8; 16],
            };
            catalog.insert(desc);
            catalog.flush(&mut manager)?;
            manager.flush()?;
            pid
        };

        let manager_arc = Arc::new(Mutex::new(manager));
        let pool_arc = Arc::new(Mutex::new(pool));

        let mut store = BTreeVersionStore::new(mvcc_root);
        store.bind(manager_arc.clone(), pool_arc.clone());

        // Restore next_commit_ts from the highest commit_ts present in
        // the persisted MVCC version tree. Without this, a fresh process
        // would start at commit_ts=1, and its snapshots (lsn=1) would be
        // unable to see any previously committed versions.
        let mut max_commit_ts = 0u64;
        {
            let all = rdbx_mvcc::VersionStore::range_all(&mut store)?;
            for (_, bytes) in &all {
                if let Ok(vv) = rdbx_mvcc::codec::decode(bytes) {
                    for v in &vv.versions {
                        max_commit_ts = max_commit_ts.max(v.commit_ts.0);
                    }
                }
            }
        }
        let next_commit_ts = max_commit_ts
            .checked_add(1)
            .ok_or(Error::Overflow("commit_ts"))?;

        let tm = TransactionManager::with_store(Box::new(store))
            .with_next_txn_id(next_txn_id)
            .with_next_commit_ts(next_commit_ts);

        Ok(Self {
            path,
            manager: manager_arc,
            pool: pool_arc,
            catalog: Mutex::new(catalog),
            wal: Arc::new(Mutex::new(wal)),
            tm: Arc::new(Mutex::new(tm)),
            mvcc_root: Mutex::new(mvcc_root),
            sql_catalog: Mutex::new(rdbx_sql::catalog::SqlCatalog::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn page_size(&self) -> usize {
        self.manager
            .lock()
            .expect("page_manager poisoned")
            .page_size()
    }

    pub fn page_count(&self) -> u64 {
        self.manager
            .lock()
            .expect("page_manager poisoned")
            .page_count()
    }

    pub fn free_page_count(&self) -> u64 {
        self.manager
            .lock()
            .expect("page_manager poisoned")
            .free_page_count()
    }

    pub fn page_manager(&self) -> MutexGuard<'_, PageManager> {
        self.manager.lock().expect("page_manager poisoned")
    }

    pub fn storage(&self) -> (MutexGuard<'_, PageManager>, MutexGuard<'_, BufferPool>) {
        (
            self.manager.lock().expect("page_manager poisoned"),
            self.pool.lock().expect("buffer_pool poisoned"),
        )
    }

    pub fn read_page(&self, pid: PageId) -> Result<Page> {
        let mut mgr = self.manager.lock().unwrap();
        let mut pool = self.pool.lock().unwrap();
        let fid = pool.fetch(&mut mgr, pid)?;
        let page = pool.get(fid)?.clone();
        pool.unpin(fid)?;
        Ok(page)
    }

    pub fn begin_transaction(&self) -> Result<Transaction<'_>> {
        let handle = {
            let mut tm = self.tm.lock().unwrap();
            tm.begin()
        };
        {
            let mut wal = self.wal.lock().unwrap();
            wal.append(
                handle.txn_id,
                RecordKind::Begin,
                PageId(0),
                Vec::new(),
                Vec::new(),
            )?;
        }
        Ok(Transaction::new(self, handle))
    }

    pub fn flush(&self) -> Result<()> {
        // 1. Propagate MVCC root into catalog if it changed.
        let new_mvcc_root = {
            let tm = self.tm.lock().unwrap();
            tm.store().current_root()
        };
        if let Some(new_root) = new_mvcc_root {
            let mut root_guard = self.mvcc_root.lock().unwrap();
            if *root_guard != new_root {
                let mut mgr = self.manager.lock().unwrap();
                {
                    let mut catalog = self.catalog.lock().unwrap();
                    if let Some(desc) = catalog
                        .tables
                        .iter_mut()
                        .find(|t| t.name == "__rdbx_mvcc__")
                    {
                        desc.root_page = new_root;
                    }
                    catalog.flush(&mut mgr)?;
                }
                mgr.flush()?;
                *root_guard = new_root;
            }
        }

        // 2. WAL flush.
        {
            let mut wal = self.wal.lock().unwrap();
            wal.flush()?;
        }

        // 3. Buffer pool flush.
        {
            let mut mgr = self.manager.lock().unwrap();
            let mut pool = self.pool.lock().unwrap();
            pool.flush_all(&mut mgr)?;
            mgr.flush()?;
        }
        Ok(())
    }

    pub fn checkpoint(&self) -> Result<usize> {
        self.flush()?;
        {
            let mut wal = self.wal.lock().unwrap();
            wal.truncate()?;
        }
        self.gc()
    }

    pub fn gc(&self) -> Result<usize> {
        let mut tm = self.tm.lock().unwrap();
        tm.gc()
    }

    pub fn close(self) -> Result<()> {
        self.flush()?;
        let mut mgr = self.manager.lock().unwrap();
        mgr.close()
    }

    pub fn catalog(&self) -> MutexGuard<'_, Catalog> {
        self.catalog.lock().expect("catalog poisoned")
    }

    /// User-visible tables. Internal tables (whose names start with `__`)
    /// are hidden.
    pub fn tables(&self) -> Vec<TableDescriptor> {
        self.catalog
            .lock()
            .expect("catalog poisoned")
            .tables
            .iter()
            .filter(|t| !t.name.starts_with("__"))
            .cloned()
            .collect()
    }

    /// Look up a user-visible table by name. Internal tables are hidden.
    pub fn table(&self, name: &str) -> Option<TableDescriptor> {
        if name.starts_with("__") {
            return None;
        }
        self.catalog
            .lock()
            .expect("catalog poisoned")
            .table_by_name(name)
            .cloned()
    }

    pub fn create_table(&mut self, name: &str) -> Result<PageId> {
        self.create_table_locked(name)
    }

    pub(crate) fn create_table_locked(&self, name: &str) -> Result<PageId> {
        if self.catalog.lock().unwrap().table_by_name(name).is_some() {
            return Err(Error::Internal(format!("table '{name}' already exists")));
        }
        let mut mgr = self.manager.lock().unwrap();
        let root_pid = mgr.allocate_page(PageType::Index)?;
        let leaf = LeafNode::new();
        let mut page = Page::new(root_pid, PageType::Index, mgr.page_size());
        Node::Leaf(leaf).write_to(&mut page)?;
        mgr.write_page(&page)?;
        {
            let mut catalog = self.catalog.lock().unwrap();
            let desc = TableDescriptor {
                id: catalog.next_table_id.max(1),
                name: name.to_string(),
                root_page: root_pid,
                schema_hash: [0u8; 16],
            };
            catalog.insert(desc);
            catalog.flush(&mut mgr)?;
        }
        Ok(root_pid)
    }

    pub fn drop_table(&mut self, name: &str) -> Result<()> {
        self.drop_table_locked(name)
    }

    pub(crate) fn drop_table_locked(&self, name: &str) -> Result<()> {
        let mut mgr = self.manager.lock().unwrap();
        let mut catalog = self.catalog.lock().unwrap();
        let desc = catalog
            .remove_by_name(name)
            .ok_or_else(|| Error::Internal(format!("table '{name}' not found")))?;
        mgr.free_page(desc.root_page)?;
        catalog.flush(&mut mgr)?;
        Ok(())
    }

    pub(crate) fn mvcc_read(
        &self,
        snapshot: &Snapshot,
        txn_id: TxnId,
        key: &[u8],
    ) -> Result<Visible> {
        let mut tm = self.tm.lock().unwrap();
        tm.read(snapshot, txn_id, key)
    }

    pub(crate) fn mvcc_write(
        &self,
        txn_id: TxnId,
        key: Vec<u8>,
        value: Option<Vec<u8>>,
    ) -> Result<()> {
        let mut tm = self.tm.lock().unwrap();
        tm.write(txn_id, key, value)
    }

    pub fn snapshot_of(&self, txn_id: TxnId) -> Option<Snapshot> {
        let tm = self.tm.lock().unwrap();
        match tm.status(txn_id) {
            Some(TxnStatus::Active) => Some(Snapshot {
                xmax: TxnId(u64::MAX),
                xmin: tm.xmin(),
                active: Default::default(),
                lsn: Lsn(u64::MAX),
            }),
            _ => None,
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl BTreeStore for &Database {
    fn read_page(&mut self, pid: PageId) -> Result<Page> {
        Database::read_page(self, pid)
    }
    fn write_page(&mut self, pid: PageId, page: &Page) -> Result<()> {
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
    fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        let mut mgr = self.manager.lock().unwrap();
        mgr.allocate_page(page_type)
    }
    fn page_size(&self) -> usize {
        self.manager.lock().unwrap().page_size()
    }
}

fn wal_dir_for(db_path: &Path) -> PathBuf {
    let file_name = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("rdbx");
    match db_path.parent() {
        Some(parent) => parent.join(format!("{file_name}.wal")),
        None => PathBuf::from(format!("{file_name}.wal")),
    }
}
