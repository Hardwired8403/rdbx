//! SQL-side catalog: schema for each table.

use std::collections::HashMap;

use rdbx_core::PageId;

use crate::ast::{ColumnDef, DataType};
use crate::error::{SqlError, SqlResult};

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSchema {
    pub name: String,
    pub ty: DataType,
    pub not_null: bool,
    pub primary_key: bool,
    pub unique: bool,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexDef {
    pub name: String,
    pub column: usize,
    pub root_page: PageId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
    pub primary_key: Option<usize>,
    pub indexes: Vec<IndexDef>,
}

impl TableSchema {
    pub fn from_defs(name: impl Into<String>, defs: &[ColumnDef]) -> SqlResult<Self> {
        let name = name.into();
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
                                "table {name}: multiple PRIMARY KEY columns not supported"
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
            name,
            columns,
            primary_key,
            indexes: Vec::new(),
        })
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        let lower = name.to_ascii_lowercase();
        self.columns
            .iter()
            .position(|c| c.name.to_ascii_lowercase() == lower)
    }

    pub fn coerce(
        &self,
        column: usize,
        value: &rdbx_query::value::Value,
    ) -> SqlResult<rdbx_query::value::Value> {
        use rdbx_query::value::Value;
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
    pub fn get_mut(&mut self, name: &str) -> Option<&mut TableSchema> {
        self.tables.get_mut(&name.to_ascii_lowercase())
    }
    pub fn remove(&mut self, name: &str) -> Option<TableSchema> {
        self.tables.remove(&name.to_ascii_lowercase())
    }
    pub fn tables(&self) -> impl Iterator<Item = &TableSchema> {
        self.tables.values()
    }
}
