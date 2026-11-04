//! Top-level `Database` handle.

use std::path::{Path, PathBuf};

use rdbx_core::{DatabaseOptions, Error, Result};
use rdbx_storage::PageManager;

/// The embedded database handle.
///
/// In Phase 1 this wraps the page manager and provides lifecycle operations.
/// Phase 5 will add `begin_transaction`; Phase 7 will add `execute(sql)`.
pub struct Database {
    path: PathBuf,
    inner: PageManager,
}

impl Database {
    /// Open an existing database, or create it when `opts.create_if_missing` is true.
    pub fn open(path: impl AsRef<Path>, opts: DatabaseOptions) -> Result<Self> {
        opts.validate()?;
        let path = path.as_ref().to_path_buf();
        let inner = if path.exists() {
            PageManager::open(&path, &opts)?
        } else if opts.create_if_missing {
            PageManager::create(&path, &opts)?
        } else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("database not found: {}", path.display()),
            )));
        };
        Ok(Self { path, inner })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn page_size(&self) -> usize {
        self.inner.page_size()
    }

    pub fn page_count(&self) -> u64 {
        self.inner.page_count()
    }

    pub fn free_page_count(&self) -> u64 {
        self.inner.free_page_count()
    }

    /// Low-level access to the page manager.
    ///
    /// Intended for tests and upcoming subsystems. Higher-level APIs will
    /// replace direct page access in later phases.
    pub fn page_manager(&mut self) -> &mut PageManager {
        &mut self.inner
    }

    /// Flush pending writes.
    pub fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }

    /// Gracefully close the database.
    pub fn close(mut self) -> Result<()> {
        self.inner.close()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.inner.close();
    }
}
