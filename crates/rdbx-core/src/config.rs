//! Database configuration and durability options.

use crate::{Error, Result};

/// Default page size: 16 KiB.
pub const DEFAULT_PAGE_SIZE: usize = 16 * 1024;
/// Minimum allowed page size.
pub const MIN_PAGE_SIZE: usize = 4 * 1024;
/// Maximum allowed page size.
pub const MAX_PAGE_SIZE: usize = 64 * 1024;

/// Default buffer pool capacity in frames (256 × 16 KiB = 4 MiB).
pub const DEFAULT_BUFFER_POOL_CAPACITY: usize = 256;

/// Durability policy for `flush` operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// `fsync` on every flush (safest, slowest).
    Full,
    /// `fsync` on checkpoint/close (fast; may lose recent commits on crash).
    Normal,
    /// No `fsync` (only for ephemeral data).
    Off,
}

/// Eviction policy implemented by the buffer pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionPolicyKind {
    /// Least Recently Used.
    Lru,
    /// Clock (second-chance).
    Clock,
}

/// Options passed to [`crate::Database`] at open time.
#[derive(Debug, Clone)]
pub struct DatabaseOptions {
    /// Page size in bytes; must be a power of two in \[4 KiB, 64 KiB\].
    pub page_size: usize,
    /// Create the database if missing (only used by `Database::open`).
    pub create_if_missing: bool,
    /// Open read-only.
    pub read_only: bool,
    /// Durability policy.
    pub sync_mode: SyncMode,
    /// Number of frames held by the buffer pool. Must be > 0.
    pub buffer_pool_capacity: usize,
    /// Buffer pool eviction policy.
    pub buffer_pool_policy: EvictionPolicyKind,
}

impl Default for DatabaseOptions {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            create_if_missing: true,
            read_only: false,
            sync_mode: SyncMode::Full,
            buffer_pool_capacity: DEFAULT_BUFFER_POOL_CAPACITY,
            buffer_pool_policy: EvictionPolicyKind::Lru,
        }
    }
}

impl DatabaseOptions {
    /// Validate the options, returning a descriptive error on failure.
    pub fn validate(&self) -> Result<()> {
        if self.page_size < MIN_PAGE_SIZE || self.page_size > MAX_PAGE_SIZE {
            return Err(Error::Internal(format!(
                "page_size {} out of range [{MIN_PAGE_SIZE}, {MAX_PAGE_SIZE}]",
                self.page_size
            )));
        }
        if !self.page_size.is_power_of_two() {
            return Err(Error::Internal("page_size must be a power of two".into()));
        }
        if self.buffer_pool_capacity == 0 {
            return Err(Error::Internal(
                "buffer_pool_capacity must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}
