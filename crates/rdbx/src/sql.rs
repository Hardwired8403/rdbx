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

fn map_sql_err(e: rdbx_sql::SqlError) -> Error {
    Error::Internal(format!("SQL error: {e}"))
}

impl Database {
    /// Execute one SQL statement.
    pub fn execute(&self, sql: &str) -> Result<QueryResult> {
        let stmt = rdbx_sql::Parser::parse(sql).map_err(map_sql_err)?;

        // Transaction statements are no-ops at this API level.
        match &stmt {
            Statement::Begin | Statement::Commit | Statement::Rollback => {
                return Ok(QueryResult::Affected(0));
            }
            _ => {}
        }

        // DDL.
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
            Statement::CreateIndex { .. } => {
                return Err(Error::Internal("CREATE INDEX not supported yet".into()));
            }
            Statement::DropIndex { .. } => {
                return Err(Error::Internal("DROP INDEX not supported yet".into()));
            }
            _ => {}
        }

        // DML / SELECT.
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
            Statement::Select { items, .. } => {
                let columns = compute_columns(items, &catalog, &table_name);
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

fn compute_columns(items: &[SelectItem], catalog: &SqlCatalog, table: &str) -> Vec<String> {
    let schema = catalog.get(table);
    let mut out = Vec::new();
    if items.len() == 1
        && let SelectItem::Expr {
            expr: Expr::Call(name, _),
            ..
        } = &items[0]
        && name.eq_ignore_ascii_case("COUNT")
    {
        return vec!["count".to_string()];
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
            SelectItem::Expr { .. } => out.push("?".to_string()),
        }
    }
    out
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
