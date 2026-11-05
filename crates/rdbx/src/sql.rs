//! SQL integration: `Database::execute(sql)`.
//!
//! The public API of this module:
//!
//! * `Database::execute(&self, sql) -> Result<QueryResult>` runs one SQL
//!   statement inside a fresh `Transaction` and returns either rows
//!   (`QueryResult::Rows`) or the number of affected rows
//!   (`QueryResult::Affected`).
//!
//! The SQL parser and planner live in `rdbx-sql`; this module only
//! glues them to `Transaction` + `rdbx-query::execute`.

use rdbx_core::{Error, Result};
use rdbx_query::exec::execute as execute_plan;
use rdbx_query::optimizer::optimize;
use rdbx_query::value::Row;
use rdbx_sql::catalog::{SqlCatalog, TableSchema};
use rdbx_sql::{Expr, SelectItem, Statement};

use crate::database::Database;

/// Result of one SQL statement.
#[derive(Debug)]
pub enum QueryResult {
    /// A result set. `columns` names each output column in order.
    Rows {
        columns: Vec<String>,
        rows: Vec<Row>,
    },
    /// DML affected this many rows.
    Affected(u64),
}

pub struct PreparedStatement<'db> {
    db: &'db Database,
    plan: rdbx_sql::PreparedPlan,
    args: Vec<Option<rdbx_query::value::Value>>,
}

impl<'db> PreparedStatement<'db> {
    pub fn arity(&self) -> usize {
        self.plan.arity
    }

    pub fn bind(&mut self, idx: usize, value: rdbx_query::value::Value) -> Result<()> {
        if idx >= self.plan.arity {
            return Err(Error::Internal(format!(
                "bind index {idx} out of range (arity {})",
                self.plan.arity
            )));
        }
        self.args[idx] = Some(value);
        Ok(())
    }

    pub fn execute(&mut self) -> Result<QueryResult> {
        let stmt = self.substitute_placeholders(&self.plan.template.clone())?;
        self.db.execute_ast(stmt)
    }

