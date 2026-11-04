//! WAL record format and (de)serialization.

use rdbx_core::{Error, Lsn, PageId, Result, TxnId};

/// Fixed serialized record header size (excluding images and crc).
pub const RECORD_HEADER_SIZE: usize = 40;
/// Trailing crc size.
pub const RECORD_CRC_SIZE: usize = 4;
/// Total fixed overhead per record.
pub const RECORD_OVERHEAD: usize = RECORD_HEADER_SIZE + RECORD_CRC_SIZE;

/// Kind of record stored in the log.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Begin = 0,
    Update = 1,
    Commit = 2,
    Abort = 3,
    Checkpoint = 4,
}

impl RecordKind {
    pub fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => RecordKind::Begin,
            1 => RecordKind::Update,
            2 => RecordKind::Commit,
            3 => RecordKind::Abort,
            4 => RecordKind::Checkpoint,
            other => {
                return Err(Error::InvalidFile(format!(
                    "WAL: unknown record kind {other}"
                )));
            }
        })
    }
}

/// One WAL record.
///
/// Serialized format (little-endian):
///
/// ```text
/// offset  size  field
///   0      4    total_len  (u32) = 40 + before_len + after_len + 4
///   4      1    kind       (u8)
///   5      3    reserved   (zero)
///   8      8    lsn        (u64)
///  16      8    txn_id     (u64)
///  24      8    page_id    (u64; 0 for Begin/Commit/Abort/Checkpoint)
///  32      4    before_len (u32)
///  36      4    after_len  (u32)
///  40    ...    before_img (before_len bytes)
///   ..   ...    after_img  (after_len bytes)
///  ..      4    crc        (crc32 of bytes 0..total_len-4)
/// ```
#[derive(Debug, Clone)]
pub struct WalRecord {
    pub lsn: Lsn,
    pub txn_id: TxnId,
    pub kind: RecordKind,
    pub page_id: PageId,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

impl WalRecord {
    pub fn begin(lsn: Lsn, txn_id: TxnId) -> Self {
        Self {
            lsn,
            txn_id,
            kind: RecordKind::Begin,
            page_id: PageId(0),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    pub fn update(
        lsn: Lsn,
        txn_id: TxnId,
        page_id: PageId,
        before: Vec<u8>,
        after: Vec<u8>,
    ) -> Self {
        Self {
            lsn,
            txn_id,
            kind: RecordKind::Update,
            page_id,
            before,
            after,
        }
    }

    pub fn commit(lsn: Lsn, txn_id: TxnId) -> Self {
        Self {
            lsn,
            txn_id,
            kind: RecordKind::Commit,
            page_id: PageId(0),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    pub fn abort(lsn: Lsn, txn_id: TxnId) -> Self {
        Self {
            lsn,
            txn_id,
            kind: RecordKind::Abort,
            page_id: PageId(0),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    pub fn checkpoint(lsn: Lsn, txn_id: TxnId) -> Self {
        Self {
            lsn,
            txn_id,
            kind: RecordKind::Checkpoint,
            page_id: PageId(0),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    /// Total serialized length.
    pub fn encoded_len(&self) -> usize {
        RECORD_OVERHEAD + self.before.len() + self.after.len()
    }

    /// Serialize with a fresh CRC.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let before_len = u32::try_from(self.before.len())
            .map_err(|_| Error::Internal("WAL: before image too large".into()))?;
        let after_len = u32::try_from(self.after.len())
            .map_err(|_| Error::Internal("WAL: after image too large".into()))?;
        let total_len = u32::try_from(self.encoded_len())
            .map_err(|_| Error::Internal("WAL: record too large".into()))?;
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.extend_from_slice(&total_len.to_le_bytes());
        buf.push(self.kind as u8);
        buf.extend_from_slice(&[0u8; 3]);
        buf.extend_from_slice(&self.lsn.0.to_le_bytes());
        buf.extend_from_slice(&self.txn_id.0.to_le_bytes());
        buf.extend_from_slice(&self.page_id.0.to_le_bytes());
        buf.extend_from_slice(&before_len.to_le_bytes());
        buf.extend_from_slice(&after_len.to_le_bytes());
        debug_assert_eq!(buf.len(), RECORD_HEADER_SIZE);
        buf.extend_from_slice(&self.before);
        buf.extend_from_slice(&self.after);
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(buf)
    }

    /// Decode one record starting at `bytes[offset]`. Returns `None` if the
    /// buffer ends before a full record (clean truncation at the tail).
    pub fn decode_from(bytes: &[u8], offset: usize) -> Result<Option<(Self, usize)>> {
        let remaining = bytes.len().saturating_sub(offset);
        if remaining < RECORD_OVERHEAD {
            return Ok(None);
        }
        let head = &bytes[offset..offset + RECORD_HEADER_SIZE];
        let total_len = u32::from_le_bytes(head[0..4].try_into().unwrap()) as usize;
        if total_len < RECORD_OVERHEAD || total_len > remaining {
            return Ok(None);
        }
        let kind = RecordKind::from_u8(head[4])?;
        let lsn = Lsn(u64::from_le_bytes(head[8..16].try_into().unwrap()));
        let txn_id = TxnId(u64::from_le_bytes(head[16..24].try_into().unwrap()));
        let page_id = PageId(u64::from_le_bytes(head[24..32].try_into().unwrap()));
        let before_len = u32::from_le_bytes(head[32..36].try_into().unwrap()) as usize;
        let after_len = u32::from_le_bytes(head[36..40].try_into().unwrap()) as usize;
        if RECORD_OVERHEAD + before_len + after_len != total_len {
            return Err(Error::InvalidFile(format!(
                "WAL: record length mismatch (declared {total_len}, \
                 computed {})",
                RECORD_OVERHEAD + before_len + after_len
            )));
        }
        let record_end = offset + total_len;
        let stored_crc = u32::from_le_bytes(bytes[record_end - 4..record_end].try_into().unwrap());
        let computed_crc = crc32fast::hash(&bytes[offset..record_end - 4]);
        if stored_crc != computed_crc {
            return Err(Error::InvalidFile(format!(
                "WAL: record crc mismatch at offset {offset}"
            )));
        }
        let before_start = offset + RECORD_HEADER_SIZE;
        let before_end = before_start + before_len;
        let after_start = before_end;
        let after_end = after_start + after_len;
        let before = bytes[before_start..before_end].to_vec();
        let after = bytes[after_start..after_end].to_vec();
        Ok(Some((
            WalRecord {
                lsn,
                txn_id,
                kind,
                page_id,
                before,
                after,
            },
            total_len,
        )))
    }

    /// Decode every complete, valid record in `bytes`. Stops at the first
    /// truncated tail; returns an error on the first bad crc or bad kind.
    pub fn decode_all(bytes: &[u8]) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while let Some((rec, consumed)) = Self::decode_from(bytes, offset)? {
            offset += consumed;
            out.push(rec);
        }
        Ok(out)
    }
}
