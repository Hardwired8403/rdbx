//! Page manager: allocate, free, read, write, flush.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rdbx_core::{
    DatabaseOptions, Error, Lsn, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PageId, Result, SyncMode,
};
use uuid::Uuid;

use crate::file_header::{FileHeader, MAGIC};
use crate::page::{PAGE_HEADER_SIZE, Page, PageType};

/// Pages 0 and 1 are reserved and cannot be freed.
const RESERVED_PAGES: u64 = 2;

/// Manages the persistent database file and page lifecycle.
pub struct PageManager {
    file: File,
    path: PathBuf,
    page_size: usize,
    header: FileHeader,
    sync_mode: SyncMode,
    closed: bool,
}

impl PageManager {
    /// Create a brand-new database file at `path`.
    pub fn create(path: impl AsRef<Path>, opts: &DatabaseOptions) -> Result<Self> {
        opts.validate()?;
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            return Err(Error::InvalidFile(format!(
                "refusing to overwrite existing file: {}",
                path.display()
            )));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let file_uuid = *Uuid::new_v4().as_bytes();
        let mut header = FileHeader::new(opts.page_size as u32, file_uuid);
        header.created_at_unix_nanos = unix_nanos_now();

        let mut pm = PageManager {
            file,
            path,
            page_size: opts.page_size,
            header,
            sync_mode: opts.sync_mode,
            closed: false,
        };
        // Persist page 0 (file header).
        pm.flush_file_header()?;
        // Persist page 1 (metadata page, currently zeroed).
        let meta = Page::new(PageId::META, PageType::Meta, opts.page_size);
        pm.write_page(&meta)?;
        pm.flush()?;
        Ok(pm)
    }

