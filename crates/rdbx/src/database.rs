//! Top-level `Database` handle.

use std::path::{Path, PathBuf};

use rdbx_btree::{BTreeStore, LeafNode, Node};
use rdbx_buffer::BufferPool;
use rdbx_core::{DatabaseOptions, Error, PageId, Result, TxnId};
use rdbx_storage::{Catalog, Page, PageManager, PageType, TableDescriptor};
use rdbx_wal::{RecordKind, WalManager};

use crate::transaction::Transaction;

/// The embedded database handle.
pub struct Database {
    path: PathBuf,
    pub(crate) manager: PageManager,
    pub(crate) pool: BufferPool,
    catalog: Catalog,
    pub(crate) wal: WalManager,
    next_txn_id: u64,
}

impl Database {
    /// Open an existing database, or create it when `opts.create_if_missing` is true.
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

        let catalog = Catalog::load(&mut manager)?;
        let pool = BufferPool::new(
            opts.buffer_pool_capacity,
            manager.page_size(),
            opts.buffer_pool_policy,
        );

        Ok(Self {
            path,
            manager,
            pool,
            catalog,
            wal,
            next_txn_id,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn page_size(&self) -> usize {
        self.manager.page_size()
    }
    pub fn page_count(&self) -> u64 {
        self.manager.page_count()
    }
    pub fn free_page_count(&self) -> u64 {
        self.manager.free_page_count()
    }

    /// Low-level access to the page manager (bypasses the buffer pool).
    pub fn page_manager(&mut self) -> &mut PageManager {
        &mut self.manager
    }

    /// Borrow the manager and the buffer pool simultaneously.
    pub fn storage(&mut self) -> (&mut PageManager, &mut BufferPool) {
        (&mut self.manager, &mut self.pool)
    }

    /// Read a page through the buffer pool.
    pub fn read_page(&mut self, pid: PageId) -> Result<Page> {
        let fid = self.pool.fetch(&mut self.manager, pid)?;
        let page = self.pool.get(fid)?.clone();
        self.pool.unpin(fid)?;
        Ok(page)
    }

    /// Begin a new transaction.
    pub fn begin_transaction(&mut self) -> Result<Transaction<'_>> {
        let txn_id = TxnId(self.next_txn_id);
        self.next_txn_id = self
            .next_txn_id
            .checked_add(1)
            .ok_or(Error::Overflow("txn id"))?;
        self.wal
            .append(txn_id, RecordKind::Begin, PageId(0), Vec::new(), Vec::new())?;
        Ok(Transaction::new(self, txn_id))
    }

    /// Flush the WAL, buffer pool, and manager (WAL first, then pages).
    pub fn flush(&mut self) -> Result<()> {
        self.wal.flush()?;
        self.pool.flush_all(&mut self.manager)?;
        self.manager.flush()?;
        Ok(())
    }

    /// Flush everything and truncate the WAL.
    pub fn checkpoint(&mut self) -> Result<()> {
        self.flush()?;
        self.wal.truncate()?;
        Ok(())
    }

    /// Gracefully close the database.
    pub fn close(mut self) -> Result<()> {
        self.flush()?;
        self.manager.close()
    }

    // --- catalog ---

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    pub fn tables(&self) -> &[TableDescriptor] {
        &self.catalog.tables
    }
    pub fn table(&self, name: &str) -> Option<&TableDescriptor> {
        self.catalog.table_by_name(name)
    }

    /// Create a new table. The root page is initialised to an empty
    /// B+Tree leaf; callers open it with `BTree::open(root_page)`.
    pub fn create_table(&mut self, name: &str) -> Result<PageId> {
        if self.catalog.table_by_name(name).is_some() {
            return Err(Error::Internal(format!("table '{name}' already exists")));
        }
        let root_pid = self.manager.allocate_page(PageType::Index)?;
        let leaf = LeafNode::new();
        let mut page = Page::new(root_pid, PageType::Index, self.manager.page_size());
        Node::Leaf(leaf).write_to(&mut page)?;
        self.manager.write_page(&page)?;
        let desc = TableDescriptor {
            id: self.catalog.next_table_id.max(1),
            name: name.to_string(),
            root_page: root_pid,
            schema_hash: [0u8; 16],
        };
        self.catalog.insert(desc);
        self.catalog.flush(&mut self.manager)?;
        Ok(root_pid)
    }

    pub fn drop_table(&mut self, name: &str) -> Result<()> {
        let desc = self
            .catalog
            .remove_by_name(name)
            .ok_or_else(|| Error::Internal(format!("table '{name}' not found")))?;
        self.manager.free_page(desc.root_page)?;
        self.catalog.flush(&mut self.manager)?;
        Ok(())
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.flush();
        let _ = self.manager.close();
    }
}

impl BTreeStore for Database {
    fn read_page(&mut self, pid: PageId) -> Result<Page> {
        Database::read_page(self, pid)
    }

    fn write_page(&mut self, pid: PageId, page: &Page) -> Result<()> {
        let fid = self.pool.fetch(&mut self.manager, pid)?;
        {
            let p = self.pool.get_mut(fid)?;
            p.payload.copy_from_slice(&page.payload);
            p.header.lsn = page.header.lsn;
        }
        self.pool.unpin(fid)?;
        Ok(())
    }

    fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        self.manager.allocate_page(page_type)
    }

    fn page_size(&self) -> usize {
        self.manager.page_size()
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
