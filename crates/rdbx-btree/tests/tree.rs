//! Unit and property tests for the B+Tree, using an in-memory mock store.

use std::collections::BTreeMap;
use std::collections::HashMap;

use proptest::prelude::*;
use rdbx_btree::{BTree, BTreeStore, InternalNode, LeafNode};
use rdbx_core::{PageId, Result};
use rdbx_storage::{Page, PageType};

// ------------------------------------------------------- mock store

struct MockStore {
    pages: HashMap<PageId, Page>,
    next_pid: u64,
    page_size: usize,
}

impl MockStore {
    fn new(page_size: usize) -> Self {
        Self {
            pages: HashMap::new(),
            // Pages 0 and 1 are reserved (file header + meta) by convention.
            next_pid: 2,
            page_size,
        }
    }
}

impl BTreeStore for MockStore {
    fn read_page(&mut self, pid: PageId) -> Result<Page> {
        self.pages
            .get(&pid)
            .cloned()
            .ok_or_else(|| rdbx_core::Error::Internal(format!("page {pid} not found")))
    }

    fn write_page(&mut self, pid: PageId, page: &Page) -> Result<()> {
        self.pages.insert(pid, page.clone());
        Ok(())
    }

    fn allocate_page(&mut self, page_type: PageType) -> Result<PageId> {
        let pid = PageId(self.next_pid);
        self.next_pid += 1;
        self.pages
            .insert(pid, Page::new(pid, page_type, self.page_size));
        Ok(pid)
    }

    fn page_size(&self) -> usize {
        self.page_size
    }
}

// -------------------------------------------------------- unit tests

#[test]
fn leaf_roundtrip() {
    let leaf = LeafNode {
        entries: vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"b".to_vec(), b"22".to_vec()),
        ],
    };
    let mut page = Page::new(PageId(2), PageType::Index, 4096);
    leaf.write_to(&mut page).unwrap();
    let leaf2 = LeafNode::from_page(&page).unwrap();
    assert_eq!(leaf.entries, leaf2.entries);
}

#[test]
fn internal_roundtrip() {
    let internal = InternalNode {
        leftmost_child: PageId(10),
        entries: vec![(b"a".to_vec(), PageId(11)), (b"bb".to_vec(), PageId(12))],
    };
    let mut page = Page::new(PageId(2), PageType::Index, 4096);
    internal.write_to(&mut page).unwrap();
    let internal2 = InternalNode::from_page(&page).unwrap();
    assert_eq!(internal.leftmost_child, internal2.leftmost_child);
    assert_eq!(internal.entries, internal2.entries);
}

#[test]
fn empty_tree_search_returns_none() {
    let mut store = MockStore::new(4096);
    let tree = BTree::create(&mut store).unwrap();
    assert!(tree.search(&mut store, b"anything").unwrap().is_none());
}

#[test]
fn basic_insert_search() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    tree.insert(&mut store, b"a".to_vec(), b"1".to_vec())
        .unwrap();
    tree.insert(&mut store, b"b".to_vec(), b"2".to_vec())
        .unwrap();
    assert_eq!(tree.search(&mut store, b"a").unwrap(), Some(b"1".to_vec()));
    assert_eq!(tree.search(&mut store, b"b").unwrap(), Some(b"2".to_vec()));
    assert_eq!(tree.search(&mut store, b"c").unwrap(), None);
}

#[test]
fn update_existing_key() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    tree.insert(&mut store, b"a".to_vec(), b"1".to_vec())
        .unwrap();
    tree.insert(&mut store, b"a".to_vec(), b"99".to_vec())
        .unwrap();
    assert_eq!(tree.search(&mut store, b"a").unwrap(), Some(b"99".to_vec()));
}

#[test]
fn basic_delete() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    tree.insert(&mut store, b"a".to_vec(), b"1".to_vec())
        .unwrap();
    tree.insert(&mut store, b"b".to_vec(), b"2".to_vec())
        .unwrap();
    assert!(tree.delete(&mut store, b"a").unwrap());
    assert_eq!(tree.search(&mut store, b"a").unwrap(), None);
    assert_eq!(tree.search(&mut store, b"b").unwrap(), Some(b"2".to_vec()));
    assert!(!tree.delete(&mut store, b"missing").unwrap());
}

#[test]
fn range_scan_full() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    for i in 0u8..5 {
        tree.insert(&mut store, vec![i], vec![i * 2]).unwrap();
    }
    let all = tree.range(&mut store, None, None).unwrap();
    assert_eq!(all.len(), 5);
    for (i, (k, v)) in all.iter().enumerate() {
        assert_eq!(k, &vec![i as u8]);
        assert_eq!(v, &vec![i as u8 * 2]);
    }
}

#[test]
fn range_scan_bounded() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    for i in 0u8..10 {
        tree.insert(&mut store, vec![i], vec![i]).unwrap();
    }
    let mid = tree.range(&mut store, Some(&[3]), Some(&[7])).unwrap();
    assert_eq!(mid.len(), 4);
    for (k, _) in &mid {
        assert!(k[0] >= 3 && k[0] < 7);
    }
}

#[test]
fn many_inserts_forces_split() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    let n = 500u32;
    for i in 0..n {
        tree.insert(
            &mut store,
            i.to_le_bytes().to_vec(),
            i.to_le_bytes().to_vec(),
        )
        .unwrap();
    }
    for i in 0..n {
        assert_eq!(
            tree.search(&mut store, &i.to_le_bytes()).unwrap(),
            Some(i.to_le_bytes().to_vec()),
            "missing key {i} after {n} inserts"
        );
    }
}

#[test]
fn split_then_delete_recovers() {
    let mut store = MockStore::new(4096);
    let mut tree = BTree::create(&mut store).unwrap();
    let n = 300u32;
    for i in 0..n {
        tree.insert(&mut store, i.to_le_bytes().to_vec(), vec![0])
            .unwrap();
    }
    for i in 0..n {
        assert!(tree.delete(&mut store, &i.to_le_bytes()).unwrap());
    }
    for i in 0..n {
        assert!(tree.search(&mut store, &i.to_le_bytes()).unwrap().is_none());
    }
}

// ----------------------------------------------------- property test

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 100,
        .. ProptestConfig::default()
    })]

    /// Random sequence of insert / delete / search must keep the tree
    /// exactly in sync with `std::collections::BTreeMap`.
    #[test]
    fn btree_matches_std_btreemap(
        ops in prop::collection::vec((0u8..3, 0u16..500, 0u8..10), 0..400)
    ) {
        let mut store = MockStore::new(4096);
        let mut tree = BTree::create(&mut store).unwrap();
        let mut reference: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();

        for (op, key, val) in ops {
            let k = key.to_le_bytes().to_vec();
            match op {
                0 => {
                    tree.insert(&mut store, k.clone(), vec![val]).unwrap();
                    reference.insert(k, vec![val]);
                }
                1 => {
                    let got = tree.delete(&mut store, &k).unwrap();
                    let expected = reference.remove(&k).is_some();
                    prop_assert_eq!(got, expected, "delete({:?}) mismatch", k);
                }
                2 => {
                    let got = tree.search(&mut store, &k).unwrap();
                    prop_assert_eq!(got, reference.get(&k).cloned(),
                        "search({:?}) mismatch", k);
                }
                _ => unreachable!(),
            }
        }

        let all = tree.range(&mut store, None, None).unwrap();
        let expected: Vec<(Vec<u8>, Vec<u8>)> = reference.into_iter().collect();
        prop_assert_eq!(all, expected, "final range mismatch");
    }
}
