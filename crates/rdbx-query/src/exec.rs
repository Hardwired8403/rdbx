//! Pull-based physical operators.

use std::collections::VecDeque;

use rdbx_btree::{BTree, BTreeStore};
use rdbx_core::{Error, PageId, Result};

use crate::codec;
use crate::plan::{BinOp, Expr, PhysicalPlan, SortDir};
use crate::value::{Row, Value};

pub struct Execution<'a, S: BTreeStore> {
    state: ExecState<'a, S>,
}

enum ExecState<'a, S: BTreeStore> {
    SeqScan(ScanState),
    IndexScan(ScanState),
    Filter(Box<ExecState<'a, S>>, Expr),
    Project(Box<ExecState<'a, S>>, Vec<usize>),
    Sort(VecDeque<Row>),
    Limit(Box<ExecState<'a, S>>, usize),
    Insert {
        store: &'a mut S,
        root: PageId,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
        done: bool,
    },
    Delete {
        store: &'a mut S,
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        done: bool,
    },
    Update {
        store: &'a mut S,
        root: PageId,
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
        value: Vec<u8>,
        done: bool,
    },
}

struct ScanState {
    rows: VecDeque<Row>,
    projection: Option<Vec<usize>>,
}

pub fn execute<'a, S: BTreeStore>(
    plan: PhysicalPlan,
    store: &'a mut S,
) -> Result<Execution<'a, S>> {
    let state = build_state(plan, store)?;
    Ok(Execution { state })
}

impl<'a, S: BTreeStore> Execution<'a, S> {
    pub fn fetch_next(&mut self) -> Result<Option<Row>> {
        step(&mut self.state)
    }
}

fn build_state<'a, S: BTreeStore>(
    plan: PhysicalPlan,
    store: &'a mut S,
) -> Result<ExecState<'a, S>> {
    Ok(match plan {
        PhysicalPlan::SeqScan { root, projection } => {
            let tree = BTree::open(root);
            let raw = tree.range(store, None, None)?;
            let mut rows = VecDeque::with_capacity(raw.len());
            for (k, v) in raw {
                rows.push_back(decode_row_with_key(&k, &v)?);
            }
            ExecState::SeqScan(ScanState { rows, projection })
        }
        PhysicalPlan::IndexScan {
            root,
            lower,
            upper,
            projection,
        } => {
            let tree = BTree::open(root);
            let raw = tree.range(store, lower.as_deref(), upper.as_deref())?;
            let mut rows = VecDeque::with_capacity(raw.len());
            for (k, v) in raw {
                rows.push_back(decode_row_with_key(&k, &v)?);
            }
            ExecState::IndexScan(ScanState { rows, projection })
        }
        PhysicalPlan::Filter { input, predicate } => {
            let inner = build_state(*input, store)?;
            ExecState::Filter(Box::new(inner), predicate)
        }
        PhysicalPlan::Project { input, columns } => {
            let inner = build_state(*input, store)?;
            ExecState::Project(Box::new(inner), columns)
        }
        PhysicalPlan::Sort { input, keys } => {
            let mut inner = build_state(*input, store)?;
            let mut all = Vec::new();
            while let Some(row) = step(&mut inner)? {
                all.push(row);
            }
            sort_rows(&mut all, &keys);
            ExecState::Sort(VecDeque::from(all))
        }
        PhysicalPlan::Limit { input, n } => {
            let inner = build_state(*input, store)?;
            ExecState::Limit(Box::new(inner), n)
        }
        PhysicalPlan::Insert { root, rows } => ExecState::Insert {
            store,
            root,
            rows,
            done: false,
        },
        PhysicalPlan::Delete { root, lower, upper } => ExecState::Delete {
            store,
            root,
            lower,
            upper,
            done: false,
        },
        PhysicalPlan::Update {
            root,
            lower,
            upper,
            value,
        } => ExecState::Update {
            store,
            root,
            lower,
            upper,
            value,
            done: false,
        },
    })
}

