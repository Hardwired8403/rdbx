//! WAL segment files: on-disk layout and file handles.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rdbx_core::{Error, Lsn, Result};

/// Magic bytes at the start of every segment file.
pub const SEGMENT_MAGIC: &[u8; 8] = b"RDBXWAL\0";

/// Current segment format version.
pub const SEGMENT_FORMAT_VERSION: u16 = 1;

/// Fixed segment header size.
pub const SEGMENT_HEADER_SIZE: usize = 64;

/// Header at the start of every WAL segment.
///
/// Serialized format (little-endian):
///
/// ```text
/// offset  size  field
///   0      8    magic = b"RDBXWAL\0"
///   8      2    format_version (u16)
///  10      2    flags          (u16, 0)
///  12      4    reserved       (u32, 0)
///  16      8    base_lsn       (u64)
///  24      8    segment_id     (u64)
///  32     16    file_uuid      ([u8; 16])
///  48      8    created_at     (u64 unix nanos)
///  56      4    reserved2      (u32, 0)
///  60      4    header_crc     (crc32 of bytes 0..60)
/// ```
#[derive(Debug, Clone)]
pub struct SegmentHeader {
    pub format_version: u16,
    pub base_lsn: Lsn,
    pub segment_id: u64,
    pub file_uuid: [u8; 16],
    pub created_at_unix_nanos: u64,
}

impl SegmentHeader {
    pub fn encode(&self) -> [u8; SEGMENT_HEADER_SIZE] {
        let mut buf = [0u8; SEGMENT_HEADER_SIZE];
        buf[0..8].copy_from_slice(SEGMENT_MAGIC);
        buf[8..10].copy_from_slice(&self.format_version.to_le_bytes());
        buf[10..12].fill(0);
        buf[12..16].fill(0);
        buf[16..24].copy_from_slice(&self.base_lsn.0.to_le_bytes());
        buf[24..32].copy_from_slice(&self.segment_id.to_le_bytes());
        buf[32..48].copy_from_slice(&self.file_uuid);
        buf[48..56].copy_from_slice(&self.created_at_unix_nanos.to_le_bytes());
        buf[56..60].fill(0);
        let crc = crc32fast::hash(&buf[0..60]);
        buf[60..64].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8; SEGMENT_HEADER_SIZE]) -> Result<Self> {
        if &buf[0..8] != SEGMENT_MAGIC {
            return Err(Error::InvalidFile("WAL: bad segment magic".into()));
        }
        let stored_crc = u32::from_le_bytes(buf[60..64].try_into().unwrap());
        let computed_crc = crc32fast::hash(&buf[0..60]);
        if stored_crc != computed_crc {
            return Err(Error::InvalidFile(
                "WAL: segment header crc mismatch".into(),
            ));
        }
        let format_version = u16::from_le_bytes(buf[8..10].try_into().unwrap());
        if format_version != SEGMENT_FORMAT_VERSION {
            return Err(Error::InvalidFile(format!(
                "WAL: unsupported segment format version {format_version}"
            )));
        }
        let base_lsn = Lsn(u64::from_le_bytes(buf[16..24].try_into().unwrap()));
        let segment_id = u64::from_le_bytes(buf[24..32].try_into().unwrap());
        let mut file_uuid = [0u8; 16];
        file_uuid.copy_from_slice(&buf[32..48]);
        let created_at_unix_nanos = u64::from_le_bytes(buf[48..56].try_into().unwrap());
        Ok(SegmentHeader {
            format_version,
            base_lsn,
            segment_id,
            file_uuid,
            created_at_unix_nanos,
        })
    }
}

/// One open WAL segment file.
pub struct SegmentFile {
    file: File,
    path: PathBuf,
    pub header: SegmentHeader,
}

impl SegmentFile {
    /// Create a new segment file. Refuses to overwrite an existing one.
    pub fn create(dir: &Path, segment_id: u64, base_lsn: Lsn, file_uuid: [u8; 16]) -> Result<Self> {
        let path = segment_path(dir, segment_id);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let header = SegmentHeader {
            format_version: SEGMENT_FORMAT_VERSION,
            base_lsn,
            segment_id,
            file_uuid,
            created_at_unix_nanos: unix_nanos_now(),
        };
        let buf = header.encode();
        file.write_all(&buf)?;
        file.sync_all()?;
        Ok(SegmentFile { file, path, header })
    }

    /// Open an existing segment file. The header is validated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut header_buf = [0u8; SEGMENT_HEADER_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header_buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                Error::InvalidFile(format!("WAL: segment {} too small", path.display()))
            } else {
                Error::Io(e)
            }
        })?;
        let header = SegmentHeader::decode(&header_buf)?;
        Ok(SegmentFile { file, path, header })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append raw bytes at the current end of the file.
    pub fn append_raw(&mut self, data: &[u8]) -> Result<u64> {
        let pos = self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(data)?;
        Ok(pos)
    }

    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    pub fn len(&self) -> Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? <= SEGMENT_HEADER_SIZE as u64)
    }

    /// Read every byte after the header.
    pub fn read_records_bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.len()?;
        if len < SEGMENT_HEADER_SIZE as u64 {
            return Err(Error::InvalidFile(
                "WAL: segment shorter than header".into(),
            ));
        }
        let mut buf = vec![0u8; (len - SEGMENT_HEADER_SIZE as u64) as usize];
        self.file
            .seek(SeekFrom::Start(SEGMENT_HEADER_SIZE as u64))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn reader(&self) -> Result<BufReader<File>> {
        let file = OpenOptions::new().read(true).open(&self.path)?;
        Ok(BufReader::new(file))
    }
}

/// Canonical segment file name: `segment-{segment_id:020}.wal`.
pub fn segment_path(dir: &Path, segment_id: u64) -> PathBuf {
    dir.join(format!("segment-{segment_id:020}.wal"))
}

/// List every `segment-*.wal` file in `dir`, sorted lexicographically.
pub fn list_segments(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("wal") {
            continue;
        }
        if !p
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.starts_with("segment-"))
            .unwrap_or(false)
        {
            continue;
        }
        out.push(p);
    }
    out.sort();
    Ok(out)
}

/// Parse `segment_id` out of a path; `None` for non-conforming names.
pub fn parse_segment_id(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_prefix("segment-")?.strip_suffix(".wal")?;
    stem.parse().ok()
}

fn unix_nanos_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
