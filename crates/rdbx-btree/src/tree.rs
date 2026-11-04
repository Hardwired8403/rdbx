//! B+Tree operations: search, insert, delete, range.

use rdbx_core::{Error, PageId, Result};
use rdbx_storage::{PAGE_HEADER_SIZE, Page, PageType};

use crate::node::{InternalNode, LeafNode, Node};
use crate::store::BTreeStore;

/// A handle to a B+Tree whose root lives at `root`.
///
/// `insert` takes `&mut self` because top-level splits may change
/// `self.root`; `search`, `range`, and `delete` are `&self`.
pub struct BTree {
    root: PageId,
}

struct DeleteOutcome {
    removed: bool,
    underflow: bool,
}

impl BTree {
    pub fn open(root: PageId) -> Self {
        Self { root }
    }

    pub fn root(&self) -> PageId {
        self.root
    }

    /// Allocate a fresh root and write an empty leaf into it.
    pub fn create(store: &mut dyn BTreeStore) -> Result<Self> {
        let root_pid = store.allocate_page(PageType::Index)?;
        let leaf = LeafNode::new();
        let mut page = Page::new(root_pid, PageType::Index, store.page_size());
        Node::Leaf(leaf).write_to(&mut page)?;
        store.write_page(root_pid, &page)?;
        Ok(Self { root: root_pid })
    }

    pub fn search(&self, store: &mut dyn BTreeStore, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut current = self.root;
        loop {
            let page = store.read_page(current)?;
            match Node::from_page(&page)? {
                Node::Leaf(leaf) => return Ok(leaf.search(key).cloned()),
                Node::Internal(internal) => {
                    let idx = internal.find_child_index(key);
                    current = internal.child(idx);
                }
            }
        }
    }

