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
                self.sql_catalog.lock().unwrap().insert(schema);
                return Ok(QueryResult::Affected(0));
            }
            Statement::DropTable { name } => {
                self.sql_catalog.lock().unwrap().remove(&name);
                self.drop_table_locked(&name)?;
                return Ok(QueryResult::Affected(0));
            }
            Statement::CreateIndex { .. } | Statement::DropIndex { .. } => {
                return Err(Error::Internal(
                    "CREATE INDEX / DROP INDEX is not supported yet".into(),
                ));
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
                Ok(QueryResult::Affected(0))
            }
            Statement::Delete { .. } => {
                txn.commit()?;
                Ok(QueryResult::Affected(0))
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
