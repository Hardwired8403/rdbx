//! Error type for RDBX.

use std::io;
use std::path::PathBuf;
use thiserror::Error;

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors surfaced by RDBX.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid database file: {0}")]
    InvalidFile(String),

    #[error(
        "unsupported format version {major}.{minor} \
         (supported: {supported_major}.{supported_minor})"
    )]
    UnsupportedVersion {
        major: u16,
        minor: u16,
        supported_major: u16,
        supported_minor: u16,
    },

    #[error(
        "checksum mismatch on page {page_id}: \
         stored {expected:#010x}, computed {actual:#010x}"
    )]
    ChecksumMismatch {
        page_id: u64,
        expected: u32,
        actual: u32,
    },

    #[error("page {0} out of bounds")]
    PageOutOfBounds(u64),

    #[error("page {0} has unexpected type tag {1}")]
    InvalidPageType(u64, u8),

    #[error("integer overflow in {0}")]
    Overflow(&'static str),

    #[error("database at {path} is already open")]
    AlreadyOpen { path: PathBuf },

    #[error("database is closed")]
    Closed,

    #[error("internal error: {0}")]
    Internal(String),
}
