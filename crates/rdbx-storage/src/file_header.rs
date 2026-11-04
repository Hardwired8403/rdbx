//! File-level header stored in page 0.

use crate::page::{PAGE_HEADER_SIZE, Page, PageType};
use rdbx_core::{Error, Lsn, PageId, Result};

/// Magic bytes at the start of the file-header payload.
pub const MAGIC: &[u8; 8] = b"RDBXFILE";
/// Current format version (major).
pub const FORMAT_VERSION_MAJOR: u16 = 1;
/// Current format version (minor).
pub const FORMAT_VERSION_MINOR: u16 = 0;

/// Size of the serialized `FileHeader` payload (excluding `PageHeader`).
pub const FILE_HEADER_PAYLOAD_SIZE: usize = 80;

/// Persistent database file header.
///
/// Serialized format (little-endian, within the page payload):
///
/// ```text
/// offset  size  field
///   0      8    magic : b"RDBXFILE"
///   8      2    format_major : u16
///  10      2    format_minor : u16
///  12      4    page_size : u32
///  16     16    file_uuid : [u8; 16]
///  32      8    created_at_unix_nanos : u64
///  40      8    page_count : u64
///  48      8    free_list_head : u64
///  56      8    free_page_count : u64
///  64      8    checkpoint_lsn : u64
///  72      8    reserved : [u8; 8]
/// ```
#[derive(Debug, Clone)]
pub struct FileHeader {
    pub format_major: u16,
    pub format_minor: u16,
    pub page_size: u32,
    pub file_uuid: [u8; 16],
    pub created_at_unix_nanos: u64,
    pub page_count: u64,
    pub free_list_head: u64,
    pub free_page_count: u64,
    pub checkpoint_lsn: Lsn,
}

impl FileHeader {
    pub fn new(page_size: u32, file_uuid: [u8; 16]) -> Self {
        Self {
            format_major: FORMAT_VERSION_MAJOR,
            format_minor: FORMAT_VERSION_MINOR,
            page_size,
            file_uuid,
            created_at_unix_nanos: 0,
            page_count: 2, // pages 0 and 1 are reserved
            free_list_head: 0,
            free_page_count: 0,
            checkpoint_lsn: Lsn(0),
        }
    }

    fn encode_payload(&self) -> [u8; FILE_HEADER_PAYLOAD_SIZE] {
        let mut out = [0u8; FILE_HEADER_PAYLOAD_SIZE];
        out[0..8].copy_from_slice(MAGIC);
        out[8..10].copy_from_slice(&self.format_major.to_le_bytes());
        out[10..12].copy_from_slice(&self.format_minor.to_le_bytes());
        out[12..16].copy_from_slice(&self.page_size.to_le_bytes());
        out[16..32].copy_from_slice(&self.file_uuid);
        out[32..40].copy_from_slice(&self.created_at_unix_nanos.to_le_bytes());
        out[40..48].copy_from_slice(&self.page_count.to_le_bytes());
        out[48..56].copy_from_slice(&self.free_list_head.to_le_bytes());
        out[56..64].copy_from_slice(&self.free_page_count.to_le_bytes());
        out[64..72].copy_from_slice(&self.checkpoint_lsn.0.to_le_bytes());
        out
    }

    fn decode_payload(payload: &[u8]) -> Result<Self> {
        if payload.len() < FILE_HEADER_PAYLOAD_SIZE {
            return Err(Error::InvalidFile("file header truncated".into()));
        }
        if &payload[0..8] != MAGIC {
            return Err(Error::InvalidFile("bad magic (not an RDBX file)".into()));
        }
        let format_major = u16::from_le_bytes(payload[8..10].try_into().unwrap());
        let format_minor = u16::from_le_bytes(payload[10..12].try_into().unwrap());
        if format_major != FORMAT_VERSION_MAJOR {
            return Err(Error::UnsupportedVersion {
                major: format_major,
                minor: format_minor,
                supported_major: FORMAT_VERSION_MAJOR,
                supported_minor: FORMAT_VERSION_MINOR,
            });
        }
        let page_size = u32::from_le_bytes(payload[12..16].try_into().unwrap());
        let mut file_uuid = [0u8; 16];
        file_uuid.copy_from_slice(&payload[16..32]);
        let created_at_unix_nanos = u64::from_le_bytes(payload[32..40].try_into().unwrap());
        let page_count = u64::from_le_bytes(payload[40..48].try_into().unwrap());
        let free_list_head = u64::from_le_bytes(payload[48..56].try_into().unwrap());
        let free_page_count = u64::from_le_bytes(payload[56..64].try_into().unwrap());
        let checkpoint_lsn = Lsn(u64::from_le_bytes(payload[64..72].try_into().unwrap()));
        Ok(Self {
            format_major,
            format_minor,
            page_size,
            file_uuid,
            created_at_unix_nanos,
            page_count,
            free_list_head,
            free_page_count,
            checkpoint_lsn,
        })
    }

    /// Serialize into a full page.
    pub fn to_page(&self) -> Result<Page> {
        let payload = self.encode_payload();
        let mut page = Page::new(
            PageId::HEADER,
            PageType::FileHeader,
            self.page_size as usize,
        );
        page.payload[..FILE_HEADER_PAYLOAD_SIZE].copy_from_slice(&payload);
        Ok(page)
    }

    /// Deserialize from a full page.
    pub fn from_page(page: &Page) -> Result<Self> {
        if page.header.page_type != PageType::FileHeader {
            return Err(Error::InvalidFile(
                "page 0 is not marked as a file header".into(),
            ));
        }
        Self::decode_payload(&page.payload)
    }
}

// Compile-time sanity check: header fits comfortably in the smallest page.
const _: () = assert!(PAGE_HEADER_SIZE + FILE_HEADER_PAYLOAD_SIZE <= 4096);
