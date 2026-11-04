//! Single-writer WAL-backed transactions.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use rdbx_btree::BTreeStore;
use rdbx_core::{Error, Lsn, PageId, Result, TxnId};
use rdbx_storage::{PAGE_HEADER_SIZE, Page, PageType};
use rdbx_wal::RecordKind;

use crate::database::Database;

/// One in-flight transaction. Dropping without `commit` or `rollback` is a
/// programmer error; the WAL will still keep the transaction uncommitted and
/// recovery will undo it on the next open.
pub struct Transaction<'a> {
    db: &'a mut Database,
    txn_id: TxnId,
    written_pages: Vec<PageId>,
    before_images: HashMap<PageId, Vec<u8>>,
    active: bool,
}

impl<'a> Transaction<'a> {
    pub(crate) fn new(db: &'a mut Database, txn_id: TxnId) -> Self {
        Self {
            db,
            txn_id,
            written_pages: Vec::new(),
            before_images: HashMap::new(),
            active: true,
        }
    }

    pub fn txn_id(&self) -> TxnId {
        self.txn_id
    }

    /// Read a page through the buffer pool.
    pub fn read_page(&mut self, pid: PageId) -> Result<Page> {
        self.db.read_page(pid)
    }

    /// Allocate a page. The allocation itself is **not** WAL-logged in
    /// Phase 4: the zeroed page reaches disk immediately, and any
    /// subsequent `write_page` records its before/after images normally.
    /// If the transaction aborts, allocated pages may be orphaned; this
    /// is a known Phase 4 limitation (transactional allocation lands in
    /// Phase 5 with MVCC).
    pub fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        self.db.manager.allocate_page(page_type)
    }

    /// Write a page. See `BTreeStore` integration for the trait form.
    pub fn write_page(&mut self, pid: PageId, new_page: &Page) -> Result<()> {
        self.write_page_impl(pid, new_page)
    }

    fn write_page_impl(&mut self, pid: PageId, new_page: &Page) -> Result<()> {
        if !self.active {
            return Err(Error::Internal("transaction is no longer active".into()));
        }

        let fid = self.db.pool.fetch(&mut self.db.manager, pid)?;
        let (actual_pid, before) = {
            let current = self.db.pool.get(fid)?;
            (current.header.page_id, current.payload.clone())
        };
        self.db.pool.unpin(fid)?;
        if actual_pid != pid {
            return Err(Error::Internal(format!(
                "page id mismatch: expected {pid}, got {actual_pid}"
            )));
        }

        let page_size = self.db.manager.page_size();
        if new_page.payload.len() != page_size - PAGE_HEADER_SIZE {
            return Err(Error::Internal(format!(
                "new page payload size {} != page_size - header",
                new_page.payload.len()
            )));
        }
        let after = new_page.payload.clone();

        let lsn = self.db.wal.append(
            self.txn_id,
            RecordKind::Update,
            pid,
            before.clone(),
            after.clone(),
        )?;

        if let Entry::Vacant(slot) = self.before_images.entry(pid) {
            slot.insert(before);
            self.written_pages.push(pid);
        }

        let fid = self.db.pool.fetch(&mut self.db.manager, pid)?;
        {
            let p = self.db.pool.get_mut(fid)?;
            if p.payload.len() != after.len() {
                self.db.pool.unpin(fid)?;
                return Err(Error::Internal(
                    "inconsistent payload size while applying write".into(),
                ));
            }
            p.payload.copy_from_slice(&after);
            p.header.lsn = lsn;
        }
        self.db.pool.unpin(fid)?;
        Ok(())
    }

    /// Commit: WAL flush → buffer flush → manager flush → Commit record.
    pub fn commit(mut self) -> Result<()> {
        if !self.active {
            return Err(Error::Internal("transaction is no longer active".into()));
        }
        self.db.wal.flush()?;
        self.db.pool.flush_all(&mut self.db.manager)?;
        self.db.manager.flush()?;
        self.db.wal.append(
            self.txn_id,
            RecordKind::Commit,
            PageId(0),
            Vec::new(),
            Vec::new(),
        )?;
        self.db.wal.flush()?;
        self.active = false;
        Ok(())
    }

    /// Rollback: undo in memory, append Abort, drop dirty frames.
    pub fn rollback(mut self) -> Result<()> {
        if !self.active {
            return Err(Error::Internal("transaction is no longer active".into()));
        }
        for pid in self.written_pages.iter().rev() {
            let Some(before) = self.before_images.get(pid) else {
                continue;
            };
            let fid = self.db.pool.fetch(&mut self.db.manager, *pid)?;
            {
                let p = self.db.pool.get_mut(fid)?;
                if p.payload.len() == before.len() {
                    p.payload.copy_from_slice(before);
                }
                p.header.lsn = Lsn(0);
            }
            self.db.pool.unpin(fid)?;
        }
        self.db.wal.append(
            self.txn_id,
            RecordKind::Abort,
            PageId(0),
            Vec::new(),
            Vec::new(),
        )?;
        self.db.wal.flush()?;
        self.active = false;
        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.active {
            let written: Vec<PageId> = self.written_pages.clone();
            for pid in written.iter().rev() {
                let Some(before) = self.before_images.get(pid) else {
                    continue;
                };
                if let Ok(fid) = self.db.pool.fetch(&mut self.db.manager, *pid) {
                    if let Ok(p) = self.db.pool.get_mut(fid)
                        && p.payload.len() == before.len()
                    {
                        p.payload.copy_from_slice(before);
                        p.header.lsn = Lsn(0);
                    }
                    let _ = self.db.pool.unpin(fid);
                }
            }
            let _ = self.db.wal.append(
                self.txn_id,
                RecordKind::Abort,
                PageId(0),
                Vec::new(),
                Vec::new(),
            );
            let _ = self.db.wal.flush();
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
        self.db.manager.allocate_page(page_type)
    }

    fn page_size(&self) -> usize {
        self.db.manager.page_size()
    }
}
