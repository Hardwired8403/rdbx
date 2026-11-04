//! B+Tree index for RDBX.
//!
//! The tree operates on any [`BTreeStore`] implementation: in RDBX, both
//! `Transaction` and `Database` implement it, so every B+Tree mutation goes
//! through the WAL and the buffer pool automatically.
//!
//! See `docs/storage/BTREE.md` for the on-page node layout and split/merge
//! rules.

pub mod node;
pub mod store;
pub mod tree;

pub use node::{
    InternalNode, LeafNode, NODE_HEADER_SIZE, NODE_TYPE_INTERNAL, NODE_TYPE_LEAF, Node,
};
pub use store::BTreeStore;
pub use tree::BTree;
