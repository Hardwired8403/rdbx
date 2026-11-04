//! Fundamental identifier types.

use std::fmt;

/// Page identifier. Page 0 is the file header, page 1 is metadata.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(pub u64);

impl PageId {
    /// Reserved file-header page.
    pub const HEADER: PageId = PageId(0);
    /// Reserved metadata page.
    pub const META: PageId = PageId(1);

    #[inline]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Byte offset of the page in the database file.
    #[inline]
    pub fn offset(self, page_size: u64) -> u64 {
        self.0 * page_size
    }
}

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "page#{}", self.0)
    }
}

/// Log sequence number (used by the WAL; Phase 1 only stores it).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Lsn(pub u64);

/// Transaction identifier (used by MVCC; Phase 1 only stores it).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxnId(pub u64);
