//! Top-level `Database` handle.

use std::path::{Path, PathBuf};

use rdbx_buffer::BufferPool;
use rdbx_core::{DatabaseOptions, Error, PageId, Result};
use rdbx_storage::{Catalog, PageManager, PageType, TableDescriptor};

/// The embedded database handle.
///
/// Owns the persistent [`PageManager`], the in-memory [`BufferPool`], and the
/// in-memory [`Catalog`]. Phase 2 does not yet expose transactions or SQL;
/// higher layers drive the storage subsystem through [`Database::storage`]
/// and the catalog helpers.
pub struct Database {
    path: PathBuf,
    manager: PageManager,
    pool: BufferPool,
    catalog: Catalog,
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

    /// Access the page manager directly (bypasses the buffer pool).
    ///
    /// Kept for Phase 1 compatibility and low-level tests; new code should
    /// use [`Database::storage`] to get both the manager and the pool.
    pub fn page_manager(&mut self) -> &mut PageManager {
        &mut self.manager
    }

    /// Borrow both the manager and the buffer pool simultaneously. This is
    /// the recommended way to drive the storage subsystem:
    ///
    /// ```ignore
    /// let (mgr, pool) = db.storage();
    /// let fid = pool.fetch(mgr, some_page_id)?;
    /// ```
    pub fn storage(&mut self) -> (&mut PageManager, &mut BufferPool) {
        (&mut self.manager, &mut self.pool)
    }

    /// Flush all dirty pages and the manager to disk.
    pub fn flush(&mut self) -> Result<()> {
        self.pool.flush_all(&mut self.manager)?;
        self.manager.flush()?;
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

    /// Create a table: allocate a root page and record the descriptor.
    /// Returns the root page id. The root page is currently just a zeroed
    /// data page; Phase 4 will replace it with an empty B+Tree root.
    pub fn create_table(&mut self, name: &str) -> Result<PageId> {
        if self.catalog.table_by_name(name).is_some() {
            return Err(Error::Internal(format!("table '{name}' already exists")));
        }
        let pid = self.manager.allocate_page(PageType::Data)?;
        let desc = TableDescriptor {
            id: self.catalog.next_table_id.max(1),
            name: name.to_string(),
            root_page: pid,
            schema_hash: [0u8; 16],
        };
        self.catalog.insert(desc);
        self.catalog.flush(&mut self.manager)?;
        Ok(pid)
    }

    /// Drop a table: free the root page and remove the descriptor.
    pub fn drop_table(&mut self, name: &str) -> Result<()> {
        let desc = self
            .catalog
            .remove_by_name(name)
            .ok_or_else(|| Error::Internal(format!("table '{name}' not found")))?;
        // If a cached copy exists, drop it before freeing the page.
        // (`free_page` would error on a pinned frame; caller's responsibility.)
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
