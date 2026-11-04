//! WalManager: append, flush, rotate, truncate.

use std::path::{Path, PathBuf};

use rdbx_core::{Error, Lsn, PageId, Result, SyncMode, TxnId};
use uuid::Uuid;

use crate::record::{RecordKind, WalRecord};
use crate::segment::{SegmentFile, list_segments, parse_segment_id, segment_path};

/// Description of one on-disk segment.
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub path: PathBuf,
    pub segment_id: u64,
    pub base_lsn: Lsn,
    pub len_bytes: u64,
}

/// Write-ahead log manager.
pub struct WalManager {
    dir: PathBuf,
    current: SegmentFile,
    next_segment_id: u64,
    last_lsn: Lsn,
    last_flushed_lsn: Lsn,
    file_uuid: [u8; 16],
    sync_mode: SyncMode,
    segment_size_limit: u64,
}

impl WalManager {
    /// Open (or create) the WAL directory and set up the current segment.
    ///
    /// `Lsn(0)` means "nothing has been logged yet"; the first record
    /// appended will carry `Lsn(1)`.
    pub fn open(dir: impl AsRef<Path>, sync_mode: SyncMode) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let segments = list_segments(&dir)?;

        let (current, next_segment_id, last_lsn, file_uuid) = if segments.is_empty() {
            let uuid = *Uuid::new_v4().as_bytes();
            // base_lsn = 1 because the first record will be Lsn(1).
            let seg = SegmentFile::create(&dir, 1, Lsn(1), uuid)?;
            (seg, 2u64, Lsn(0), uuid)
        } else {
            let mut max_id = 0u64;
            let mut max_lsn = Lsn(0);
            let mut uuid = [0u8; 16];
            for p in &segments {
                let id = parse_segment_id(p).ok_or_else(|| {
                    Error::InvalidFile(format!(
                        "WAL: unexpected file in segment dir: {}",
                        p.display()
                    ))
                })?;
                max_id = max_id.max(id);
                let mut seg = SegmentFile::open(p)?;
                uuid = seg.header.file_uuid;
                let bytes = seg.read_records_bytes()?;
                let records = WalRecord::decode_all(&bytes)?;
                // If the segment has records, the last one's LSN is the
                // highest emitted so far. An empty segment stores its
                // "first possible LSN" in `base_lsn`, so the highest
                // emitted is `base_lsn - 1`.
                let highest = records
                    .last()
                    .map(|r| r.lsn)
                    .unwrap_or_else(|| Lsn(seg.header.base_lsn.0.saturating_sub(1)));
                max_lsn = max_lsn.max(highest);
            }
            let last_path = segment_path(&dir, max_id);
            let seg = SegmentFile::open(&last_path)?;
            (seg, max_id + 1, max_lsn, uuid)
        };

        Ok(WalManager {
            dir,
            current,
            next_segment_id,
            last_lsn,
            last_flushed_lsn: last_lsn,
            file_uuid,
            sync_mode,
            segment_size_limit: 4 * 1024 * 1024,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn last_lsn(&self) -> Lsn {
        self.last_lsn
    }
    pub fn last_flushed_lsn(&self) -> Lsn {
        self.last_flushed_lsn
    }
    pub fn current_segment_id(&self) -> u64 {
        self.current.header.segment_id
    }

    /// Append a record and return its LSN. Flushes per `SyncMode`.
    pub fn append(
        &mut self,
        txn_id: TxnId,
        kind: RecordKind,
        page_id: PageId,
        before: Vec<u8>,
        after: Vec<u8>,
    ) -> Result<Lsn> {
        let next_lsn = self
            .last_lsn
            .0
            .checked_add(1)
            .ok_or(Error::Overflow("WAL LSN"))?;
        let lsn = Lsn(next_lsn);
        let record = WalRecord {
            lsn,
            txn_id,
            kind,
            page_id,
            before,
            after,
        };
        let encoded = record.encode()?;
        self.current.append_raw(&encoded)?;
        self.last_lsn = lsn;

        if self.sync_mode == SyncMode::Full {
            self.current.sync()?;
            self.last_flushed_lsn = lsn;
        }

        self.maybe_rotate()?;
        Ok(lsn)
    }

    /// Force a durable flush.
    pub fn flush(&mut self) -> Result<()> {
        if self.last_flushed_lsn.0 < self.last_lsn.0 {
            self.current.sync()?;
            self.last_flushed_lsn = self.last_lsn;
        }
        Ok(())
    }

    /// Read every record across all segments, sorted by LSN.
    pub fn read_all(&self) -> Result<Vec<WalRecord>> {
        let mut out = Vec::new();
        for p in list_segments(&self.dir)? {
            let mut seg = SegmentFile::open(&p)?;
            let bytes = seg.read_records_bytes()?;
            out.extend(WalRecord::decode_all(&bytes)?);
        }
        out.sort_by_key(|r| r.lsn.0);
        Ok(out)
    }

    /// Enumerate on-disk segments.
    pub fn segment_info(&self) -> Result<Vec<SegmentInfo>> {
        let mut out = Vec::new();
        for p in list_segments(&self.dir)? {
            let seg = SegmentFile::open(&p)?;
            let len = std::fs::metadata(&p)?.len();
            out.push(SegmentInfo {
                path: p,
                segment_id: seg.header.segment_id,
                base_lsn: seg.header.base_lsn,
                len_bytes: len,
            });
        }
        Ok(out)
    }

    /// Delete all segments and start fresh. Only safe after a checkpoint
    /// (data pages durably on disk). The next emitted LSN continues from
    /// `last_lsn + 1` so page LSNs remain globally monotonic across
    /// truncations.
    pub fn truncate(&mut self) -> Result<()> {
        self.flush()?;
        for p in list_segments(&self.dir)? {
            let _ = std::fs::remove_file(p);
        }
        let new_uuid = *Uuid::new_v4().as_bytes();
        let base = self
            .last_lsn
            .0
            .checked_add(1)
            .ok_or(Error::Overflow("WAL LSN"))?;
        let seg = SegmentFile::create(&self.dir, self.next_segment_id, Lsn(base), new_uuid)?;
        self.current = seg;
        self.next_segment_id += 1;
        self.file_uuid = new_uuid;
        self.last_flushed_lsn = self.last_lsn;
        Ok(())
    }

    fn maybe_rotate(&mut self) -> Result<()> {
        if self.current.len()? < self.segment_size_limit {
            return Ok(());
        }
        self.current.sync()?;
        let base = self
            .last_lsn
            .0
            .checked_add(1)
            .ok_or(Error::Overflow("WAL LSN"))?;
        let seg = SegmentFile::create(&self.dir, self.next_segment_id, Lsn(base), self.file_uuid)?;
        self.current = seg;
        self.next_segment_id += 1;
        Ok(())
    }
}
