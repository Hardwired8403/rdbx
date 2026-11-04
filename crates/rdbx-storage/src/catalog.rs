//! Catalog (persistent schema) stored in page 1.
//!
//! Phase 2 stores an ordered list of table descriptors. Each descriptor
//! points to a *root page*; in Phase 4 this root becomes a B+Tree root,
//! and in Phase 7 the schema grows to include columns, types, and indexes.
//!
//! On-disk payload layout (little-endian, within the meta page payload):
//!
//! ```text
//! offset  size  field
//!   0      8    catalog_version : u64    (incremented on every flush)
//!   8      4    table_count     : u32
//!  12      4    reserved        : u32     (must be 0)
//!  16    …      entries, each:
//!                 offset  size  field
//!                   0      8    table_id    : u64
//!                   8      8    root_page   : u64
//!                  16     16    schema_hash : [u8; 16]
//!                  32      2    name_len    : u16
//!                  34      N    name_utf8   : [u8; name_len]
//! ```

use crate::manager::PageManager;
use crate::page::{PAGE_HEADER_SIZE, Page, PageType};
use rdbx_core::{Error, PageId, Result};

/// Size of the fixed catalog payload header.
pub const CATALOG_PAYLOAD_HEADER_SIZE: usize = 16;
/// Fixed size of one table entry (excluding variable-length name).
pub const TABLE_ENTRY_FIXED_SIZE: usize = 34;
/// Maximum length of a table name in bytes.
pub const MAX_TABLE_NAME_LEN: usize = 255;

/// A single table descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDescriptor {
    pub id: u64,
    pub name: String,
    pub root_page: PageId,
    /// Placeholder for future schema identification (0 in Phase 2).
    pub schema_hash: [u8; 16],
}

/// In-memory catalog of tables. Serialized to / from page 1.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub version: u64,
    pub next_table_id: u64,
    pub tables: Vec<TableDescriptor>,
}

impl Catalog {
    /// Load the catalog from page 1. Treats an all-zero page (a fresh
    /// database before the first flush) as an empty catalog.
    pub fn load(mgr: &mut PageManager) -> Result<Self> {
        let page = mgr.read_page(PageId::META)?;
        Self::decode(&page, mgr.page_size())
    }

    /// Serialize and write the catalog to page 1.
    pub fn flush(&mut self, mgr: &mut PageManager) -> Result<()> {
        self.version = self.version.saturating_add(1);
        let payload = self.encode(mgr.page_size())?;
        let mut page = Page::new(PageId::META, PageType::Meta, mgr.page_size());
        page.payload[..payload.len()].copy_from_slice(&payload);
        mgr.write_page(&page)?;
        Ok(())
    }

    pub fn table_by_name(&self, name: &str) -> Option<&TableDescriptor> {
        self.tables.iter().find(|t| t.name == name)
    }

    pub fn insert(&mut self, desc: TableDescriptor) {
        self.next_table_id = self.next_table_id.max(desc.id.saturating_add(1));
        self.tables.push(desc);
    }

    pub fn remove_by_name(&mut self, name: &str) -> Option<TableDescriptor> {
        let idx = self.tables.iter().position(|t| t.name == name)?;
        Some(self.tables.remove(idx))
    }

    fn encode(&self, page_size: usize) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(page_size - PAGE_HEADER_SIZE);
        buf.extend_from_slice(&self.version.to_le_bytes());
        let count = u32::try_from(self.tables.len())
            .map_err(|_| Error::Internal("too many tables".into()))?;
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        let capacity = page_size - PAGE_HEADER_SIZE;
        for t in &self.tables {
            if t.name.len() > MAX_TABLE_NAME_LEN {
                return Err(Error::Internal(format!(
                    "table name too long: {} bytes (max {MAX_TABLE_NAME_LEN})",
                    t.name.len()
                )));
            }
            if buf.len() + TABLE_ENTRY_FIXED_SIZE + t.name.len() > capacity {
                return Err(Error::Internal(
                    "catalog page overflow (too many tables)".into(),
                ));
            }
            buf.extend_from_slice(&t.id.to_le_bytes());
            buf.extend_from_slice(&t.root_page.0.to_le_bytes());
            buf.extend_from_slice(&t.schema_hash);
            let name_len = t.name.len() as u16;
            buf.extend_from_slice(&name_len.to_le_bytes());
            buf.extend_from_slice(t.name.as_bytes());
        }
        Ok(buf)
    }

    fn decode(page: &Page, page_size: usize) -> Result<Self> {
        if page.header.page_type != PageType::Meta {
            return Err(Error::InvalidFile("page 1 is not a Meta page".into()));
        }
        let buf = &page.payload;
        if buf.len() < CATALOG_PAYLOAD_HEADER_SIZE {
            return Err(Error::InvalidFile("catalog page too small".into()));
        }
        if buf.iter().all(|&b| b == 0) {
            return Ok(Catalog::default());
        }
        let version = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let count = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
        let capacity = page_size - PAGE_HEADER_SIZE;
        if count > capacity / TABLE_ENTRY_FIXED_SIZE {
            return Err(Error::InvalidFile(format!(
                "catalog table_count {count} exceeds capacity"
            )));
        }
        let mut off = CATALOG_PAYLOAD_HEADER_SIZE;
        let mut tables = Vec::with_capacity(count);
        let mut next_id = 1u64;
        for _ in 0..count {
            if off + TABLE_ENTRY_FIXED_SIZE > buf.len() {
                return Err(Error::InvalidFile("catalog entry truncated".into()));
            }
            let id = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
            let root = PageId(u64::from_le_bytes(
                buf[off + 8..off + 16].try_into().unwrap(),
            ));
            let mut schema_hash = [0u8; 16];
            schema_hash.copy_from_slice(&buf[off + 16..off + 32]);
            let name_len = u16::from_le_bytes(buf[off + 32..off + 34].try_into().unwrap()) as usize;
            if name_len > MAX_TABLE_NAME_LEN {
                return Err(Error::InvalidFile(format!(
                    "catalog table name_len {name_len} exceeds limit"
                )));
            }
            let name_end = off + TABLE_ENTRY_FIXED_SIZE + name_len;
            if name_end > buf.len() {
                return Err(Error::InvalidFile("catalog name truncated".into()));
            }
            let name = std::str::from_utf8(&buf[off + 34..name_end])
                .map_err(|_| Error::InvalidFile("catalog name is not UTF-8".into()))?
                .to_string();
            tables.push(TableDescriptor {
                id,
                name,
                root_page: root,
                schema_hash,
            });
            next_id = next_id.max(id.saturating_add(1));
            off = name_end;
        }
        Ok(Catalog {
            version,
            next_table_id: next_id,
            tables,
        })
    }
}
