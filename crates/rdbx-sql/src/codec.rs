//! Serialization of `TableSchema` to / from bytes for the persistent
//! schema store.

use rdbx_core::{Error, Result};

use crate::ast::DataType;
use crate::catalog::{ColumnSchema, IndexDef, TableSchema};

/// Binary encoding:
///
/// ```text
/// version: u8 = 1
/// column_count: u32
/// for each column:
///     name_len: u16 | name | ty: u8 | flags: u8 | index: u32
/// primary_key: u8 (0xFF = none)  -- actually u32 with 0xFFFF_FFFF for none
/// index_count: u32
/// for each index:
///     name_len: u16 | name | column: u32
/// ```
const VERSION: u8 = 1;

fn encode_ty(ty: DataType) -> u8 {
    match ty {
        DataType::Integer => 0,
        DataType::BigInt => 1,
        DataType::Text => 2,
        DataType::Blob => 3,
        DataType::Bool => 4,
        DataType::Float => 5,
        DataType::Double => 6,
    }
}

fn decode_ty(tag: u8) -> Result<DataType> {
    Ok(match tag {
        0 => DataType::Integer,
        1 => DataType::BigInt,
        2 => DataType::Text,
        3 => DataType::Blob,
        4 => DataType::Bool,
        5 => DataType::Float,
        6 => DataType::Double,
        other => {
            return Err(Error::InvalidFile(format!("unknown DataType tag {other}")));
        }
    })
}

pub fn encode_schema(schema: &TableSchema) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(VERSION);
    out.extend_from_slice(&(schema.columns.len() as u32).to_le_bytes());
    for col in &schema.columns {
        out.extend_from_slice(&(col.name.len() as u16).to_le_bytes());
        out.extend_from_slice(col.name.as_bytes());
        out.push(encode_ty(col.ty));
        let flags = (u8::from(col.not_null))
            | (u8::from(col.primary_key) << 1)
            | (u8::from(col.unique) << 2);
        out.push(flags);
        out.extend_from_slice(&(col.index as u32).to_le_bytes());
    }
    let pk = schema.primary_key.unwrap_or(u32::MAX as usize) as u32;
    out.extend_from_slice(&pk.to_le_bytes());
    out.extend_from_slice(&(schema.indexes.len() as u32).to_le_bytes());
    for idx in &schema.indexes {
        out.extend_from_slice(&(idx.name.len() as u16).to_le_bytes());
        out.extend_from_slice(idx.name.as_bytes());
        out.extend_from_slice(&(idx.column as u32).to_le_bytes());
    }
    out
}

pub fn decode_schema(bytes: &[u8]) -> Result<TableSchema> {
    if bytes.len() < 5 || bytes[0] != VERSION {
        return Err(Error::InvalidFile("bad schema blob version".into()));
    }
    let mut p = 1usize;
    let col_count = read_u32(bytes, &mut p)? as usize;
    let mut columns = Vec::with_capacity(col_count);
    for _ in 0..col_count {
        let name_len = read_u16(bytes, &mut p)? as usize;
        let name = read_str(bytes, &mut p, name_len)?;
        let ty_tag = read_u8(bytes, &mut p)?;
        let ty = decode_ty(ty_tag)?;
        let flags = read_u8(bytes, &mut p)?;
        let index = read_u32(bytes, &mut p)? as usize;
        columns.push(ColumnSchema {
            name,
            ty,
            not_null: flags & 1 != 0,
            primary_key: flags & 2 != 0,
            unique: flags & 4 != 0,
            index,
        });
    }
    let pk = read_u32(bytes, &mut p)?;
    let primary_key = if pk == u32::MAX {
        None
    } else {
        Some(pk as usize)
    };
    let idx_count = read_u32(bytes, &mut p)? as usize;
    let mut indexes = Vec::with_capacity(idx_count);
    for _ in 0..idx_count {
        let name_len = read_u16(bytes, &mut p)? as usize;
        let name = read_str(bytes, &mut p, name_len)?;
        let column = read_u32(bytes, &mut p)? as usize;
        indexes.push(IndexDef { name, column });
    }
    Ok(TableSchema {
        name: String::new(), // name lives in the key
        columns,
        primary_key,
        indexes,
    })
}

fn read_u8(b: &[u8], p: &mut usize) -> Result<u8> {
    let v = *b
        .get(*p)
        .ok_or_else(|| Error::InvalidFile("schema short".into()))?;
    *p += 1;
    Ok(v)
}

fn read_u16(b: &[u8], p: &mut usize) -> Result<u16> {
    if *p + 2 > b.len() {
        return Err(Error::InvalidFile("schema short".into()));
    }
    let v = u16::from_le_bytes(b[*p..*p + 2].try_into().unwrap());
    *p += 2;
    Ok(v)
}

fn read_u32(b: &[u8], p: &mut usize) -> Result<u32> {
    if *p + 4 > b.len() {
        return Err(Error::InvalidFile("schema short".into()));
    }
    let v = u32::from_le_bytes(b[*p..*p + 4].try_into().unwrap());
    *p += 4;
    Ok(v)
}

fn read_str(b: &[u8], p: &mut usize, len: usize) -> Result<String> {
    if *p + len > b.len() {
        return Err(Error::InvalidFile("schema short".into()));
    }
    let s = std::str::from_utf8(&b[*p..*p + len])
        .map_err(|_| Error::InvalidFile("schema name not UTF-8".into()))?
        .to_string();
    *p += len;
    Ok(s)
}
