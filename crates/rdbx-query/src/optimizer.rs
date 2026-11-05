//! Logical → physical plan optimizer.

use crate::plan::{BinOp, Expr, LogicalPlan, PhysicalPlan};

/// Bounds for a range scan: `(lower, upper)`, both half-open-ish
/// (`lower` inclusive, `upper` exclusive).
type KeyRange = (Option<Vec<u8>>, Option<Vec<u8>>);

pub fn optimize(plan: LogicalPlan) -> PhysicalPlan {
    physicalize(plan, None, None)
}

fn physicalize(plan: LogicalPlan, lower: Option<Vec<u8>>, upper: Option<Vec<u8>>) -> PhysicalPlan {
    match plan {
        LogicalPlan::Scan { root } => {
            if lower.is_some() || upper.is_some() {
                PhysicalPlan::IndexScan {
                    root,
                    lower,
                    upper,
                    projection: None,
                }
            } else {
                PhysicalPlan::SeqScan {
                    root,
                    projection: None,
                }
            }
        }
        LogicalPlan::IndexScan { root, lower, upper } => PhysicalPlan::IndexScan {
            root,
            lower,
            upper,
            projection: None,
        },
        LogicalPlan::Filter { input, predicate } => {
            if let Some((lo, hi)) = extract_key_range(&predicate) {
                let inner = physicalize(*input, lo, hi);
                return PhysicalPlan::Filter {
                    input: Box::new(inner),
                    predicate,
                };
            }
            let inner = physicalize(*input, lower, upper);
            PhysicalPlan::Filter {
                input: Box::new(inner),
                predicate,
            }
        }
        LogicalPlan::Project { input, columns } => {
            let inner = physicalize(*input, None, None);
            match inner {
                PhysicalPlan::SeqScan { root, .. } => PhysicalPlan::SeqScan {
                    root,
                    projection: Some(columns),
                },
                PhysicalPlan::IndexScan {
                    root, lower, upper, ..
                } => PhysicalPlan::IndexScan {
                    root,
                    lower,
                    upper,
                    projection: Some(columns),
                },
                other => PhysicalPlan::Project {
                    input: Box::new(other),
                    columns,
                },
            }
        }
        LogicalPlan::Sort { input, keys } => {
            let inner = physicalize(*input, None, None);
            PhysicalPlan::Sort {
                input: Box::new(inner),
                keys,
            }
        }
        LogicalPlan::Limit { input, n } => {
            let inner = physicalize(*input, None, None);
            PhysicalPlan::Limit {
                input: Box::new(inner),
                n,
            }
        }
        LogicalPlan::Insert { root, rows } => PhysicalPlan::Insert { root, rows },
        LogicalPlan::Delete { root, lower, upper } => PhysicalPlan::Delete { root, lower, upper },
        LogicalPlan::Update {
            root,
            lower,
            upper,
            value,
        } => PhysicalPlan::Update {
            root,
            lower,
            upper,
            value,
        },
        LogicalPlan::UpdateSet {
            root,
            lower,
            upper,
            assignments,
        } => PhysicalPlan::UpdateSet {
            root,
            lower,
            upper,
            assignments,
        },
        LogicalPlan::Count { input, column } => {
            let inner = physicalize(*input, None, None);
            PhysicalPlan::Count {
                input: Box::new(inner),
                column,
            }
        }
    }
}

fn extract_key_range(expr: &Expr) -> Option<KeyRange> {
    if let Expr::Binary(lhs, op, rhs) = expr {
        let (lit_expr, op) = match (lhs.as_ref(), rhs.as_ref()) {
            (Expr::Column(0), Expr::Literal(v)) => (v, *op),
            (Expr::Literal(v), Expr::Column(0)) => {
                let flipped = match op {
                    BinOp::Lt => BinOp::Gt,
                    BinOp::Le => BinOp::Ge,
                    BinOp::Gt => BinOp::Lt,
                    BinOp::Ge => BinOp::Le,
                    other => *other,
                };
                (v, flipped)
            }
            _ => return None,
        };
        let key = lit_expr.encode_key().ok()?;
        Some(match op {
            BinOp::Eq => (Some(key.clone()), Some(successor(&key))),
            BinOp::Ge => (Some(key), None),
            BinOp::Gt => (Some(successor(&key)), None),
            BinOp::Lt => (None, Some(key)),
            BinOp::Le => (None, Some(successor(&key))),
            _ => return None,
        })
    } else {
        None
    }
}

fn successor(key: &[u8]) -> Vec<u8> {
    let mut out = key.to_vec();
    out.push(0);
    out
}
