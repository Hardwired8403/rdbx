//! The buffer pool itself.

use std::collections::HashMap;

use rdbx_core::{Error, EvictionPolicyKind, PageId, Result};
use rdbx_storage::{Page, PageManager, PageType};

use crate::frame::{Frame, FrameId};
use crate::policy::{EvictionPolicy, make_policy};

/// Aggregate counters exposed by the buffer pool.
#[derive(Debug, Default, Clone, Copy)]
pub struct BufferStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub dirty_flushes: u64,
    pub allocations: u64,
}

impl BufferStats {
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Fixed-capacity cache of decoded pages with LRU or Clock eviction.
pub struct BufferPool {
    frames: Vec<Frame>,
    by_page: HashMap<PageId, FrameId>,
    free_frames: Vec<FrameId>,
    capacity: usize,
    policy: Box<dyn EvictionPolicy>,
    stats: BufferStats,
}

impl BufferPool {
    /// Create a buffer pool with `capacity` frames. All frames are allocated
    /// up front so memory usage is bounded and predictable.
    pub fn new(capacity: usize, page_size: usize, policy: EvictionPolicyKind) -> Self {
        assert!(capacity > 0, "buffer pool capacity must be > 0");
        let mut frames = Vec::with_capacity(capacity);
        let mut free = Vec::with_capacity(capacity);
        for i in 0..capacity {
            frames.push(Frame::empty(page_size));
            free.push(FrameId(i as u32));
        }
        Self {
            frames,
            by_page: HashMap::with_capacity(capacity),
            free_frames: free,
            capacity,
            policy: make_policy(policy, capacity),
            stats: BufferStats::default(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn stats(&self) -> BufferStats {
        self.stats
    }
    pub fn policy_name(&self) -> &'static str {
        self.policy.name()
    }
    pub fn resident_page_count(&self) -> usize {
        self.by_page.len()
    }
    pub fn pinned_count(&self) -> usize {
        self.frames.iter().filter(|f| f.pin_count > 0).count()
    }

    /// Inspection helper: whether `pid` is currently resident in some frame.
    pub fn by_page_contains(&self, pid: PageId) -> bool {
        self.by_page.contains_key(&pid)
    }

    /// Fetch a page. On hit, bumps the pin count and returns the existing
    /// frame. On miss, reads from disk and returns a newly pinned frame.
    pub fn fetch(&mut self, mgr: &mut PageManager, pid: PageId) -> Result<FrameId> {
        if let Some(&fid) = self.by_page.get(&pid) {
            self.frames[fid.as_usize()].pin_count += 1;
            self.policy.on_access(&mut self.frames, fid);
            self.stats.hits += 1;
            return Ok(fid);
        }
        self.stats.misses += 1;
        let fid = self.acquire_frame(mgr)?;
        let page = mgr.read_page(pid)?;
        let frame = &mut self.frames[fid.as_usize()];
        frame.page_id = Some(pid);
        frame.data = page;
        frame.pin_count = 1;
        frame.dirty = false;
        self.by_page.insert(pid, fid);
        self.policy.on_insert(&mut self.frames, fid);
        Ok(fid)
    }

    /// Allocate a fresh page in `mgr` and insert it into the pool as pinned
    /// and dirty. The frame must be unpinned by the caller.
    pub fn new_page(&mut self, mgr: &mut PageManager, page_type: PageType) -> Result<FrameId> {
        let pid = mgr.allocate_page(page_type)?;
        let fid = self.acquire_frame(mgr)?;
        let page = Page::new(pid, page_type, mgr.page_size());
        let frame = &mut self.frames[fid.as_usize()];
        frame.page_id = Some(pid);
        frame.data = page;
        frame.pin_count = 1;
        frame.dirty = true;
        self.by_page.insert(pid, fid);
        self.policy.on_insert(&mut self.frames, fid);
        self.stats.allocations += 1;
        Ok(fid)
    }

    /// Free a page. Removes any cached copy and delegates to the manager.
    pub fn free_page(&mut self, mgr: &mut PageManager, pid: PageId) -> Result<()> {
        if let Some(&fid) = self.by_page.get(&pid) {
            if self.frames[fid.as_usize()].pin_count > 0 {
                return Err(Error::Internal(format!("cannot free pinned page {pid}")));
            }
            self.policy.on_remove(&mut self.frames, fid);
            self.by_page.remove(&pid);
            self.frames[fid.as_usize()].page_id = None;
            self.frames[fid.as_usize()].dirty = false;
            self.free_frames.push(fid);
        }
        mgr.free_page(pid)?;
        Ok(())
    }

    /// Borrow a pinned frame's page for read-only access.
    pub fn get(&self, fid: FrameId) -> Result<&Page> {
        let frame = self.frame(fid)?;
        if frame.pin_count == 0 {
            return Err(Error::Internal(format!("frame {fid:?} is not pinned")));
        }
        Ok(&frame.data)
    }

    /// Borrow a pinned frame's page mutably. Marks the frame dirty.
    pub fn get_mut(&mut self, fid: FrameId) -> Result<&mut Page> {
        let frame = self.frame_mut(fid)?;
        if frame.pin_count == 0 {
            return Err(Error::Internal(format!("frame {fid:?} is not pinned")));
        }
        frame.dirty = true;
        Ok(&mut frame.data)
    }

    /// Decrement the pin count. Errors on over-unpin.
    pub fn unpin(&mut self, fid: FrameId) -> Result<()> {
        let frame = self.frame_mut(fid)?;
        if frame.pin_count == 0 {
            return Err(Error::Internal(format!(
                "unpin on already-unpinned frame {fid:?}"
            )));
        }
        frame.pin_count -= 1;
        Ok(())
    }

    /// Write every dirty frame back to disk. Returns the number of pages
    /// actually written.
    pub fn flush_all(&mut self, mgr: &mut PageManager) -> Result<usize> {
        let mut written = 0;
        for frame in self.frames.iter_mut() {
            if frame.dirty {
                if frame.page_id.is_some() {
                    mgr.write_page(&frame.data)?;
                    written += 1;
                }
                frame.dirty = false;
            }
        }
        Ok(written)
    }

    /// Write a single frame to disk if dirty.
    pub fn flush_frame(&mut self, mgr: &mut PageManager, fid: FrameId) -> Result<()> {
        let frame = self.frame_mut(fid)?;
        if frame.dirty {
            mgr.write_page(&frame.data)?;
            frame.dirty = false;
        }
        Ok(())
    }

    // --- internals ---

    fn frame(&self, fid: FrameId) -> Result<&Frame> {
        self.frames
            .get(fid.as_usize())
            .ok_or_else(|| Error::Internal(format!("frame {fid:?} out of range")))
    }

    fn frame_mut(&mut self, fid: FrameId) -> Result<&mut Frame> {
        self.frames
            .get_mut(fid.as_usize())
            .ok_or_else(|| Error::Internal(format!("frame {fid:?} out of range")))
    }

    fn acquire_frame(&mut self, mgr: &mut PageManager) -> Result<FrameId> {
        if let Some(fid) = self.free_frames.pop() {
            return Ok(fid);
        }
        self.evict_one(mgr)
    }

    fn evict_one(&mut self, mgr: &mut PageManager) -> Result<FrameId> {
        let victim = self
            .policy
            .select_victim(&mut self.frames)
            .ok_or_else(|| Error::Internal("all buffer frames are pinned".into()))?;
        self.stats.evictions += 1;
        let i = victim.as_usize();
        if self.frames[i].dirty {
            mgr.write_page(&self.frames[i].data)?;
            self.frames[i].dirty = false;
            self.stats.dirty_flushes += 1;
        }
        if let Some(pid) = self.frames[i].page_id.take() {
            self.by_page.remove(&pid);
        }
        self.policy.on_remove(&mut self.frames, victim);
        Ok(victim)
    }
}