fn step<'a, S: BTreeStore>(state: &mut ExecState<'a, S>) -> Result<Option<Row>> {
    match state {
        ExecState::SeqScan(scan) | ExecState::IndexScan(scan) => {
            if let Some(mut row) = scan.rows.pop_front() {
                if let Some(proj) = &scan.projection {
                    row = proj
                        .iter()
                        .map(|&i| row.get(i).cloned().unwrap_or(Value::Null))
                        .collect();
                }
                Ok(Some(row))
            } else {
                Ok(None)
            }
        }
        ExecState::Filter(inner, predicate) => loop {
            match step(inner)? {
                Some(row) => {
                    if eval(predicate, &row)? {
                        return Ok(Some(row));
                    }
                }
                None => return Ok(None),
            }
        },
        ExecState::Project(inner, columns) => match step(inner)? {
            Some(row) => {
                let projected = columns
                    .iter()
                    .map(|&i| row.get(i).cloned().unwrap_or(Value::Null))
                    .collect();
                Ok(Some(projected))
            }
            None => Ok(None),
        },
        ExecState::Sort(rows) => Ok(rows.pop_front()),
        ExecState::Limit(inner, n) => {
            if *n == 0 {
                return Ok(None);
            }
            *n -= 1;
            step(inner)
        }
        ExecState::Insert {
            store,
            root,
            rows,
            done,
        } => {
            if *done {
                return Ok(None);
            }
            *done = true;
            let mut tree = BTree::open(*root);
            for (k, v) in rows.drain(..) {
                tree.insert(&mut **store, k, v)?;
            }
            Ok(Some(vec![]))
        }
        ExecState::Delete {
            store,
            root,
            lower,
            upper,
            done,
        } => {
            if *done {
                return Ok(None);
            }
            *done = true;
            let mut tree = BTree::open(*root);
            let keys: Vec<Vec<u8>> = tree
                .range(&mut **store, lower.as_deref(), upper.as_deref())?
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            for k in keys {
                tree.delete(&mut **store, &k)?;
            }
            Ok(Some(vec![]))
        }
        ExecState::Update {
            store,
            root,
            lower,
            upper,
            value,
            done,
        } => {
            if *done {
                return Ok(None);
            }
            *done = true;
            let mut tree = BTree::open(*root);
            let existing = tree.range(&mut **store, lower.as_deref(), upper.as_deref())?;
            for (k, _) in existing {
                tree.insert(&mut **store, k, value.clone())?;
            }
            Ok(Some(vec![]))
        }
    }
}

fn eval(expr: &Expr, row: &Row) -> Result<bool> {
    let v = eval_expr(expr, row)?;
    match v {
        Value::Bool(b) => Ok(b),
        Value::Null => Ok(false),
        other => Err(Error::Internal(format!(
            "filter predicate is not boolean: {other:?}"
        ))),
    }
}

fn eval_expr(expr: &Expr, row: &Row) -> Result<Value> {
    Ok(match expr {
        Expr::Literal(v) => v.clone(),
        Expr::Column(i) => row.get(*i).cloned().unwrap_or(Value::Null),
        Expr::Binary(l, op, r) => {
            let lv = eval_expr(l, row)?;
            let rv = eval_expr(r, row)?;
            return eval_binop(&lv, *op, &rv);
        }
    })
}

fn eval_binop(l: &Value, op: BinOp, r: &Value) -> Result<Value> {
    let cmp = |f: bool| Ok(Value::Bool(f));
    match op {
        BinOp::Eq => cmp(l == r),
        BinOp::Ne => cmp(l != r),
        BinOp::And => match (l, r) {
            (Value::Bool(a), Value::Bool(b)) => cmp(*a && *b),
            _ => cmp(false),
        },
        BinOp::Or => match (l, r) {
            (Value::Bool(a), Value::Bool(b)) => cmp(*a || *b),
            _ => cmp(false),
        },
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = value_cmp(l, r);
            cmp(match op {
                BinOp::Lt => ord == std::cmp::Ordering::Less,
                BinOp::Le => ord != std::cmp::Ordering::Greater,
                BinOp::Gt => ord == std::cmp::Ordering::Greater,
                BinOp::Ge => ord != std::cmp::Ordering::Less,
                _ => unreachable!(),
            })
        }
    }
}

fn value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn sort_rows(rows: &mut [Row], keys: &[crate::plan::SortKey]) {
    rows.sort_by(|a, b| {
        for k in keys {
            let av = a.get(k.column).cloned().unwrap_or(Value::Null);
            let bv = b.get(k.column).cloned().unwrap_or(Value::Null);
            let mut ord = value_cmp(&av, &bv);
            if k.dir == SortDir::Desc {
                ord = ord.reverse();
            }
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn decode_row_with_key(key: &[u8], value: &[u8]) -> Result<Row> {
    if value.is_empty() {
        return Ok(vec![Value::Bytes(key.to_vec())]);
    }
    codec::decode_row(value)
}
