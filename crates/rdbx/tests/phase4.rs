//! Phase 4 integration tests: B+Tree via Database + Transaction.

use std::path::{Path, PathBuf};

use rdbx::{BTree, Database, DatabaseOptions};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-p4-{tag}-{}-{}.rdbx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn wal_dir(path: &Path) -> PathBuf {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("rdbx");
    path.parent()
        .map(|parent| parent.join(format!("{file_name}.wal")))
        .unwrap_or_else(|| PathBuf::from(format!("{file_name}.wal")))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(wal_dir(path));
}

#[test]
fn btree_insert_search_commit_reopen() {
    let path = unique_path("basic");
    let root;
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        root = db.create_table("users").unwrap();
        let mut txn = db.begin_transaction().unwrap();
        let mut tree = BTree::open(root);
        for i in 0u32..50 {
            tree.insert(&mut txn, i.to_le_bytes().to_vec(), i.to_le_bytes().to_vec())
                .unwrap();
        }
        txn.commit().unwrap();
        db.close().unwrap();
    }
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        // Root persisted in the catalog.
        let r = db.table("users").unwrap().root_page;
        assert_eq!(r, root);
        let mut txn = db.begin_transaction().unwrap();
        let tree = BTree::open(r);
        for i in 0u32..50 {
            let got = tree.search(&mut txn, &i.to_le_bytes()).unwrap();
            assert_eq!(got, Some(i.to_le_bytes().to_vec()), "missing {i}");
        }
        txn.rollback().unwrap();
        db.close().unwrap();
    }
    cleanup(&path);
}

#[test]
fn btree_delete_and_range() {
    let path = unique_path("del");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let root = db.create_table("t").unwrap();
        let mut txn = db.begin_transaction().unwrap();
        let mut tree = BTree::open(root);
        for i in 0u8..20 {
            tree.insert(&mut txn, vec![i], vec![i]).unwrap();
        }
        for i in (0u8..20).step_by(2) {
            assert!(tree.delete(&mut txn, &[i]).unwrap());
        }
        let all = tree.range(&mut txn, None, None).unwrap();
        let keys: Vec<u8> = all.iter().map(|(k, _)| k[0]).collect();
        assert_eq!(keys, vec![1, 3, 5, 7, 9, 11, 13, 15, 17, 19]);
        txn.commit().unwrap();
    }
    cleanup(&path);
}

#[test]
fn btree_rollback_leaves_tree_empty() {
    let path = unique_path("rollback");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let root = db.create_table("t").unwrap();
        let mut txn = db.begin_transaction().unwrap();
        let mut tree = BTree::open(root);
        for i in 0u8..30 {
            tree.insert(&mut txn, vec![i], vec![i]).unwrap();
        }
        txn.rollback().unwrap();

        let mut txn = db.begin_transaction().unwrap();
        let tree = BTree::open(root);
        let all = tree.range(&mut txn, None, None).unwrap();
        assert!(all.is_empty(), "rollback should discard all inserts");
        txn.rollback().unwrap();
    }
    cleanup(&path);
}

#[test]
fn btree_survives_checkpoint() {
    let path = unique_path("ckpt");
    {
        let mut db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let root = db.create_table("t").unwrap();
        let mut txn = db.begin_transaction().unwrap();
        let mut tree = BTree::open(root);
        for i in 0u32..200 {
            tree.insert(&mut txn, i.to_le_bytes().to_vec(), i.to_le_bytes().to_vec())
                .unwrap();
        }
        txn.commit().unwrap();
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    {
        let db = Database::open(&path, DatabaseOptions::default()).unwrap();
        let root = db.table("t").unwrap().root_page;
        let mut txn = db.begin_transaction().unwrap();
        let tree = BTree::open(root);
        for i in 0u32..200 {
            assert_eq!(
                tree.search(&mut txn, &i.to_le_bytes()).unwrap(),
                Some(i.to_le_bytes().to_vec()),
                "missing {i}"
            );
        }
        txn.rollback().unwrap();
    }
    cleanup(&path);
}
