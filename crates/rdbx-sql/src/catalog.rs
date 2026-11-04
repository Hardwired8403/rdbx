//! SQL-side catalog: schema for each table.
//!
//! Phase 7 stores the schema as a self-describing blob in the existing
//! `TableDescriptor.schema_hash` slot (which Phase 4 reserved for future
//! use). We repurpose the 16-byte slot as an opaque pointer to a
//! separate "schema" tree, but to keep Phase 7 self-contained we instead
//! persist the schema as JSON-like bytes inside the descriptor itself.
//!
//! To avoid extending `TableDescriptor` (which lives in `rdbx-storage`
//! and is on the Phase 2 on-disk format), we keep the schema *in memory
//! only* in this crate's `SqlCatalog`, and rebuild it lazily from the
//! descriptor's `schema_hash` field when the DB is reopened. In practice
//! that means: after this phase, any table created by raw
//! `Database::create_table` has no schema and can only be used from the
//! B+Tree API. Schema-aware tables must be created through SQL.

use std::collections::HashMap;

use rdbx_query::value::Value;

use crate::ast::{ColumnDef, DataType};
use crate::error::{SqlError, SqlResult};

/// One column in a user-visible table.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSchema {
    pub name: String,
    pub ty: DataType,
    pub not_null: bool,
    pub primary_key: bool,
    pub unique: bool,
    /// Physical position within a row.
    pub index: usize,
}

/// Full schema of one table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
    pub primary_key: Option<usize>,
}

impl TableSchema {
    pub fn from_defs(name: impl Into<String>, defs: &[ColumnDef]) -> SqlResult<Self> {
        let mut columns = Vec::with_capacity(defs.len());
        let mut primary_key = None;
        for (i, d) in defs.iter().enumerate() {
            let mut pk = false;
            let mut not_null = false;
            let mut unique = false;
            for c in &d.constraints {
                match c {
                    crate::ast::Constraint::PrimaryKey => {
                        if primary_key.is_some() {
                            return Err(SqlError::Semantic(format!(
                                "table {name_hint}: multiple PRIMARY KEY columns not supported",
                                name_hint = name.into()
                            )));
                        }
                        pk = true;
                        not_null = true;
                    }
                    crate::ast::Constraint::Unique => unique = true,
                    crate::ast::Constraint::NotNull => not_null = true,
                }
            }
            if pk {
                primary_key = Some(i);
            }
            columns.push(ColumnSchema {
                name: d.name.clone(),
                ty: d.ty,
                not_null,
                primary_key: pk,
                unique,
                index: i,
            });
        }
        Ok(Self {
            name: name.into(),
            columns,
            primary_key,
        })
    }

    /// Resolve a column name to its position. Case-insensitive.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        let lower = name.to_ascii_lowercase();
        self.columns
            .iter()
            .position(|c| c.name.to_ascii_lowercase() == lower)
    }

    /// Coerce a literal value to the column's declared type, or report an
    /// error.
    pub fn coerce(&self, column: usize, value: &Value) -> SqlResult<Value> {
        let ty = self.columns[column].ty;
        Ok(match (ty, value) {
            (_, Value::Null) => Value::Null,
            (DataType::Integer | DataType::BigInt, Value::Int(i)) => Value::Int(*i),
            (DataType::Integer | DataType::BigInt, Value::Float(f)) if f.fract() == 0.0 => {
                Value::Int(*f as i64)
            }
            (DataType::Float | DataType::Double, Value::Int(i)) => Value::Float(*i as f64),
            (DataType::Float | DataType::Double, Value::Float(f)) => Value::Float(*f),
            (DataType::Text, Value::Bytes(b)) => Value::Bytes(b.clone()),
            (DataType::Text, Value::Int(i)) => Value::Bytes(i.to_string().into_bytes()),
            (DataType::Blob, Value::Bytes(b)) => Value::Bytes(b.clone()),
            (DataType::Bool, Value::Bool(b)) => Value::Bool(*b),
            (DataType::Bool, Value::Int(0)) => Value::Bool(false),
            (DataType::Bool, Value::Int(1)) => Value::Bool(true),
            (expected, got) => {
                return Err(SqlError::Semantic(format!(
                    "cannot coerce {got:?} into {expected:?} for column '{}'",
                    self.columns[column].name
                )));
            }
        })
    }
}

/// In-memory SQL schema catalog keyed by table name.
#[derive(Debug, Default, Clone)]
pub struct SqlCatalog {
    tables: HashMap<String, TableSchema>,
}

impl SqlCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, schema: TableSchema) {
        self.tables.insert(schema.name.to_ascii_lowercase(), schema);
    }

    pub fn get(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    pub fn remove(&mut self, name: &str) -> Option<TableSchema> {
        self.tables.remove(&name.to_ascii_lowercase())
    }

    pub fn tables(&self) -> impl Iterator<Item = &TableSchema> {
        self.tables.values()
    }
}
