//! Page layout and serialization.
//!
//! Wire format (little-endian):
//!
//! ```text
//! offset  size  field
//!   0      4    checksum : u32     (CRC-32 of bytes 4..page_size)
//!   4      8    lsn      : u64
//!  12      8    page_id  : u64
//!  20      1    page_type: u8
//!  21      1    flags    : u8
//!  22      2    reserved : u16
//!  24    …      payload  : [u8; page_size - 24]
//! ```

use crate::checksum::crc32;
use rdbx_core::{Error, Lsn, PageId, Result};

/// Size of the page header in bytes.
pub const PAGE_HEADER_SIZE: usize = 24;

/// Page type tag stored in the header.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PageType {
    /// Free page (linked into the free list).
    Free = 0,
    /// File header (only page 0).
    FileHeader = 1,
    /// Metadata page (reserved for catalog).
    Meta = 2,
    /// Table data page.
    Data = 3,
    /// Index page.
    Index = 4,
    /// Overflow page for large values.
    Overflow = 5,
}

impl PageType {
    pub fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => PageType::Free,
            1 => PageType::FileHeader,
            2 => PageType::Meta,
            3 => PageType::Data,
            4 => PageType::Index,
            5 => PageType::Overflow,
            other => {
                return Err(Error::InvalidFile(format!("unknown page type tag {other}")));
            }
        })
    }
}

/// The 24-byte header at the beginning of every page.
#[derive(Copy, Clone, Debug)]
pub struct PageHeader {
    pub lsn: Lsn,
    pub page_id: PageId,
    pub page_type: PageType,
    pub flags: u8,
}

impl PageHeader {
    pub fn encode(&self, buf: &mut [u8], page_size: usize) -> Result<()> {
        if buf.len() < page_size || page_size < PAGE_HEADER_SIZE {
            return Err(Error::Internal("page buffer too small".into()));
        }
        buf[0..4].fill(0); // checksum placeholder, filled by Page::encode
        buf[4..12].copy_from_slice(&self.lsn.0.to_le_bytes());
        buf[12..20].copy_from_slice(&self.page_id.0.to_le_bytes());
        buf[20] = self.page_type as u8;
        buf[21] = self.flags;
        buf[22..24].fill(0);
        Ok(())
    }

    pub fn decode(buf: &[u8], page_size: usize) -> Result<Self> {
        if buf.len() < page_size || page_size < PAGE_HEADER_SIZE {
            return Err(Error::Internal("page buffer too small".into()));
        }
        let lsn = Lsn(u64::from_le_bytes(buf[4..12].try_into().unwrap()));
        let page_id = PageId(u64::from_le_bytes(buf[12..20].try_into().unwrap()));
        let page_type = PageType::from_u8(buf[20])?;
        let flags = buf[21];
        Ok(PageHeader {
            lsn,
            page_id,
            page_type,
            flags,
        })
    }
}

/// An in-memory page: header + payload.
#[derive(Clone, Debug)]
pub struct Page {
    pub header: PageHeader,
    pub payload: Vec<u8>,
}

impl Page {
    /// Create a zero-filled page of the given type.
    pub fn new(page_id: PageId, page_type: PageType, page_size: usize) -> Self {
        Self {
            header: PageHeader {
                lsn: Lsn(0),
                page_id,
                page_type,
                flags: 0,
            },
            payload: vec![0u8; page_size - PAGE_HEADER_SIZE],
        }
    }

    /// Serialize the page to a `page_size`-byte buffer with a valid checksum.
    pub fn encode(&self, page_size: usize) -> Result<Vec<u8>> {
        if self.payload.len() + PAGE_HEADER_SIZE != page_size {
            return Err(Error::Internal(format!(
                "payload size {} + header {} != page_size {}",
                self.payload.len(),
                PAGE_HEADER_SIZE,
                page_size
            )));
        }
        let mut buf = vec![0u8; page_size];
        self.header.encode(&mut buf, page_size)?;
        buf[PAGE_HEADER_SIZE..].copy_from_slice(&self.payload);
        let checksum = crc32(&buf[4..]);
        buf[0..4].copy_from_slice(&checksum.to_le_bytes());
        Ok(buf)
    }

    /// Deserialize and verify checksum. `page_id` is used only for error reporting.
    pub fn decode(buf: &[u8], page_size: usize, page_id: PageId) -> Result<Self> {
        if buf.len() != page_size {
            return Err(Error::Internal(format!(
                "page buffer size {} != expected {}",
                buf.len(),
                page_size
            )));
        }
        if page_size < PAGE_HEADER_SIZE {
            return Err(Error::Internal("page_size < PAGE_HEADER_SIZE".into()));
        }
        let stored = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let computed = crc32(&buf[4..]);
        if stored != computed {
            return Err(Error::ChecksumMismatch {
                page_id: page_id.0,
                expected: stored,
                actual: computed,
            });
        }
        let header = PageHeader::decode(buf, page_size)?;
        let payload = buf[PAGE_HEADER_SIZE..].to_vec();
        Ok(Page { header, payload })
    }
}
