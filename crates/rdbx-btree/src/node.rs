//! On-page node layout and (de)serialization.
//!
//! Both node kinds share an 8-byte header:
//!
//! ```text
//! offset  size  field
//!   0      1    type      (1 = leaf, 2 = internal)
//!   1      1    flags     (0)
//!   2      2    count     (u16, number of entries)
//!   4      4    reserved  (0)
//! ```
//!
//! Internal nodes append `leftmost_child: u64` before the entries.
//!
//! Leaf entry:     `key_len: u16 | key | val_len: u32 | val`
//! Internal entry: `key_len: u16 | key | child_pid: u64`

use rdbx_core::{Error, PageId, Result};
use rdbx_storage::Page;

pub const NODE_HEADER_SIZE: usize = 8;
pub const NODE_TYPE_LEAF: u8 = 1;
pub const NODE_TYPE_INTERNAL: u8 = 2;

const PAGE_ID_SIZE: usize = 8;
const KEY_LEN_SIZE: usize = 2;
const VAL_LEN_SIZE: usize = 4;

#[derive(Debug, Clone)]
pub struct LeafNode {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub struct InternalNode {
    pub leftmost_child: PageId,
    pub entries: Vec<(Vec<u8>, PageId)>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(LeafNode),
    Internal(InternalNode),
}

// ---------------------------------------------------------------- leaf

impl LeafNode {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn search(&self, key: &[u8]) -> Option<&Vec<u8>> {
        match self
            .entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        {
            Ok(i) => Some(&self.entries[i].1),
            Err(_) => None,
        }
    }

    /// Insert or update. Returns `true` if a new entry was added,
    /// `false` if an existing entry's value was replaced.
    pub fn insert(&mut self, key: Vec<u8>, value: Vec<u8>) -> bool {
        match self
            .entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(&key))
        {
            Ok(i) => {
                self.entries[i].1 = value;
                false
            }
            Err(i) => {
                self.entries.insert(i, (key, value));
                true
            }
        }
    }

