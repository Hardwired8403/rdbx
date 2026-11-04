//! Write-ahead log and crash recovery for RDBX.
//!
//! Phase 3 implements a steal/no-force ARIES-lite WAL:
//!
//! * Every page mutation is first appended as a [`WalRecord`] with kind
//!   `Update`, carrying both before and after images of the page.
//! * A transaction commits only after its `Commit` record is durably
//!   on disk (`wal.flush()` before `page.flush()`).
//! * Recovery performs `Analysis → Redo (repeating history) → Undo
//!   (uncommitted transactions)`.
//!
//! See `docs/transactions/WAL.md` for the on-disk format and the
//! durability matrix.

pub mod manager;
pub mod record;
pub mod recovery;
pub mod segment;

pub use manager::{SegmentInfo, WalManager};
pub use record::{RECORD_OVERHEAD, RecordKind, WalRecord};
pub use recovery::{RecoveryReport, RecoveryStopReason, recover};
pub use segment::{SEGMENT_HEADER_SIZE, SegmentHeader};
