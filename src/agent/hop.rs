//! Per-turn hop count tracking.
//!
//! A session's (or the main agent's) "current hop count" is the highest hop
//! count among the inputs driving its current turn: the turn's kickoff input,
//! plus any agent-message interrupts drained during it. [`HopCounter`] is the
//! shared, mutable cell that the turn-driving code (the session runtime, or
//! the gateway event loop for main) updates as a turn progresses, and that
//! `message_agent`/`subagent_spawn` read from to compute the hop count an
//! outgoing message or spawn carries.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The soft and hard hop-count limits agent message delivery is checked
/// against, resolved once from `[background]` config.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HopLimits {
    /// At or above this hop count, a delivered message carries a note asking
    /// the receiver to reply only if a reply is actually needed.
    pub soft: u32,
    /// At or above this hop count, delivery is refused outright.
    pub hard: u32,
}

impl From<&crate::config::BackgroundConfig> for HopLimits {
    fn from(cfg: &crate::config::BackgroundConfig) -> Self {
        Self {
            soft: cfg.hop_soft_limit,
            hard: cfg.hop_hard_limit,
        }
    }
}

/// Shared, cheaply-cloneable cell holding the current turn's hop count.
#[derive(Debug, Clone)]
pub struct HopCounter(Arc<AtomicU32>);

impl HopCounter {
    /// Create a counter starting at `initial` — the hop count of the input
    /// that kicked off the first turn this counter will track.
    #[must_use]
    pub fn new(initial: u32) -> Self {
        Self(Arc::new(AtomicU32::new(initial)))
    }

    /// The current turn's hop count.
    #[must_use]
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Overwrite the current turn's hop count — used at the start of a new
    /// turn, to reset to that turn's kickoff input's hop count.
    pub fn set(&self, value: u32) {
        self.0.store(value, Ordering::Relaxed);
    }

    /// Raise the current turn's hop count to `candidate` if it is higher —
    /// used when an agent-message interrupt is drained mid-turn, since it
    /// becomes one more input driving the turn alongside the kickoff.
    pub fn bump(&self, candidate: u32) {
        self.0.fetch_max(candidate, Ordering::Relaxed);
    }

    /// The hop count an outgoing message or spawn from this turn should
    /// carry: one more than the highest hop count among this turn's inputs.
    #[must_use]
    pub fn outgoing(&self) -> u32 {
        self.get() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_at_the_given_value() {
        assert_eq!(HopCounter::new(3).get(), 3);
    }

    #[test]
    fn set_overwrites_regardless_of_direction() {
        let counter = HopCounter::new(5);
        counter.set(1);
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn bump_only_raises() {
        let counter = HopCounter::new(3);
        counter.bump(1);
        assert_eq!(counter.get(), 3, "bump must never lower the count");
        counter.bump(7);
        assert_eq!(counter.get(), 7);
    }

    #[test]
    fn outgoing_is_one_more_than_current() {
        let counter = HopCounter::new(4);
        assert_eq!(counter.outgoing(), 5);
    }

    #[test]
    fn clones_share_the_same_underlying_cell() {
        let counter = HopCounter::new(0);
        let clone = counter.clone();
        clone.set(9);
        assert_eq!(counter.get(), 9, "clones must observe each other's updates");
    }
}
