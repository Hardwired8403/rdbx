//! Frame: one slot in the buffer pool.

use rdbx_core::PageId;
use rdbx_storage::{Page, PageType};

/// Identifier of a frame inside a [`super::BufferPool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(pub u32);

impl FrameId {
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// One cached page.
pub struct Frame {
    /// `None` when the frame is unused (fresh or just evicted).
    pub page_id: Option<PageId>,
    /// Decoded page contents.
    pub data: Page,
    /// Number of outstanding pins. Must be 0 for the frame to be evictable.
    pub pin_count: u32,
    /// Whether the frame content differs from what is on disk.
    pub dirty: bool,
}

impl Frame {
    pub fn empty(page_size: usize) -> Self {
        Self {
            page_id: None,
            data: Page::new(PageId(0), PageType::Free, page_size),
            pin_count: 0,
            dirty: false,
        }
    }
}
