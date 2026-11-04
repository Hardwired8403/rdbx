//! Eviction policies (LRU and Clock).

use std::collections::VecDeque;

use rdbx_core::EvictionPolicyKind;

use crate::frame::{Frame, FrameId};

/// Buffer pool eviction strategy.
///
/// All methods receive the frame array so policies that need per-frame state
/// (Clock) can inspect or reset it. Policies own any bookkeeping they require.
pub trait EvictionPolicy: Send {
    fn name(&self) -> &'static str;
    fn on_insert(&mut self, frames: &mut [Frame], fid: FrameId);
    fn on_access(&mut self, frames: &mut [Frame], fid: FrameId);
    fn on_remove(&mut self, frames: &mut [Frame], fid: FrameId);
    /// Pick a victim among frames with `pin_count == 0`. Returns `None` if
    /// every resident frame is pinned.
    fn select_victim(&mut self, frames: &mut [Frame]) -> Option<FrameId>;
}

pub(crate) fn make_policy(kind: EvictionPolicyKind, capacity: usize) -> Box<dyn EvictionPolicy> {
    match kind {
        EvictionPolicyKind::Lru => Box::new(LruPolicy::new()),
        EvictionPolicyKind::Clock => Box::new(ClockPolicy::new(capacity)),
    }
}

// ---------------------------------------------------------------- LRU

/// Least Recently Used. O(n) per touch; adequate for Phase 2 capacities.
pub struct LruPolicy {
    order: VecDeque<FrameId>,
}

impl LruPolicy {
    pub fn new() -> Self {
        Self {
            order: VecDeque::new(),
        }
    }
}

impl Default for LruPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl EvictionPolicy for LruPolicy {
    fn name(&self) -> &'static str {
        "lru"
    }

    fn on_insert(&mut self, _frames: &mut [Frame], fid: FrameId) {
        self.order.push_front(fid);
    }

    fn on_access(&mut self, _frames: &mut [Frame], fid: FrameId) {
        if let Some(pos) = self.order.iter().position(|&x| x == fid) {
            self.order.remove(pos);
        }
        self.order.push_front(fid);
    }

    fn on_remove(&mut self, _frames: &mut [Frame], fid: FrameId) {
        if let Some(pos) = self.order.iter().position(|&x| x == fid) {
            self.order.remove(pos);
        }
    }

    fn select_victim(&mut self, frames: &mut [Frame]) -> Option<FrameId> {
        // Back of queue is least recently used.
        self.order
            .iter()
            .rev()
            .find(|&&fid| frames[fid.as_usize()].pin_count == 0)
            .copied()
    }
}

// -------------------------------------------------------------- Clock

/// Clock (second-chance). O(1) amortised.
pub struct ClockPolicy {
    hand: usize,
    ref_bits: Vec<bool>,
}

impl ClockPolicy {
    pub fn new(capacity: usize) -> Self {
        Self {
            hand: 0,
            ref_bits: vec![false; capacity],
        }
    }
}

impl EvictionPolicy for ClockPolicy {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn on_insert(&mut self, _frames: &mut [Frame], fid: FrameId) {
        self.ref_bits[fid.as_usize()] = true;
    }

    fn on_access(&mut self, _frames: &mut [Frame], fid: FrameId) {
        self.ref_bits[fid.as_usize()] = true;
    }

    fn on_remove(&mut self, _frames: &mut [Frame], fid: FrameId) {
        self.ref_bits[fid.as_usize()] = false;
    }

    fn select_victim(&mut self, frames: &mut [Frame]) -> Option<FrameId> {
        let n = frames.len();
        // At most 2 full sweeps are enough to guarantee a victim or exhaustion.
        for _ in 0..(2 * n) {
            let i = self.hand;
            let f = &frames[i];
            if f.page_id.is_some() && f.pin_count == 0 {
                if self.ref_bits[i] {
                    self.ref_bits[i] = false;
                } else {
                    self.hand = (i + 1) % n;
                    return Some(FrameId(i as u32));
                }
            }
            self.hand = (i + 1) % n;
        }
        None
    }
}
