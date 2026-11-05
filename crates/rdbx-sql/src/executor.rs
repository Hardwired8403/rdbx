//! Lower AST statements into `rdbx_query::LogicalPlan`.
//!
//! The executor does not run queries; it only produces plans. The
//! `rdbx` crate drives the actual execution through `Transaction` +
//! `rdbx-query::execute`.

use rdbx_core::PageId;
use rdbx_query::LogicalPlan;
use rdbx_query::plan::{BinOp as QBinOp, Expr as QExpr, SortDir, SortKey};
use rdbx_query::value::Value;

use crate::ast::{BinOp, Expr, SelectItem, Statement, UnaryOp};
use crate::catalog::SqlCatalog;
use crate::error::{SqlError, SqlResult};

/// `[lower, upper)` bounds for an index range scan.
type KeyRange = (Option<Vec<u8>>, Option<Vec<u8>>);

/// Lower `stmt` to a `LogicalPlan` using the given schema catalog.
///
/// `root_for` maps a table name to the B+Tree root page id.
pub fn compile<F>(stmt: &Statement, catalog: &SqlCatalog, root_for: F) -> SqlResult<LogicalPlan>
where
    F: Fn(&str) -> Option<PageId>,
{
    match stmt {
        Statement::Select {
            items,
            table,
            filter,
            order_by,
            limit,
            group_by,
            having,
        } => compile_select(
            items, table, filter, order_by, limit, group_by, having, catalog, &root_for,
        ),
        Statement::Insert {
            table,
            columns,
            values,
        } => compile_insert(table, columns, values, catalog, &root_for),
        Statement::Update {
            table,
            assignments,
            filter,
        } => compile_update(table, assignments, filter, catalog, &root_for),
        Statement::Delete { table, filter } => compile_delete(table, filter, catalog, &root_for),
        Statement::CreateTable { .. } => Err(SqlError::Unimplemented(
            "CREATE TABLE must be executed through the schema layer, not compiled to a plan".into(),
        )),
        Statement::DropTable { .. } => Err(SqlError::Unimplemented(
            "DROP TABLE must be executed through the schema layer".into(),
        )),
        Statement::CreateIndex { .. } => Err(SqlError::Unimplemented(
            "CREATE INDEX is not supported yet".into(),
        )),
        Statement::DropIndex { .. } => Err(SqlError::Unimplemented(
            "DROP INDEX is not supported yet".into(),
        )),
        Statement::Begin | Statement::Commit | Statement::Rollback => Err(SqlError::Unimplemented(
            "transaction statements are handled by the driver, not the planner".into(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_select<F>(
    items: &[SelectItem],
    table: &str,
    filter: &Option<Expr>,
    order_by: &[(String, bool)],
    limit: &Option<usize>,
    group_by: &[String],
    having: &Option<Expr>,
    catalog: &SqlCatalog,
    root_for: &F,
) -> SqlResult<LogicalPlan>
where
    F: Fn(&str) -> Option<PageId>,
{
    let schema = catalog
        .get(table)
        .ok_or_else(|| SqlError::Semantic(format!("unknown table '{table}'")))?;
    let root = root_for(table)
        .ok_or_else(|| SqlError::Semantic(format!("table '{table}' has no B+Tree root")))?;

    if having.is_some() {
        return Err(SqlError::Unimplemented(
            "HAVING clause is not supported yet".into(),
        ));
    }

    if !group_by.is_empty() || items.iter().any(is_aggregate_select_item) {
        return compile_grouped_select(
            items, table, filter, order_by, limit, group_by, catalog, root_for,
        );
    }

    if items.len() == 1
        && let SelectItem::Expr {
            expr: Expr::Call(name, args),
            ..
        } = &items[0]
        && name.eq_ignore_ascii_case("COUNT")
    {
        if !order_by.is_empty() {
            return Err(SqlError::Unimplemented(
                "ORDER BY with COUNT is not supported yet".into(),
            ));
        }
        let column = match args.as_slice() {
            [] => None,
            [Expr::Column(name)] => Some(
                schema
                    .column_index(name)
                    .ok_or_else(|| SqlError::Semantic(format!("unknown column '{name}'")))?,
            ),
            _ => {
                return Err(SqlError::Unimplemented(
                    "COUNT requires either * or a single column".into(),
                ));
            }
        };
        let mut plan = LogicalPlan::Scan { root };
        if let Some(f) = filter {
            plan = LogicalPlan::Filter {
                input: Box::new(plan),
                predicate: lower_expr(f, schema)?,
            };
        }
        let mut count_plan = LogicalPlan::Count {
            input: Box::new(plan),
            column,
        };
        if let Some(n) = limit {
            count_plan = LogicalPlan::Limit {
                input: Box::new(count_plan),
                n: *n,
            };
        }
        return Ok(count_plan);
    }
    if items.iter().filter(|item| matches!(item, SelectItem::Expr { expr: Expr::Call(name, _), .. } if name.eq_ignore_ascii_case("COUNT"))).count() > 0
    {
        return Err(SqlError::Unimplemented(
            "mixing aggregates and columns is not supported yet".into(),
        ));
    }

    // Base scan.
    let mut plan = LogicalPlan::Scan { root };

    // WHERE.
    if let Some(f) = filter {
        let pred = lower_expr(f, schema)?;
        plan = LogicalPlan::Filter {
            input: Box::new(plan),
            predicate: pred,
        };
    }

    // ORDER BY.
    if !order_by.is_empty() {
        let mut keys = Vec::with_capacity(order_by.len());
        for (col_name, asc) in order_by {
            let idx = schema.column_index(col_name).ok_or_else(|| {
                SqlError::Semantic(format!("unknown column '{col_name}' in ORDER BY"))
            })?;
            keys.push(SortKey {
                column: idx,
                dir: if *asc { SortDir::Asc } else { SortDir::Desc },
            });
        }
        plan = LogicalPlan::Sort {
            input: Box::new(plan),
            keys,
        };
    }

    // SELECT list.
    let mut projection: Option<Vec<usize>> = None;
    let mut extra_project: Vec<QExpr> = Vec::new();
    let mut non_column = false;
    for item in items {
        match item {
            SelectItem::Wildcard => {
                projection = None;
                break;
            }
            SelectItem::Expr {
                expr: Expr::Column(name),
                ..
            } => {
                let idx = schema
                    .column_index(name)
                    .ok_or_else(|| SqlError::Semantic(format!("unknown column '{name}'")))?;
                extra_project.push(QExpr::Column(idx));
            }
            SelectItem::Expr { expr, .. } => {
                non_column = true;
                let lower = lower_expr(expr, schema)?;
                extra_project.push(lower);
            }
        }
    }

    if !non_column {
        // All projections are plain columns: push them into the scan.
        let mut cols = Vec::with_capacity(extra_project.len());
        for e in &extra_project {
            if let QExpr::Column(i) = e {
                cols.push(*i);
            } else {
                non_column = true;
                break;
            }
        }
        if !non_column && !cols.is_empty() {
            projection = Some(cols);
        }
    }

    if let Some(cols) = projection {
        plan = LogicalPlan::Project {
            input: Box::new(plan),
            columns: cols,
        };
    } else if non_column {
        // Mixed / computed expressions: wrap with a general Project that
        // only accepts column references in Phase 7. Anything else is
        // rejected as Unimplemented.
        return Err(SqlError::Unimplemented(
            "SELECT of computed expressions is not supported yet".into(),
        ));
    }

    if let Some(n) = limit {
        plan = LogicalPlan::Limit {
            input: Box::new(plan),
            n: *n,
        };
    }

    Ok(plan)
}

fn is_aggregate_select_item(item: &SelectItem) -> bool {
    match item {
        SelectItem::Expr {
            expr: Expr::Call(name, _),
            ..
        } => matches!(
            name.to_ascii_uppercase().as_str(),
            "COUNT" | "SUM" | "MIN" | "MAX" | "AVG"
        ),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_grouped_select<F>(
    items: &[SelectItem],
    table: &str,
    filter: &Option<Expr>,
    order_by: &[(String, bool)],
    limit: &Option<usize>,
    group_by: &[String],
    catalog: &SqlCatalog,
    root_for: &F,
) -> SqlResult<LogicalPlan>
where
    F: Fn(&str) -> Option<PageId>,
{
    let schema = catalog
        .get(table)
        .ok_or_else(|| SqlError::Semantic(format!("unknown table '{table}'")))?;
    let root = root_for(table)
        .ok_or_else(|| SqlError::Semantic(format!("table '{table}' has no B+Tree root")))?;

    let mut plan = LogicalPlan::Scan { root };
    if let Some(f) = filter {
        plan = LogicalPlan::Filter {
            input: Box::new(plan),
            predicate: lower_expr(f, schema)?,
        };
    }

    let mut group_cols = Vec::with_capacity(group_by.len());
    for name in group_by {
        let idx = schema
            .column_index(name)
            .ok_or_else(|| SqlError::Semantic(format!("unknown column '{name}' in GROUP BY")))?;
        group_cols.push(idx);
    }

    let mut aggregates = Vec::new();
    for item in items {
        if let SelectItem::Expr {
            expr: Expr::Call(name, args),
            ..
        } = item
        {
            let kind = match name.to_ascii_uppercase().as_str() {
                "COUNT" => crate::ast::AggregateKind::Count,
                "SUM" => crate::ast::AggregateKind::Sum,
                "MIN" => crate::ast::AggregateKind::Min,
                "MAX" => crate::ast::AggregateKind::Max,
                "AVG" => crate::ast::AggregateKind::Avg,
                _ => continue,
            };
            let column = match args.as_slice() {
                [] => None,
                [Expr::Column(col)] => Some(
                    schema
                        .column_index(col)
                        .ok_or_else(|| SqlError::Semantic(format!("unknown column '{col}'")))?,
                ),
                _ => {
                    return Err(SqlError::Unimplemented(
                        "aggregate calls only support a single column or COUNT(*)".into(),
                    ));
                }
            };
            let aggregate = match kind {
                crate::ast::AggregateKind::Count => rdbx_query::plan::Aggregate::Count,
                crate::ast::AggregateKind::Sum => rdbx_query::plan::Aggregate::Sum(
                    column
                        .ok_or_else(|| SqlError::Unimplemented("SUM requires a column".into()))?,
                ),
                crate::ast::AggregateKind::Min => rdbx_query::plan::Aggregate::Min(
                    column
                        .ok_or_else(|| SqlError::Unimplemented("MIN requires a column".into()))?,
                ),
                crate::ast::AggregateKind::Max => rdbx_query::plan::Aggregate::Max(
                    column
                        .ok_or_else(|| SqlError::Unimplemented("MAX requires a column".into()))?,
                ),
                crate::ast::AggregateKind::Avg => rdbx_query::plan::Aggregate::Avg(
                    column
                        .ok_or_else(|| SqlError::Unimplemented("AVG requires a column".into()))?,
                ),
            };
            let output = aggregates.len() + group_cols.len();
            aggregates.push(rdbx_query::plan::AggregateSpec {
                kind: aggregate,
                output,
            });
        }
    }

    if !order_by.is_empty() {
        return Err(SqlError::Unimplemented(
            "ORDER BY with GROUP BY is not supported yet".into(),
        ));
    }

    plan = LogicalPlan::Aggregate {
        input: Box::new(plan),
        group_by: group_cols,
        aggregates,
    };

    if let Some(n) = limit {
        plan = LogicalPlan::Limit {
            input: Box::new(plan),
            n: *n,
        };
    }

    Ok(plan)
}

fn compile_insert<F>(
    table: &str,
    columns: &Option<Vec<String>>,
    values: &[Vec<Expr>],
    catalog: &SqlCatalog,
    root_for: &F,
) -> SqlResult<LogicalPlan>
where
    F: Fn(&str) -> Option<PageId>,
{
    let schema = catalog
        .get(table)
        .ok_or_else(|| SqlError::Semantic(format!("unknown table '{table}'")))?;
    let root = root_for(table)
        .ok_or_else(|| SqlError::Semantic(format!("table '{table}' has no B+Tree root")))?;

    // Determine which column each value goes into.
    let target_columns: Vec<usize> = match columns {
        Some(cols) => {
            let mut out = Vec::with_capacity(cols.len());
            for c in cols {
                out.push(
                    schema
                        .column_index(c)
                        .ok_or_else(|| SqlError::Semantic(format!("unknown column '{c}'")))?,
                );
            }
            out
        }
        None => (0..schema.columns.len()).collect(),
    };

    let mut rows = Vec::with_capacity(values.len());
    for value_row in values {
        if value_row.len() != target_columns.len() {
            return Err(SqlError::Semantic(format!(
                "INSERT has {} values for {} columns",
                value_row.len(),
                target_columns.len()
            )));
        }
        let mut full_row: Vec<Value> = vec![Value::Null; schema.columns.len()];
        for (val_expr, &col_idx) in value_row.iter().zip(target_columns.iter()) {
            let value = const_eval(val_expr)?;
            let coerced = schema.coerce(col_idx, &value)?;
            if schema.columns[col_idx].not_null && coerced.is_null() {
                return Err(SqlError::Semantic(format!(
                    "column '{}' is NOT NULL",
                    schema.columns[col_idx].name
                )));
            }
            full_row[col_idx] = coerced;
        }
        let pk_idx = schema
            .primary_key
            .ok_or_else(|| SqlError::Semantic(format!("table '{table}' has no PRIMARY KEY")))?;
        let key_value = &full_row[pk_idx];
        let key_bytes = key_value
            .encode_key()
            .map_err(|e| SqlError::Semantic(format!("invalid PRIMARY KEY: {e}")))?;
        let payload = rdbx_query::codec::encode_row(&full_row);
        rows.push((key_bytes, payload));
    }
    Ok(LogicalPlan::Insert { root, rows })
}

fn compile_update<F>(
    table: &str,
    assignments: &[(String, Expr)],
    filter: &Option<Expr>,
    catalog: &SqlCatalog,
    root_for: &F,
) -> SqlResult<LogicalPlan>
where
    F: Fn(&str) -> Option<PageId>,
{
    let schema = catalog
        .get(table)
        .ok_or_else(|| SqlError::Semantic(format!("unknown table '{table}'")))?;
    let root = root_for(table)
        .ok_or_else(|| SqlError::Semantic(format!("table '{table}' has no B+Tree root")))?;

    // UPDATE requires a primary key to address rows.
    if schema.primary_key.is_none() {
        return Err(SqlError::Unimplemented(
            "UPDATE without PRIMARY KEY is not supported".into(),
        ));
    }

    // Phase 7 requires UPDATE to rewrite only non-PK columns and to
    // filter on a single PRIMARY KEY value.
    if assignments.iter().any(|(c, _)| {
        let lower = c.to_ascii_lowercase();
        schema
            .columns
            .iter()
            .find(|col| col.name.to_ascii_lowercase() == lower)
            .map(|col| col.primary_key)
            .unwrap_or(false)
    }) {
        return Err(SqlError::Unimplemented(
            "UPDATE of the PRIMARY KEY column is not supported".into(),
        ));
    }

    let Some(filter_expr) = filter else {
        return Err(SqlError::Unimplemented(
            "UPDATE without WHERE is not supported in Phase 7".into(),
        ));
    };
    let (lower, upper) = range_from_filter(filter_expr, schema)?;

    let mut assignments_q = Vec::with_capacity(assignments.len());
    for (col_name, val_expr) in assignments {
        let col_idx = schema
            .column_index(col_name)
            .ok_or_else(|| SqlError::Semantic(format!("unknown column '{col_name}'")))?;
        let value = lower_expr(val_expr, schema)?;
        assignments_q.push((col_idx, value));
    }

    Ok(LogicalPlan::UpdateSet {
        root,
        lower,
        upper,
        assignments: assignments_q,
    })
}

fn compile_delete<F>(
    table: &str,
    filter: &Option<Expr>,
    catalog: &SqlCatalog,
    root_for: &F,
) -> SqlResult<LogicalPlan>
where
    F: Fn(&str) -> Option<PageId>,
{
    let schema = catalog
        .get(table)
        .ok_or_else(|| SqlError::Semantic(format!("unknown table '{table}'")))?;
    let root = root_for(table)
        .ok_or_else(|| SqlError::Semantic(format!("table '{table}' has no B+Tree root")))?;
    let Some(filter_expr) = filter else {
        return Err(SqlError::Unimplemented(
            "DELETE without WHERE is not supported in Phase 7".into(),
        ));
    };
    let (lower, upper) = range_from_filter(filter_expr, schema)?;
    Ok(LogicalPlan::Delete { root, lower, upper })
}

/// Extract `[lower, upper)` bounds from a filter that pins the PRIMARY
/// KEY column with equality.
fn range_from_filter(expr: &Expr, schema: &crate::catalog::TableSchema) -> SqlResult<KeyRange> {
    let pk_idx = schema
        .primary_key
        .ok_or_else(|| SqlError::Semantic("table has no PRIMARY KEY".into()))?;
    let pk_name = schema.columns[pk_idx].name.to_ascii_lowercase();

    match expr {
        Expr::Binary(l, BinOp::Eq, r) => {
            let (col, lit) = match (l.as_ref(), r.as_ref()) {
                (Expr::Column(name), other) if name.to_ascii_lowercase() == pk_name => {
                    (name, other)
                }
                (other, Expr::Column(name)) if name.to_ascii_lowercase() == pk_name => {
                    (name, other)
                }
                _ => {
                    return Err(SqlError::Unimplemented(
                        "WHERE must constrain the PRIMARY KEY column".into(),
                    ));
                }
            };
            let _ = col;
            let value = const_eval(lit)?;
            let key = value
                .encode_key()
                .map_err(|e| SqlError::Semantic(format!("invalid PK literal: {e}")))?;
            let mut upper = key.clone();
            upper.push(0);
            Ok((Some(key), Some(upper)))
        }
        _ => Err(SqlError::Unimplemented(
            "WHERE must be a PRIMARY KEY equality comparison in Phase 7".into(),
        )),
    }
}

fn lower_expr(expr: &Expr, schema: &crate::catalog::TableSchema) -> SqlResult<QExpr> {
    Ok(match expr {
        Expr::Null => QExpr::Literal(Value::Null),
        Expr::Bool(b) => QExpr::Literal(Value::Bool(*b)),
        Expr::Int(i) => QExpr::Literal(Value::Int(*i)),
        Expr::Float(f) => QExpr::Literal(Value::Float(*f)),
        Expr::Str(s) => QExpr::Literal(Value::Bytes(s.as_bytes().to_vec())),
        Expr::Blob(b) => QExpr::Literal(Value::Bytes(b.clone())),
        Expr::Column(name) => {
            let idx = schema
                .column_index(name)
                .ok_or_else(|| SqlError::Semantic(format!("unknown column '{name}'")))?;
            QExpr::Column(idx)
        }
        Expr::Binary(l, op, r) => {
            let lop = match op {
                BinOp::Eq => QBinOp::Eq,
                BinOp::Ne => QBinOp::Ne,
                BinOp::Lt => QBinOp::Lt,
                BinOp::Le => QBinOp::Le,
                BinOp::Gt => QBinOp::Gt,
                BinOp::Ge => QBinOp::Ge,
                BinOp::And => QBinOp::And,
                BinOp::Or => QBinOp::Or,
                other => {
                    return Err(SqlError::Unimplemented(format!(
                        "operator {other:?} is not supported in WHERE"
                    )));
                }
            };
            QExpr::Binary(
                Box::new(lower_expr(l, schema)?),
                lop,
                Box::new(lower_expr(r, schema)?),
            )
        }
        Expr::Unary(UnaryOp::Not, inner) => QExpr::Binary(
            Box::new(QExpr::Literal(Value::Bool(true))),
            QBinOp::Ne,
            Box::new(lower_expr(inner, schema)?),
        ),
        Expr::Unary(UnaryOp::Neg, _) => {
            return Err(SqlError::Unimplemented(
                "unary minus in WHERE is not supported".into(),
            ));
        }
        Expr::Call(name, _) => {
            return Err(SqlError::Unimplemented(format!(
                "function call '{name}' is not supported in WHERE"
            )));
        }
        Expr::Placeholder(_) | Expr::Subquery(_) | Expr::InSubquery(_, _) => {
            return Err(SqlError::Unimplemented(
                "subqueries and placeholders are not supported in WHERE".into(),
            ));
        }
    })
}

fn const_eval(expr: &Expr) -> SqlResult<Value> {
    Ok(match expr {
        Expr::Null => Value::Null,
        Expr::Bool(b) => Value::Bool(*b),
        Expr::Int(i) => Value::Int(*i),
        Expr::Float(f) => Value::Float(*f),
        Expr::Str(s) => Value::Bytes(s.as_bytes().to_vec()),
        Expr::Blob(b) => Value::Bytes(b.clone()),
        Expr::Unary(UnaryOp::Neg, inner) => match const_eval(inner)? {
            Value::Int(i) => Value::Int(-i),
            Value::Float(f) => Value::Float(-f),
            _ => {
                return Err(SqlError::Semantic(
                    "unary minus on a non-numeric literal".into(),
                ));
            }
        },
        Expr::Column(name) => {
            return Err(SqlError::Semantic(format!(
                "column '{name}' is not allowed in a constant expression"
            )));
        }
        Expr::Binary(_, _, _) | Expr::Unary(UnaryOp::Not, _) | Expr::Call(_, _) => {
            return Err(SqlError::Unimplemented(
                "only literal constants are supported in INSERT / UPDATE values".into(),
            ));
        }
        Expr::Placeholder(_) | Expr::Subquery(_) | Expr::InSubquery(_, _) => {
            return Err(SqlError::Unimplemented(
                "prepared statements and subqueries are not supported in constant expressions"
                    .into(),
            ));
        }
    })
}
