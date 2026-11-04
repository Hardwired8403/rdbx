//! In-memory buffer pool for RDBX.
//!
//! The pool caches decoded [`rdbx_storage::Page`] values in fixed-size frames
//! and keeps at most `capacity` of them resident. Callers `fetch` a page to
//! obtain a [`FrameId`] (which pins the frame), mutate it via `get_mut`, and
//! `unpin` when done. Pinned frames are never evicted.

mod frame;
mod policy;
mod pool;

pub use frame::{Frame, FrameId};
pub use policy::{ClockPolicy, EvictionPolicy, LruPolicy};
pub use pool::{BufferPool, BufferStats};
