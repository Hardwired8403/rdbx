//! Unit tests for the query engine using an in-memory mock store.

use std::collections::HashMap;

use rdbx_btree::BTreeStore;
use rdbx_core::{PageId, Result};
use rdbx_query::codec;
use rdbx_query::exec::execute;
use rdbx_query::optimizer::optimize;
use rdbx_query::plan::{BinOp, Expr, LogicalPlan, SortDir, SortKey};
use rdbx_query::value::{Row, Value};
use rdbx_storage::{Page, PageType};

struct MockStore {
    pages: HashMap<PageId, Page>,
    next_pid: u64,
    page_size: usize,
}

impl MockStore {
    fn new(page_size: usize) -> Self {
        Self {
            pages: HashMap::new(),
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

fn key_of(v: i64) -> Vec<u8> {
    Value::Int(v).encode_key().unwrap()
}

fn row_of(ints: &[i64]) -> Vec<u8> {
    let row: Row = ints.iter().map(|i| Value::Int(*i)).collect();
    codec::encode_row(&row)
}

fn read_all(mut exec: rdbx_query::Execution<'_, MockStore>) -> Vec<Row> {
    let mut out = Vec::new();
    while let Some(row) = exec.fetch_next().unwrap() {
        out.push(row);
    }
    out
}

#[test]
fn seq_scan_returns_all_rows() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..5i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i, i * 10]))
            .unwrap();
    }

    let plan = LogicalPlan::Scan { root };
    let phys = optimize(plan);
    let exec = execute(phys, &mut store).unwrap();
    let rows = read_all(exec);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0], vec![Value::Int(0), Value::Int(0)]);
    assert_eq!(rows[4], vec![Value::Int(4), Value::Int(40)]);
}

#[test]
fn filter_by_column_0() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..10i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i])).unwrap();
    }

    let plan = LogicalPlan::Filter {
        input: Box::new(LogicalPlan::Scan { root }),
        predicate: Expr::Binary(
            Box::new(Expr::Column(0)),
            BinOp::Ge,
            Box::new(Expr::Literal(Value::Int(5))),
        ),
    };
    let rows = read_all(execute(optimize(plan), &mut store).unwrap());
    assert_eq!(rows.len(), 5);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row[0], Value::Int(5 + i as i64));
    }
}

#[test]
fn filter_above_index_scan() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..10i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i])).unwrap();
    }

    let plan = LogicalPlan::Filter {
        input: Box::new(LogicalPlan::Scan { root }),
        predicate: Expr::Binary(
            Box::new(Expr::Column(0)),
            BinOp::Eq,
            Box::new(Expr::Literal(Value::Int(3))),
        ),
    };
    let rows = read_all(execute(optimize(plan), &mut store).unwrap());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int(3));
}

#[test]
fn projection_pushes_into_scan() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..3i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i, i * 100, i * 10000]))
            .unwrap();
    }

    let plan = LogicalPlan::Project {
        input: Box::new(LogicalPlan::Scan { root }),
        columns: vec![2, 0],
    };
    let rows = read_all(execute(optimize(plan), &mut store).unwrap());
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1], vec![Value::Int(10000), Value::Int(1)]);
}

#[test]
fn limit_stops_early() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..10i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i])).unwrap();
    }

    let plan = LogicalPlan::Limit {
        input: Box::new(LogicalPlan::Scan { root }),
        n: 3,
    };
    let rows = read_all(execute(optimize(plan), &mut store).unwrap());
    assert_eq!(rows.len(), 3);
}

#[test]
fn sort_by_column() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for (k, v) in [(0i64, 5i64), (1, 3), (2, 8), (3, 1)] {
        tree.insert(&mut store, key_of(k), row_of(&[v])).unwrap();
    }

    let plan = LogicalPlan::Sort {
        input: Box::new(LogicalPlan::Scan { root }),
        keys: vec![SortKey {
            column: 0,
            dir: SortDir::Asc,
        }],
    };
    let rows = read_all(execute(optimize(plan), &mut store).unwrap());
    let values: Vec<i64> = rows
        .iter()
        .map(|r| match r[0] {
            Value::Int(i) => i,
            _ => panic!("expected int"),
        })
        .collect();
    assert_eq!(values, vec![1, 3, 5, 8]);
}

#[test]
fn insert_adds_rows() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();

    let plan = LogicalPlan::Insert {
        root,
        rows: vec![(key_of(1), row_of(&[1])), (key_of(2), row_of(&[2]))],
    };
    let _ = read_all(execute(optimize(plan), &mut store).unwrap());

    let scan_plan = LogicalPlan::Scan { root };
    let rows = read_all(execute(optimize(scan_plan), &mut store).unwrap());
    assert_eq!(rows.len(), 2);
}

#[test]
fn delete_removes_matching_range() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..10i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i])).unwrap();
    }

    let plan = LogicalPlan::Delete {
        root,
        lower: Some(Value::Int(3).encode_key().unwrap()),
        upper: Some(Value::Int(7).encode_key().unwrap()),
    };
    let _ = read_all(execute(optimize(plan), &mut store).unwrap());

    let rows = read_all(execute(optimize(LogicalPlan::Scan { root }), &mut store).unwrap());
    assert_eq!(rows.len(), 6);
    let values: Vec<i64> = rows
        .iter()
        .map(|r| match r[0] {
            Value::Int(i) => i,
            _ => 0,
        })
        .collect();
    assert_eq!(values, vec![0, 1, 2, 7, 8, 9]);
}

#[test]
fn update_replaces_value_in_range() {
    let mut store = MockStore::new(4096);
    let root = rdbx_btree::BTree::create(&mut store).unwrap().root();
    let mut tree = rdbx_btree::BTree::open(root);
    for i in 0..5i64 {
        tree.insert(&mut store, key_of(i), row_of(&[i])).unwrap();
    }

    let plan = LogicalPlan::Update {
        root,
        lower: Some(Value::Int(2).encode_key().unwrap()),
        upper: Some(Value::Int(4).encode_key().unwrap()),
        value: row_of(&[99]),
    };
    let _ = read_all(execute(optimize(plan), &mut store).unwrap());

    let rows = read_all(execute(optimize(LogicalPlan::Scan { root }), &mut store).unwrap());
    assert_eq!(rows[2][0], Value::Int(99));
    assert_eq!(rows[3][0], Value::Int(99));
    assert_eq!(rows[0][0], Value::Int(0));
}
