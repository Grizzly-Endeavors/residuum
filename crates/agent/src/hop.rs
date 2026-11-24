use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct HopLimits {
    pub soft: u32,
    pub hard: u32,
}

impl From<&residuum_config::BackgroundConfig> for HopLimits {
    fn from(cfg: &residuum_config::BackgroundConfig) -> Self {
        Self {
            soft: cfg.hop_soft_limit,
            hard: cfg.hop_hard_limit,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HopCounter(Arc<AtomicU32>);

impl HopCounter {
    #[must_use]
    pub fn new(initial: u32) -> Self {
        Self(Arc::new(AtomicU32::new(initial)))
    }

    #[must_use]
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set(&self, value: u32) {
        self.0.store(value, Ordering::Relaxed);
    }

    pub fn bump(&self, candidate: u32) {
        self.0.fetch_max(candidate, Ordering::Relaxed);
    }

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