    /// Open an existing database file.
    pub fn open(path: impl AsRef<Path>, opts: &DatabaseOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("database file not found: {}", path.display()),
            )));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(!opts.read_only)
            .open(&path)?;

        // Probe the first 128 bytes to determine page size and verify magic.
        // File offset 24 = PAGE_HEADER_SIZE → payload starts there.
        let mut probe = [0u8; 128];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut probe).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                Error::InvalidFile("file too small to be an RDBX database".into())
            } else {
                Error::Io(e)
            }
        })?;
        if &probe[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 8] != MAGIC {
            return Err(Error::InvalidFile("bad magic (not an RDBX file)".into()));
        }
        let disk_page_size = u32::from_le_bytes(
            probe[PAGE_HEADER_SIZE + 12..PAGE_HEADER_SIZE + 16]
                .try_into()
                .unwrap(),
        ) as usize;
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&disk_page_size)
            || !disk_page_size.is_power_of_two()
        {
            return Err(Error::InvalidFile(format!(
                "invalid on-disk page size {disk_page_size}"
            )));
        }

        // Read page 0 with the correct page size and decode the header.
        let mut header_buf = vec![0u8; disk_page_size];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header_buf)?;
        let header_page = Page::decode(&header_buf, disk_page_size, PageId::HEADER)?;
        let header = FileHeader::from_page(&header_page)?;

        Ok(PageManager {
            file,
            path,
            page_size: disk_page_size,
            header,
            sync_mode: opts.sync_mode,
            closed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn page_size(&self) -> usize {
        self.page_size
    }
    pub fn page_count(&self) -> u64 {
        self.header.page_count
    }
    pub fn free_page_count(&self) -> u64 {
        self.header.free_page_count
    }
    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Allocate a page, reusing from the free list if available.
    pub fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        if self.header.free_list_head != 0 {
            let pid = PageId(self.header.free_list_head);
            let page = self.read_page(pid)?;
            if page.header.page_type != PageType::Free {
                return Err(Error::Internal(format!(
                    "free list head {pid} is not a free page"
                )));
            }
            let next = u64::from_le_bytes(page.payload[0..8].try_into().unwrap());
            self.header.free_list_head = next;
            self.header.free_page_count = self.header.free_page_count.saturating_sub(1);
            self.flush_file_header()?;
            let fresh = Page::new(pid, page_type, self.page_size);
            self.write_page(&fresh)?;
            return Ok(pid);
        }

        let pid = PageId(self.header.page_count);
        self.header.page_count = self
            .header
            .page_count
            .checked_add(1)
            .ok_or(Error::Overflow("page_count"))?;
        self.flush_file_header()?;
        let page = Page::new(pid, page_type, self.page_size);
        self.write_page(&page)?;
        Ok(pid)
    }

    /// Free a page and link it into the free list.
    pub fn free_page(&mut self, pid: PageId) -> Result<()> {
        if pid.0 < RESERVED_PAGES {
            return Err(Error::Internal(format!("cannot free reserved page {pid}")));
        }
        if pid.0 >= self.header.page_count {
            return Err(Error::PageOutOfBounds(pid.0));
        }
        let existing = self.read_page(pid)?;
        if existing.header.page_type == PageType::Free {
            return Ok(()); // idempotent
        }
        let mut free_page = Page::new(pid, PageType::Free, self.page_size);
        free_page.payload[0..8].copy_from_slice(&self.header.free_list_head.to_le_bytes());
        self.write_page(&free_page)?;
        self.header.free_list_head = pid.0;
        self.header.free_page_count = self
            .header
            .free_page_count
            .checked_add(1)
            .ok_or(Error::Overflow("free_page_count"))?;
        self.flush_file_header()?;
        Ok(())
    }

    /// Read a page by id and verify it.
    pub fn read_page(&mut self, pid: PageId) -> Result<Page> {
        if pid.0 >= self.header.page_count {
            return Err(Error::PageOutOfBounds(pid.0));
        }
        let raw = self.read_raw_page(pid)?;
        let page = Page::decode(&raw, self.page_size, pid)?;
        if page.header.page_id != pid {
            return Err(Error::InvalidFile(format!(
                "page id mismatch: expected {pid}, got {}",
                page.header.page_id
            )));
        }
        Ok(page)
    }

    /// Write a page. The page's `page_id` must already exist.
    pub fn write_page(&mut self, page: &Page) -> Result<()> {
        let pid = page.header.page_id;
        if pid.0 >= self.header.page_count {
            return Err(Error::PageOutOfBounds(pid.0));
        }
        let raw = page.encode(self.page_size)?;
        self.write_raw_page(pid, &raw)?;
        Ok(())
    }

    /// Flush OS buffers according to `sync_mode`.
    pub fn flush(&mut self) -> Result<()> {
        match self.sync_mode {
            SyncMode::Full | SyncMode::Normal => self.file.sync_all()?,
            SyncMode::Off => self.file.flush()?,
        }
        Ok(())
    }

    /// Gracefully close the database.
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.flush()?;
        self.closed = true;
        Ok(())
    }

    // --- internals ---

    fn flush_file_header(&mut self) -> Result<()> {
        let page = self.header.to_page()?;
        let raw = page.encode(self.page_size)?;
        self.write_raw_page(PageId::HEADER, &raw)?;
        Ok(())
    }

    fn read_raw_page(&mut self, pid: PageId) -> Result<Vec<u8>> {
        let offset = pid
            .0
            .checked_mul(self.page_size as u64)
            .ok_or(Error::Overflow("page offset"))?;
        let mut buf = vec![0u8; self.page_size];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn write_raw_page(&mut self, pid: PageId, data: &[u8]) -> Result<()> {
        let offset = pid
            .0
            .checked_mul(self.page_size as u64)
            .ok_or(Error::Overflow("page offset"))?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)?;
        Ok(())
    }
}

impl Drop for PageManager {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn unix_nanos_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// Silence "unused" warnings when only some Lsn helpers are used in Phase 1.
#[allow(dead_code)]
fn _lsn_helper(lsn: Lsn) -> u64 {
    lsn.0
}
