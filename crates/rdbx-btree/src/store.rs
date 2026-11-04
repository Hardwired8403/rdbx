//! Abstraction over the page storage used by the B+Tree.
//!
//! Implemented in `rdbx` by `Transaction` (WAL-backed writes) and by
//! `Database` (direct writes; used only internally by `create_table`).

use rdbx_core::{PageId, Result};
use rdbx_storage::{Page, PageType};

/// Page-level operations required by the B+Tree.
pub trait BTreeStore {
    fn read_page(&mut self, pid: PageId) -> Result<Page>;
    fn write_page(&mut self, pid: PageId, page: &Page) -> Result<()>;
    fn allocate_page(&mut self, page_type: PageType) -> Result<PageId>;
    fn page_size(&self) -> usize;
}