    fn substitute_placeholders(&self, stmt: &Statement) -> Result<Statement> {
        match stmt {
            Statement::Select {
                items,
                table,
                filter,
                order_by,
                limit,
                group_by,
                having,
            } => {
                let mut new_items = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        SelectItem::Wildcard => new_items.push(SelectItem::Wildcard),
                        SelectItem::Expr { expr, alias } => new_items.push(SelectItem::Expr {
                            expr: self.substitute_expr(expr)?,
                            alias: alias.clone(),
                        }),
                    }
                }
                Ok(Statement::Select {
                    items: new_items,
                    table: table.clone(),
                    filter: filter
                        .as_ref()
                        .map(|e| self.substitute_expr(e))
                        .transpose()?,
                    order_by: order_by.clone(),
                    limit: *limit,
                    group_by: group_by.clone(),
                    having: having
                        .as_ref()
                        .map(|e| self.substitute_expr(e))
                        .transpose()?,
                })
            }
            Statement::Insert {
                table,
                columns,
                values,
            } => {
                let mut new_values = Vec::with_capacity(values.len());
                for row in values {
                    let mut out = Vec::with_capacity(row.len());
                    for expr in row {
                        out.push(self.substitute_expr(expr)?);
                    }
                    new_values.push(out);
                }
                Ok(Statement::Insert {
                    table: table.clone(),
                    columns: columns.clone(),
                    values: new_values,
                })
            }
            Statement::Update {
                table,
                assignments,
                filter,
            } => {
                let mut new_assignments = Vec::with_capacity(assignments.len());
                for (name, expr) in assignments {
                    new_assignments.push((name.clone(), self.substitute_expr(expr)?));
                }
                Ok(Statement::Update {
                    table: table.clone(),
                    assignments: new_assignments,
                    filter: filter
                        .as_ref()
                        .map(|e| self.substitute_expr(e))
                        .transpose()?,
                })
            }
            Statement::Delete { table, filter } => Ok(Statement::Delete {
                table: table.clone(),
                filter: filter
                    .as_ref()
                    .map(|e| self.substitute_expr(e))
                    .transpose()?,
            }),
            Statement::CreateIndex {
                table,
                columns,
                name,
            } => Ok(Statement::CreateIndex {
                table: table.clone(),
                columns: columns.clone(),
                name: name.clone(),
            }),
            Statement::CreateTable { .. }
            | Statement::DropTable { .. }
            | Statement::DropIndex { .. }
            | Statement::Begin
            | Statement::Commit
            | Statement::Rollback => Ok(stmt.clone()),
        }
    }

    fn substitute_expr(&self, expr: &Expr) -> Result<Expr> {
        Ok(match expr {
            Expr::Placeholder(idx) => match self.args.get(*idx as usize).and_then(|v| v.as_ref()) {
                Some(value) => match value {
                    rdbx_query::value::Value::Null => Expr::Null,
                    rdbx_query::value::Value::Bool(v) => Expr::Bool(*v),
                    rdbx_query::value::Value::Int(v) => Expr::Int(*v),
                    rdbx_query::value::Value::Float(v) => Expr::Float(*v),
                    rdbx_query::value::Value::Bytes(v) => {
                        Expr::Str(String::from_utf8_lossy(v).to_string())
                    }
                },
                None => {
                    return Err(Error::Internal(format!(
                        "missing bound value for parameter {idx}"
                    )));
                }
            },
            Expr::Binary(l, op, r) => Expr::Binary(
                Box::new(self.substitute_expr(l)?),
                *op,
                Box::new(self.substitute_expr(r)?),
            ),
            Expr::Unary(op, inner) => Expr::Unary(*op, Box::new(self.substitute_expr(inner)?)),
            Expr::Call(name, args) => {
                let mut out = Vec::with_capacity(args.len());
                for arg in args {
                    out.push(self.substitute_expr(arg)?);
                }
                Expr::Call(name.clone(), out)
            }
            Expr::Subquery(sub) => Expr::Subquery(Box::new(self.substitute_placeholders(sub)?)),
            Expr::InSubquery(lhs, sub) => Expr::InSubquery(
                Box::new(self.substitute_expr(lhs)?),
                Box::new(self.substitute_placeholders(sub)?),
            ),
            other => other.clone(),
        })
    }
}

fn map_sql_err(e: rdbx_sql::SqlError) -> Error {
    Error::Internal(format!("SQL error: {e}"))
}

