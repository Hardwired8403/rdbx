//! Page-oriented persistent storage for RDBX.
//!
//! On-disk layout:
//!
//! ```text
//! page 0 : FileHeader  (magic, version, page size, page count, free list …)
//! page 1 : Meta        (catalog: list of table descriptors)
//! page 2…: Data / Index / Overflow / Free pages
//! ```
//!
//! Every page is `page_size` bytes and begins with a 24-byte [`PageHeader`].
//! The first 4 bytes of the header are a CRC-32 of the rest of the page.

pub mod catalog;
pub mod checksum;
pub mod file_header;
pub mod manager;
pub mod page;

pub use catalog::{Catalog, MAX_TABLE_NAME_LEN, TableDescriptor};
pub use file_header::{FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR, FileHeader, MAGIC};
pub use manager::PageManager;
pub use page::{PAGE_HEADER_SIZE, Page, PageHeader, PageType};
