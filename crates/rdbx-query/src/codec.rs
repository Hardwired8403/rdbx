//! Row encoding for B+Tree values.
//!
//! A row is stored as:
//!
//! ```text
//! field_count: u16
//! field_0: Value::encode_value
//! field_1: Value::encode_value
//! ...
//! ```
//!
//! The decoding side reads fields until `field_count` values have been
//! produced.

use rdbx_core::{Error, Result};

use crate::value::{Row, Value};

pub fn encode_row(row: &Row) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + row.len() * 9);
    out.extend_from_slice(&(row.len() as u16).to_le_bytes());
    for v in row {
        out.extend_from_slice(&v.encode_value());
    }
    out
}

pub fn decode_row(mut bytes: &[u8]) -> Result<Row> {
    if bytes.len() < 2 {
        return Err(Error::Internal("row shorter than field count".into()));
    }
    let count = u16::from_le_bytes(bytes[..2].try_into().unwrap()) as usize;
    bytes = &bytes[2..];
    let mut row = Vec::with_capacity(count);
    for _ in 0..count {
        let (value, consumed) = decode_one(bytes)?;
        row.push(value);
        bytes = &bytes[consumed..];
    }
    Ok(row)
}

fn decode_one(bytes: &[u8]) -> Result<(Value, usize)> {
    let Some(&tag) = bytes.first() else {
        return Err(Error::Internal("empty Value bytes".into()));
    };
    let size = match tag {
        0x00 => {
            if bytes.len() < 5 {
                return Err(Error::Internal("truncated Bytes header".into()));
            }
            let len = u32::from_le_bytes(bytes[1..5].try_into().unwrap()) as usize;
            5 + len
        }
        0x01 | 0x03 => 9,
        0x02 => 2,
        0x04 => 1,
        other => {
            return Err(Error::Internal(format!("unknown Value tag {other:#x}")));
        }
    };
    if bytes.len() < size {
        return Err(Error::Internal("truncated Value".into()));
    }
    let value = Value::decode_value(&bytes[..size])?;
    Ok((value, size))
}

/// Marker type kept for tests; no runtime use.
#[allow(dead_code)]
pub struct CodecMarker;
