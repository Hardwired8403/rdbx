//! Wire format for MVCC versioned values.
//!
//! A [`VersionedValue`] is stored as a single B+Tree value:
//!
//! ```text
//! offset  size  field
//!   0      4    version_count : u32
//!   4    ...    repeated Version:
//!                  txn_id     : u64
//!                  commit_ts  : u64
//!                  tombstone  : u8
//!                  value_len  : u32
//!                  value      : [u8; value_len]
//! ```
//!
//! Explicit little-endian serialization, no `repr(Rust)` dependence.

use rdbx_core::{Error, Lsn, Result, TxnId};

use crate::value::{Version, VersionedValue};

const FIXED_HEADER: usize = 4;
const VERSION_FIXED: usize = 8 + 8 + 1 + 4;

pub fn encode(value: &VersionedValue) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        FIXED_HEADER
            + value
                .versions
                .iter()
                .map(|v| VERSION_FIXED + v.value.len())
                .sum::<usize>(),
    );
    buf.extend_from_slice(&(value.versions.len() as u32).to_le_bytes());
    for v in &value.versions {
        buf.extend_from_slice(&v.txn_id.0.to_le_bytes());
        buf.extend_from_slice(&v.commit_ts.0.to_le_bytes());
        buf.push(u8::from(v.tombstone));
        buf.extend_from_slice(&(v.value.len() as u32).to_le_bytes());
        buf.extend_from_slice(&v.value);
    }
    buf
}

pub fn decode(buf: &[u8]) -> Result<VersionedValue> {
    if buf.len() < FIXED_HEADER {
        return Err(Error::InvalidFile(
            "MVCC codec: buffer shorter than header".into(),
        ));
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let mut versions = Vec::with_capacity(count);
    let mut off = FIXED_HEADER;
    for _ in 0..count {
        if off + VERSION_FIXED > buf.len() {
            return Err(Error::InvalidFile("MVCC codec: version truncated".into()));
        }
        let txn_id = TxnId(u64::from_le_bytes(buf[off..off + 8].try_into().unwrap()));
        off += 8;
        let commit_ts = Lsn(u64::from_le_bytes(buf[off..off + 8].try_into().unwrap()));
        off += 8;
        let tombstone = buf[off] != 0;
        off += 1;
        let value_len = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        if off + value_len > buf.len() {
            return Err(Error::InvalidFile("MVCC codec: value truncated".into()));
        }
        let value = buf[off..off + value_len].to_vec();
        off += value_len;
        versions.push(Version {
            txn_id,
            commit_ts,
            tombstone,
            value,
        });
    }
    Ok(VersionedValue { versions })
}