    pub fn insert(
        &mut self,
        store: &mut dyn BTreeStore,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<()> {
        if let Some((median, new_page)) = self.insert_rec(store, self.root, key, value)? {
            let old_root = self.root;
            let new_root_pid = store.allocate_page(PageType::Index)?;
            let mut new_root = InternalNode::new(old_root);
            new_root.insert_separator(0, median, new_page);
            let mut page = Page::new(new_root_pid, PageType::Index, store.page_size());
            Node::Internal(new_root).write_to(&mut page)?;
            store.write_page(new_root_pid, &page)?;
            self.root = new_root_pid;
        }
        Ok(())
    }

    fn insert_rec(
        &mut self,
        store: &mut dyn BTreeStore,
        pid: PageId,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<Option<(Vec<u8>, PageId)>> {
        let page = store.read_page(pid)?;
        let node = Node::from_page(&page)?;
        let available = store.page_size() - PAGE_HEADER_SIZE;
        match node {
            Node::Leaf(mut leaf) => {
                leaf.insert(key, value);
                if leaf.encoded_size() <= available {
                    self.write_leaf(store, pid, &leaf)?;
                    return Ok(None);
                }
                let (new_leaf, median) = leaf.split_in_half();
                let new_pid = store.allocate_page(PageType::Index)?;
                self.write_leaf(store, pid, &leaf)?;
                self.write_leaf(store, new_pid, &new_leaf)?;
                Ok(Some((median, new_pid)))
            }
            Node::Internal(mut internal) => {
                let child_idx = internal.find_child_index(&key);
                let child_pid = internal.child(child_idx);
                let Some((median, new_page)) = self.insert_rec(store, child_pid, key, value)?
                else {
                    return Ok(None);
                };
                internal.insert_separator(child_idx, median, new_page);
                if internal.encoded_size() <= available {
                    self.write_internal(store, pid, &internal)?;
                    return Ok(None);
                }
                let (new_internal, median) = internal.split_in_half();
                let new_pid = store.allocate_page(PageType::Index)?;
                self.write_internal(store, pid, &internal)?;
                self.write_internal(store, new_pid, &new_internal)?;
                Ok(Some((median, new_pid)))
            }
        }
    }

    pub fn delete(&mut self, store: &mut dyn BTreeStore, key: &[u8]) -> Result<bool> {
        let outcome = self.delete_rec(store, self.root, key)?;
        Ok(outcome.removed)
    }

    fn delete_rec(
        &mut self,
        store: &mut dyn BTreeStore,
        pid: PageId,
        key: &[u8],
    ) -> Result<DeleteOutcome> {
        let page = store.read_page(pid)?;
        match Node::from_page(&page)? {
            Node::Leaf(mut leaf) => {
                let removed = leaf.remove(key);
                if removed {
                    self.write_leaf(store, pid, &leaf)?;
                }
                let underflow = leaf.entries.is_empty() && pid != self.root;
                Ok(DeleteOutcome { removed, underflow })
            }
            Node::Internal(mut internal) => {
                let child_idx = internal.find_child_index(key);
                let child_pid = internal.child(child_idx);
                let outcome = self.delete_rec(store, child_pid, key)?;
                if !outcome.removed {
                    return Ok(outcome);
                }
                if outcome.underflow {
                    self.try_merge(store, &mut internal, child_idx)?;
                    self.write_internal(store, pid, &internal)?;
                }
                let underflow = internal.entries.is_empty() && pid != self.root;
                Ok(DeleteOutcome {
                    removed: true,
                    underflow,
                })
            }
        }
    }

    /// Merge an underfull child at `child_idx` with an adjacent sibling if
    /// the combined node fits in one page.
    ///
    /// The sibling's page is **orphaned** (not freed). Proper page
    /// deallocation is a Phase 5 concern (transactional free).
    fn try_merge(
        &self,
        store: &mut dyn BTreeStore,
        parent: &mut InternalNode,
        child_idx: usize,
    ) -> Result<()> {
        let sibling = if child_idx + 1 < parent.child_count() {
            // Has a right sibling: merge (child, right) into left.
            Some((
                child_idx,
                parent.child(child_idx),
                parent.child(child_idx + 1),
            ))
        } else if child_idx > 0 {
            // Has a left sibling only: merge (left, child) into left.
            Some((
                child_idx - 1,
                parent.child(child_idx - 1),
                parent.child(child_idx),
            ))
        } else {
            None
        };
        let Some((sep_idx, left_pid, right_pid)) = sibling else {
            return Ok(());
        };
        let left_page = store.read_page(left_pid)?;
        let right_page = store.read_page(right_pid)?;
        let left_node = Node::from_page(&left_page)?;
        let right_node = Node::from_page(&right_page)?;
        let sep_key = parent.entries[sep_idx].0.clone();
        let combined = combine(left_node, &sep_key, right_node)?;
        let available = store.page_size() - PAGE_HEADER_SIZE;
        if combined.encoded_size() > available {
            return Ok(());
        }
        let mut page = Page::new(left_pid, PageType::Index, store.page_size());
        combined.write_to(&mut page)?;
        store.write_page(left_pid, &page)?;
        parent.entries.remove(sep_idx);
        Ok(())
    }

    pub fn range(
        &self,
        store: &mut dyn BTreeStore,
        lo: Option<&[u8]>,
        hi: Option<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut out = Vec::new();
        self.range_rec(store, self.root, lo, hi, &mut out)?;
        Ok(out)
    }

    fn range_rec(
        &self,
        store: &mut dyn BTreeStore,
        pid: PageId,
        lo: Option<&[u8]>,
        hi: Option<&[u8]>,
        out: &mut Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<()> {
        let page = store.read_page(pid)?;
        match Node::from_page(&page)? {
            Node::Leaf(leaf) => {
                for (k, v) in leaf.entries {
                    let after_lo = lo.is_none_or(|l| k.as_slice() >= l);
                    let before_hi = hi.is_none_or(|h| k.as_slice() < h);
                    if after_lo && before_hi {
                        out.push((k, v));
                    }
                }
            }
            Node::Internal(internal) => {
                let child_count = internal.child_count();
                for i in 0..child_count {
                    let child_pid = internal.child(i);
                    let child_lo: Option<&[u8]> = if i == 0 {
                        lo
                    } else {
                        Some(&internal.entries[i - 1].0)
                    };
                    let child_hi: Option<&[u8]> = if i == internal.entries.len() {
                        hi
                    } else {
                        Some(&internal.entries[i].0)
                    };
                    let skip_low = match (child_lo, hi) {
                        (Some(cl), Some(h)) => cl >= h,
                        _ => false,
                    };
                    let skip_high = match (child_hi, lo) {
                        (Some(ch), Some(l)) => ch <= l,
                        _ => false,
                    };
                    if skip_low || skip_high {
                        continue;
                    }
                    self.range_rec(store, child_pid, child_lo, child_hi, out)?;
                }
            }
        }
        Ok(())
    }

    fn write_leaf(&self, store: &mut dyn BTreeStore, pid: PageId, leaf: &LeafNode) -> Result<()> {
        let mut page = Page::new(pid, PageType::Index, store.page_size());
        leaf.write_to(&mut page)?;
        store.write_page(pid, &page)
    }

    fn write_internal(
        &self,
        store: &mut dyn BTreeStore,
        pid: PageId,
        internal: &InternalNode,
    ) -> Result<()> {
        let mut page = Page::new(pid, PageType::Index, store.page_size());
        internal.write_to(&mut page)?;
        store.write_page(pid, &page)
    }
}

fn combine(left: Node, sep_key: &[u8], right: Node) -> Result<Node> {
    match (left, right) {
        (Node::Leaf(mut l), Node::Leaf(r)) => {
            l.entries.extend(r.entries);
            Ok(Node::Leaf(l))
        }
        (Node::Internal(mut l), Node::Internal(r)) => {
            l.entries.push((sep_key.to_vec(), r.leftmost_child));
            l.entries.extend(r.entries);
            Ok(Node::Internal(l))
        }
        _ => Err(Error::Internal(
            "cannot merge nodes of different kinds".into(),
        )),
    }
}
