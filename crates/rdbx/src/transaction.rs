//! Multi-writer WAL-backed transactions with MVCC snapshot isolation.

use std::collections::{HashMap, hash_map::Entry};

use rdbx_btree::BTreeStore;
use rdbx_core::{Error, Lsn, PageId, Result, TxnId};
use rdbx_mvcc::{Snapshot, SnapshotHandle, Visible};
use rdbx_storage::{PAGE_HEADER_SIZE, Page, PageType};
use rdbx_wal::RecordKind;

use crate::database::Database;

/// One in-flight transaction. Borrows the database shared, so multiple
/// transactions can be opened concurrently from the same thread.
pub struct Transaction<'db> {
    db: &'db Database,
    handle: SnapshotHandle,
    written_pages: Vec<PageId>,
    before_images: HashMap<PageId, Vec<u8>>,
    active: bool,
}

impl<'db> Transaction<'db> {
    pub(crate) fn new(db: &'db Database, handle: SnapshotHandle) -> Self {
        Self {
            db,
            handle,
            written_pages: Vec::new(),
            before_images: HashMap::new(),
            active: true,
        }
    }

    pub fn txn_id(&self) -> TxnId {
        self.handle.txn_id
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.handle.snapshot
    }

    /// Read a page through the buffer pool.
    pub fn read_page(&mut self, pid: PageId) -> Result<Page> {
        self.db.read_page(pid)
    }

    pub fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        let mut mgr = self.db.manager.lock().unwrap();
        mgr.allocate_page(page_type)
    }

    pub fn write_page(&mut self, pid: PageId, new_page: &Page) -> Result<()> {
        self.write_page_impl(pid, new_page)
    }

    fn write_page_impl(&mut self, pid: PageId, new_page: &Page) -> Result<()> {
        if !self.active {
            return Err(Error::Internal("transaction is no longer active".into()));
        }

        let mut mgr = self.db.manager.lock().unwrap();
        let mut pool = self.db.pool.lock().unwrap();

        let fid = pool.fetch(&mut mgr, pid)?;
        let (actual_pid, before) = {
            let current = pool.get(fid)?;
            (current.header.page_id, current.payload.clone())
        };
        pool.unpin(fid)?;
        if actual_pid != pid {
            return Err(Error::Internal(format!(
                "page id mismatch: expected {pid}, got {actual_pid}"
            )));
        }

        let page_size = mgr.page_size();
        if new_page.payload.len() != page_size - PAGE_HEADER_SIZE {
            return Err(Error::Internal(format!(
                "new page payload size {} != page_size - header",
                new_page.payload.len()
            )));
        }
        let after = new_page.payload.clone();
        drop(pool);
        drop(mgr);

        // WAL the update.
        let mut wal = self.db.wal.lock().unwrap();
        let lsn = wal.append(
            self.handle.txn_id,
            RecordKind::Update,
            pid,
            before.clone(),
            after.clone(),
        )?;
        drop(wal);

        if let Entry::Vacant(slot) = self.before_images.entry(pid) {
            slot.insert(before);
            self.written_pages.push(pid);
        }

        // Apply to buffer pool.
        let mut mgr = self.db.manager.lock().unwrap();
        let mut pool = self.db.pool.lock().unwrap();
        let fid = pool.fetch(&mut mgr, pid)?;
        {
            let p = pool.get_mut(fid)?;
            if p.payload.len() != after.len() {
                pool.unpin(fid)?;
                return Err(Error::Internal(
                    "inconsistent payload size while applying write".into(),
                ));
            }
            p.payload.copy_from_slice(&after);
            p.header.lsn = lsn;
        }
        pool.unpin(fid)?;
        Ok(())
    }

    // --- MVCC convenience ---

    pub fn mvcc_read(&mut self, key: &[u8]) -> Result<Visible> {
        self.db
            .mvcc_read(&self.handle.snapshot, self.handle.txn_id, key)
    }

    pub fn mvcc_write(&mut self, key: Vec<u8>, value: Option<Vec<u8>>) -> Result<()> {
        self.db.mvcc_write(self.handle.txn_id, key, value)
    }

    /// Commit: WAL flush, MVCC commit (may fail with `WriteConflict`),
    /// buffer flush, Commit record.
    ///
    /// If MVCC conflict detection fires, the transaction is **not**
    /// committed and the caller can retry. All page writes recorded
    /// through this transaction are rolled back in the buffer pool.
    pub fn commit(mut self) -> Result<()> {
        if !self.active {
            return Err(Error::Internal("transaction is no longer active".into()));
        }

        // 1. WAL flush: all Update records for this transaction are durable.
        {
            let mut wal = self.db.wal.lock().unwrap();
            wal.flush()?;
        }

        // 2. MVCC commit (conflict detection + version store write).
        let mvcc_outcome = {
            let mut tm = self.db.tm.lock().unwrap();
            tm.commit(self.handle.txn_id, &self.handle.snapshot)
        };
        if let Err(commit_err) = mvcc_outcome {
            // Abort: restore pages, append Abort record.
            self.rollback_impl()?;
            return Err(Error::Internal(format!(
                "transaction aborted: {commit_err:?}"
            )));
        }

        // 3. Flush pages.
        {
            let mut mgr = self.db.manager.lock().unwrap();
            let mut pool = self.db.pool.lock().unwrap();
            pool.flush_all(&mut mgr)?;
            mgr.flush()?;
        }

        // 4. Commit record.
        {
            let mut wal = self.db.wal.lock().unwrap();
            wal.append(
                self.handle.txn_id,
                RecordKind::Commit,
                PageId(0),
                Vec::new(),
                Vec::new(),
            )?;
            wal.flush()?;
        }

        self.active = false;
        Ok(())
    }

    pub fn rollback(mut self) -> Result<()> {
        if !self.active {
            return Err(Error::Internal("transaction is no longer active".into()));
        }
        self.rollback_impl()?;
        self.active = false;
        Ok(())
    }

    fn rollback_impl(&mut self) -> Result<()> {
        let mut mgr = self.db.manager.lock().unwrap();
        let mut pool = self.db.pool.lock().unwrap();
        for pid in self.written_pages.iter().rev() {
            let Some(before) = self.before_images.get(pid) else {
                continue;
            };
            let fid = pool.fetch(&mut mgr, *pid)?;
            {
                let p = pool.get_mut(fid)?;
                if p.payload.len() == before.len() {
                    p.payload.copy_from_slice(before);
                }
                p.header.lsn = Lsn(0);
            }
            pool.unpin(fid)?;
        }
        drop(pool);
        drop(mgr);

        let mut tm = self.db.tm.lock().unwrap();
        let _ = tm.abort(self.handle.txn_id);
        drop(tm);

        let mut wal = self.db.wal.lock().unwrap();
        let _ = wal.append(
            self.handle.txn_id,
            RecordKind::Abort,
            PageId(0),
            Vec::new(),
            Vec::new(),
        );
        let _ = wal.flush();
        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.rollback_impl();
            self.active = false;
        }
    }
}

impl BTreeStore for Transaction<'_> {
    fn read_page(&mut self, pid: PageId) -> Result<Page> {
        self.db.read_page(pid)
    }

    fn write_page(&mut self, pid: PageId, page: &Page) -> Result<()> {
        self.write_page_impl(pid, page)
    }

    fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        let mut mgr = self.db.manager.lock().unwrap();
        mgr.allocate_page(page_type)
    }

    fn page_size(&self) -> usize {
        self.db.manager.lock().unwrap().page_size()
    }
}