impl Database {
    fn execute_ast(&self, stmt: Statement) -> Result<QueryResult> {
        // Transaction statements are no-ops at this API level.
        match &stmt {
            Statement::Begin | Statement::Commit | Statement::Rollback => {
                return Ok(QueryResult::Affected(0));
            }
            _ => {}
        }

        match stmt.clone() {
            Statement::CreateTable { name, columns } => {
                let schema = TableSchema::from_defs(name.clone(), &columns).map_err(map_sql_err)?;
                self.create_table_locked(&name)?;
                let schema_hash = uuid::Uuid::new_v4().into_bytes();
                {
                    let mut catalog = self.catalog.lock().unwrap();
                    if let Some(desc) = catalog.tables.iter_mut().find(|t| t.name == name) {
                        desc.schema_hash = schema_hash;
                    }
                    let mut mgr = self.manager.lock().unwrap();
                    catalog.flush(&mut mgr)?;
                }
                self.schema_store
                    .store(schema_hash, rdbx_sql::codec::encode_schema(&schema))?;
                self.sql_catalog.lock().unwrap().insert(schema);
                return Ok(QueryResult::Affected(0));
            }
            Statement::DropTable { name } => {
                self.sql_catalog.lock().unwrap().remove(&name);
                self.drop_table_locked(&name)?;
                return Ok(QueryResult::Affected(0));
            }
            Statement::CreateIndex {
                table,
                columns,
                name,
            } => {
                if columns.len() != 1 {
                    return Err(Error::Internal(
                        "multi-column CREATE INDEX is not supported yet".into(),
                    ));
                }
                let mut sql_catalog = self.sql_catalog.lock().unwrap();
                let schema = sql_catalog
                    .get_mut(&table)
                    .ok_or_else(|| Error::Internal(format!("unknown table '{table}'")))?;
                let col_name = &columns[0];
                let col = schema
                    .column_index(col_name)
                    .ok_or_else(|| Error::Internal(format!("unknown column '{col_name}'")))?;
                if schema
                    .indexes
                    .iter()
                    .any(|idx| idx.name == name.as_deref().unwrap_or(col_name))
                    || schema.indexes.iter().any(|idx| idx.column == col)
                {
                    return Err(Error::Internal(format!(
                        "index already exists on column '{col_name}'"
                    )));
                }
                let mut store = self;
                let mut tree = rdbx_btree::BTree::create(&mut store)?;
                let table_desc = self
                    .table(&table)
                    .ok_or_else(|| Error::Internal(format!("unknown table '{table}'")))?;
                let user_tree = rdbx_btree::BTree::open(table_desc.root_page);
                let rows = user_tree.range(&mut store, None, None)?;
                for (_, payload) in rows {
                    let row = rdbx_query::codec::decode_row(&payload)?;
                    if let Some(value) = row.get(col).cloned() {
                        if value == rdbx_query::value::Value::Null {
                            continue;
                        }
                        let key = value
                            .encode_key()
                            .map_err(|err| Error::Internal(err.to_string()))?;
                        tree.insert(&mut store, key, payload.clone())?;
                    }
                }

                let idx_name = name
                    .clone()
                    .unwrap_or_else(|| format!("idx_{table}_{col_name}"));
                schema.indexes.push(rdbx_sql::catalog::IndexDef {
                    name: idx_name,
                    column: col,
                    root_page: tree.root(),
                });
                let schema_hash = uuid::Uuid::new_v4().into_bytes();
                let blob = rdbx_sql::codec::encode_schema(schema);
                self.schema_store.store(schema_hash, blob)?;
                {
                    let mut catalog = self.catalog.lock().unwrap();
                    if let Some(desc) = catalog.tables.iter_mut().find(|t| t.name == table) {
                        desc.schema_hash = schema_hash;
                    }
                    let mut mgr = self.manager.lock().unwrap();
                    catalog.flush(&mut mgr)?;
                }
                return Ok(QueryResult::Affected(0));
            }
            Statement::DropIndex { .. } => {
                return Err(Error::Internal("DROP INDEX not supported yet".into()));
            }
            _ => {}
        }

        let catalog = self.sql_catalog.lock().unwrap().clone();
        let table_name = statement_table(&stmt).unwrap_or("").to_string();
        let root_for = |name: &str| self.table(name).map(|t| t.root_page);
        let plan = rdbx_sql::executor::compile(&stmt, &catalog, root_for).map_err(map_sql_err)?;

        let mut txn = self.begin_transaction()?;
        let physical = optimize(plan);
        let mut rows = Vec::new();
        {
            let mut exec = execute_plan(physical, &mut txn)?;
            while let Some(row) = exec.fetch_next()? {
                rows.push(row);
            }
        }

        match &stmt {
            Statement::Select {
                items, group_by, ..
            } => {
                let columns = compute_columns(items, group_by, &catalog, &table_name);
                txn.rollback()?;
                Ok(QueryResult::Rows { columns, rows })
            }
            Statement::Insert { values, .. } => {
                txn.commit()?;
                Ok(QueryResult::Affected(values.len() as u64))
            }
            Statement::Update { .. } => {
                txn.commit()?;
                Ok(QueryResult::Affected(affected_count(&rows)))
            }
            Statement::Delete { .. } => {
                txn.commit()?;
                Ok(QueryResult::Affected(affected_count(&rows)))
            }
            _ => Err(Error::Internal("unexpected statement".into())),
        }
    }

    /// Execute one SQL statement.
    pub fn execute(&self, sql: &str) -> Result<QueryResult> {
        let stmt = rdbx_sql::Parser::parse(sql).map_err(map_sql_err)?;
        self.execute_ast(stmt)
    }

