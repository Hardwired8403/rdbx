//! Typed values and rows.

use rdbx_core::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Bytes(Vec<u8>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Ordering-preserving B+Tree key encoding.
    pub fn encode_key(&self) -> Result<Vec<u8>> {
        match self {
            Value::Null => Err(Error::Internal("cannot use NULL as a B+Tree key".into())),
            Value::Bytes(v) => {
                let mut out = Vec::with_capacity(1 + v.len());
                out.push(0x00);
                out.extend_from_slice(v);
                Ok(out)
            }
            Value::Int(i) => {
                let u = (*i as u64) ^ (1u64 << 63);
                let mut out = Vec::with_capacity(9);
                out.push(0x01);
                let mut bytes = [0u8; 8];
                let mut v = u;
                for x in bytes.iter_mut().rev() {
                    *x = (v & 0xFF) as u8;
                    v >>= 8;
                }
                out.extend_from_slice(&bytes);
                Ok(out)
            }
            Value::Bool(b) => Ok(vec![0x02, u8::from(*b)]),
            Value::Float(f) => {
                let bits = f.to_bits() as i64;
                let ordered = if bits < 0 { !bits } else { bits ^ (1i64 << 63) };
                let u = ordered as u64;
                let mut out = Vec::with_capacity(9);
                out.push(0x03);
                let mut bytes = [0u8; 8];
                let mut v = u;
                for x in bytes.iter_mut().rev() {
                    *x = (v & 0xFF) as u8;
                    v >>= 8;
                }
                out.extend_from_slice(&bytes);
                Ok(out)
            }
        }
    }

    pub fn encode_value(&self) -> Vec<u8> {
        match self {
            Value::Null => vec![0x04],
            Value::Bytes(v) => {
                let mut out = Vec::with_capacity(5 + v.len());
                out.push(0x00);
                out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                out.extend_from_slice(v);
                out
            }
            Value::Int(i) => {
                let mut out = Vec::with_capacity(9);
                out.push(0x01);
                out.extend_from_slice(&i.to_le_bytes());
                out
            }
            Value::Bool(b) => vec![0x02, u8::from(*b)],
            Value::Float(f) => {
                let mut out = Vec::with_capacity(9);
                out.push(0x03);
                out.extend_from_slice(&f.to_le_bytes());
                out
            }
        }
    }

    pub fn decode_value(bytes: &[u8]) -> Result<Self> {
        let Some(&tag) = bytes.first() else {
            return Err(Error::Internal("empty Value bytes".into()));
        };
        Ok(match tag {
            0x00 => {
                if bytes.len() < 5 {
                    return Err(Error::Internal("truncated Bytes value".into()));
                }
                let len = u32::from_le_bytes(bytes[1..5].try_into().unwrap()) as usize;
                if bytes.len() < 5 + len {
                    return Err(Error::Internal("truncated Bytes payload".into()));
                }
                Value::Bytes(bytes[5..5 + len].to_vec())
            }
            0x01 => {
                if bytes.len() < 9 {
                    return Err(Error::Internal("truncated Int value".into()));
                }
                Value::Int(i64::from_le_bytes(bytes[1..9].try_into().unwrap()))
            }
            0x02 => {
                if bytes.len() < 2 {
                    return Err(Error::Internal("truncated Bool value".into()));
                }
                Value::Bool(bytes[1] != 0)
            }
            0x03 => {
                if bytes.len() < 9 {
                    return Err(Error::Internal("truncated Float value".into()));
                }
                Value::Float(f64::from_le_bytes(bytes[1..9].try_into().unwrap()))
            }
            0x04 => Value::Null,
            other => {
                return Err(Error::Internal(format!("unknown Value tag {other:#x}")));
            }
        })
    }
}

/// One row of results.
pub type Row = Vec<Value>;