    pub fn remove(&mut self, key: &[u8]) -> bool {
        match self
            .entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        {
            Ok(i) => {
                self.entries.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    pub fn encoded_size(&self) -> usize {
        NODE_HEADER_SIZE
            + self
                .entries
                .iter()
                .map(|(k, v)| KEY_LEN_SIZE + k.len() + VAL_LEN_SIZE + v.len())
                .sum::<usize>()
    }

    pub fn write_to(&self, page: &mut Page) -> Result<()> {
        let payload = &mut page.payload;
        if payload.len() < NODE_HEADER_SIZE {
            return Err(Error::Internal(
                "page payload too small for node header".into(),
            ));
        }
        if self.encoded_size() > payload.len() {
            return Err(Error::Internal(format!(
                "leaf node too large: {} > {}",
                self.encoded_size(),
                payload.len()
            )));
        }
        payload[0] = NODE_TYPE_LEAF;
        payload[1] = 0;
        let count = u16::try_from(self.entries.len())
            .map_err(|_| Error::Internal("too many entries in leaf".into()))?;
        payload[2..4].copy_from_slice(&count.to_le_bytes());
        payload[4..8].fill(0);
        let mut off = NODE_HEADER_SIZE;
        for (k, v) in &self.entries {
            let klen =
                u16::try_from(k.len()).map_err(|_| Error::Internal("key too long".into()))?;
            let vlen =
                u32::try_from(v.len()).map_err(|_| Error::Internal("value too long".into()))?;
            payload[off..off + 2].copy_from_slice(&klen.to_le_bytes());
            off += 2;
            payload[off..off + k.len()].copy_from_slice(k);
            off += k.len();
            payload[off..off + 4].copy_from_slice(&vlen.to_le_bytes());
            off += 4;
            payload[off..off + v.len()].copy_from_slice(v);
            off += v.len();
        }
        Ok(())
    }

    pub fn from_page(page: &Page) -> Result<Self> {
        let payload = &page.payload;
        if payload.len() < NODE_HEADER_SIZE {
            return Err(Error::Internal(
                "page payload too small for node header".into(),
            ));
        }
        if payload[0] != NODE_TYPE_LEAF {
            return Err(Error::Internal(format!(
                "not a leaf node (type byte {})",
                payload[0]
            )));
        }
        let count = u16::from_le_bytes(payload[2..4].try_into().unwrap()) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut off = NODE_HEADER_SIZE;
        for _ in 0..count {
            if off + 2 > payload.len() {
                return Err(Error::Internal("leaf entry key_len truncated".into()));
            }
            let klen = u16::from_le_bytes(payload[off..off + 2].try_into().unwrap()) as usize;
            off += 2;
            if off + klen > payload.len() {
                return Err(Error::Internal("leaf entry key truncated".into()));
            }
            let key = payload[off..off + klen].to_vec();
            off += klen;
            if off + 4 > payload.len() {
                return Err(Error::Internal("leaf entry val_len truncated".into()));
            }
            let vlen = u32::from_le_bytes(payload[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            if off + vlen > payload.len() {
                return Err(Error::Internal("leaf entry val truncated".into()));
            }
            let value = payload[off..off + vlen].to_vec();
            off += vlen;
            entries.push((key, value));
        }
        Ok(LeafNode { entries })
    }

    /// Split in half. Returns `(right_half, median_key)`. The median key
    /// is a copy of the first key of the right half; both halves retain
    /// their own copy.
    pub fn split_in_half(&mut self) -> (LeafNode, Vec<u8>) {
        let mid = self.entries.len() / 2;
        let right = self.entries.split_off(mid);
        let median = right[0].0.clone();
        (LeafNode { entries: right }, median)
    }
}

impl Default for LeafNode {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------ internal

impl InternalNode {
    pub fn new(leftmost_child: PageId) -> Self {
        Self {
            leftmost_child,
            entries: Vec::new(),
        }
    }

    /// Return the index (0..=`entries.len()`) of the child that should be
    /// followed when searching for `key`.
    ///
    /// - `key < entries[0].0`  → 0 (leftmost)
    /// - `entries[i-1].0 <= key < entries[i].0`  → i
    /// - exact match on `entries[i].0`  → i+1 (separator belongs to the right)
    pub fn find_child_index(&self, key: &[u8]) -> usize {
        match self
            .entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
        {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    pub fn child(&self, idx: usize) -> PageId {
        if idx == 0 {
            self.leftmost_child
        } else {
            self.entries[idx - 1].1
        }
    }

    pub fn child_count(&self) -> usize {
        self.entries.len() + 1
    }

    pub fn insert_separator(&mut self, at: usize, key: Vec<u8>, right: PageId) {
        self.entries.insert(at, (key, right));
    }

    pub fn encoded_size(&self) -> usize {
        NODE_HEADER_SIZE
            + PAGE_ID_SIZE
            + self
                .entries
                .iter()
                .map(|(k, _)| KEY_LEN_SIZE + k.len() + PAGE_ID_SIZE)
                .sum::<usize>()
    }

    pub fn write_to(&self, page: &mut Page) -> Result<()> {
        let payload = &mut page.payload;
        if payload.len() < NODE_HEADER_SIZE + PAGE_ID_SIZE {
            return Err(Error::Internal(
                "page payload too small for internal header".into(),
            ));
        }
        if self.encoded_size() > payload.len() {
            return Err(Error::Internal(format!(
                "internal node too large: {} > {}",
                self.encoded_size(),
                payload.len()
            )));
        }
        payload[0] = NODE_TYPE_INTERNAL;
        payload[1] = 0;
        let count = u16::try_from(self.entries.len())
            .map_err(|_| Error::Internal("too many entries in internal node".into()))?;
        payload[2..4].copy_from_slice(&count.to_le_bytes());
        payload[4..8].fill(0);
        payload[8..16].copy_from_slice(&self.leftmost_child.0.to_le_bytes());
        let mut off = NODE_HEADER_SIZE + PAGE_ID_SIZE;
        for (k, c) in &self.entries {
            let klen =
                u16::try_from(k.len()).map_err(|_| Error::Internal("key too long".into()))?;
            payload[off..off + 2].copy_from_slice(&klen.to_le_bytes());
            off += 2;
            payload[off..off + k.len()].copy_from_slice(k);
            off += k.len();
            payload[off..off + 8].copy_from_slice(&c.0.to_le_bytes());
            off += 8;
        }
        Ok(())
    }

    pub fn from_page(page: &Page) -> Result<Self> {
        let payload = &page.payload;
        if payload.len() < NODE_HEADER_SIZE + PAGE_ID_SIZE {
            return Err(Error::Internal(
                "page payload too small for internal header".into(),
            ));
        }
        if payload[0] != NODE_TYPE_INTERNAL {
            return Err(Error::Internal(format!(
                "not an internal node (type byte {})",
                payload[0]
            )));
        }
        let count = u16::from_le_bytes(payload[2..4].try_into().unwrap()) as usize;
        let leftmost_child = PageId(u64::from_le_bytes(payload[8..16].try_into().unwrap()));
        let mut entries = Vec::with_capacity(count);
        let mut off = NODE_HEADER_SIZE + PAGE_ID_SIZE;
        for _ in 0..count {
            if off + 2 > payload.len() {
                return Err(Error::Internal("internal entry key_len truncated".into()));
            }
            let klen = u16::from_le_bytes(payload[off..off + 2].try_into().unwrap()) as usize;
            off += 2;
            if off + klen > payload.len() {
                return Err(Error::Internal("internal entry key truncated".into()));
            }
            let key = payload[off..off + klen].to_vec();
            off += klen;
            if off + 8 > payload.len() {
                return Err(Error::Internal("internal entry child truncated".into()));
            }
            let child = PageId(u64::from_le_bytes(
                payload[off..off + 8].try_into().unwrap(),
            ));
            off += 8;
            entries.push((key, child));
        }
        Ok(InternalNode {
            leftmost_child,
            entries,
        })
    }

    /// Split in half. The median separator *moves* into the parent (it is
    /// removed from `self.entries` and returned separately).
    pub fn split_in_half(&mut self) -> (InternalNode, Vec<u8>) {
        let mid = self.entries.len() / 2;
        let right_entries = self.entries.split_off(mid + 1);
        let (median_key, right_leftmost) =
            self.entries.pop().expect("split on empty internal node");
        let right = InternalNode {
            leftmost_child: right_leftmost,
            entries: right_entries,
        };
        (right, median_key)
    }
}

// ---------------------------------------------------------------- node

impl Node {
    pub fn from_page(page: &Page) -> Result<Self> {
        let payload = &page.payload;
        if payload.is_empty() {
            return Err(Error::Internal("empty page payload".into()));
        }
        match payload[0] {
            NODE_TYPE_LEAF => Ok(Node::Leaf(LeafNode::from_page(page)?)),
            NODE_TYPE_INTERNAL => Ok(Node::Internal(InternalNode::from_page(page)?)),
            t => Err(Error::Internal(format!("unknown node type byte {t}"))),
        }
    }

    pub fn encoded_size(&self) -> usize {
        match self {
            Node::Leaf(l) => l.encoded_size(),
            Node::Internal(i) => i.encoded_size(),
        }
    }

    pub fn write_to(&self, page: &mut Page) -> Result<()> {
        match self {
            Node::Leaf(l) => l.write_to(page),
            Node::Internal(i) => i.write_to(page),
        }
    }
}