    pub fn prepare(&self, sql: &str) -> Result<PreparedStatement<'_>> {
        let stmt = rdbx_sql::Parser::parse(sql).map_err(map_sql_err)?;
        let arity = match &stmt {
            Statement::Insert { .. }
            | Statement::Select { .. }
            | Statement::Update { .. }
            | Statement::Delete { .. } => count_placeholders(&stmt),
            _ => 0,
        };
        Ok(PreparedStatement {
            db: self,
            plan: rdbx_sql::PreparedPlan {
                template: stmt,
                arity,
            },
            args: vec![None; arity],
        })
    }
}

fn statement_table(stmt: &Statement) -> Option<&str> {
    match stmt {
        Statement::Select { table, .. }
        | Statement::Insert { table, .. }
        | Statement::Update { table, .. }
        | Statement::Delete { table, .. } => Some(table.as_str()),
        _ => None,
    }
}

fn compute_columns(
    items: &[SelectItem],
    group_by: &[String],
    catalog: &SqlCatalog,
    table: &str,
) -> Vec<String> {
    let schema = catalog.get(table);
    let mut out = Vec::new();
    if !group_by.is_empty() {
        out.extend(group_by.iter().cloned());
    }
    if items.len() == 1
        && let SelectItem::Expr {
            expr: Expr::Call(name, _),
            ..
        } = &items[0]
        && name.eq_ignore_ascii_case("COUNT")
    {
        return if group_by.is_empty() {
            vec!["count".to_string()]
        } else {
            out
        };
    }
    for item in items {
        match item {
            SelectItem::Wildcard => {
                if let Some(s) = schema {
                    out.extend(s.columns.iter().map(|c| c.name.clone()));
                }
            }
            SelectItem::Expr {
                expr: Expr::Column(name),
                ..
            } => out.push(name.clone()),
            SelectItem::Expr {
                expr: Expr::Call(name, _),
                ..
            } => out.push(name.to_ascii_lowercase()),
            SelectItem::Expr { .. } => out.push("?".to_string()),
        }
    }
    out
}

fn count_placeholders(stmt: &Statement) -> usize {
    let mut count = 0usize;
    fn walk_expr(expr: &Expr, count: &mut usize) {
        match expr {
            Expr::Placeholder(_) => *count += 1,
            Expr::Binary(l, _, r) => {
                walk_expr(l, count);
                walk_expr(r, count);
            }
            Expr::Unary(_, inner) => walk_expr(inner, count),
            Expr::Call(_, args) => {
                for arg in args {
                    walk_expr(arg, count);
                }
            }
            Expr::Subquery(sub) => *count += count_placeholders(sub),
            Expr::InSubquery(lhs, sub) => {
                walk_expr(lhs, count);
                *count += count_placeholders(sub);
            }
            _ => {}
        }
    }
    match stmt {
        Statement::Select { items, filter, .. } => {
            for item in items {
                if let SelectItem::Expr { expr, .. } = item {
                    walk_expr(expr, &mut count);
                }
            }
            if let Some(f) = filter {
                walk_expr(f, &mut count);
            }
        }
        Statement::Insert { values, .. } => {
            for row in values {
                for expr in row {
                    walk_expr(expr, &mut count);
                }
            }
        }
        Statement::Update {
            assignments,
            filter,
            ..
        } => {
            for (_, expr) in assignments {
                walk_expr(expr, &mut count);
            }
            if let Some(f) = filter {
                walk_expr(f, &mut count);
            }
        }
        Statement::Delete {
            filter: Some(f), ..
        } => walk_expr(f, &mut count),
        Statement::Delete { filter: None, .. } => {}
        _ => {}
    }
    count
}

fn affected_count(rows: &[Row]) -> u64 {
    rows.iter()
        .filter_map(|row| row.first())
        .filter_map(|value| match value {
            rdbx_query::value::Value::Int(n) => Some(*n as u64),
            _ => None,
        })
        .next()
        .unwrap_or(0)
}
