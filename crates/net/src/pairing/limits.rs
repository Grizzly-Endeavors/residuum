//! Sliding-window limits on the pairing routes a stranger can reach.
//!
//! Creating a pairing request, entering a recovery code and presenting a
//! token are each capped per peer address and, so many addresses can't add up
//! to a flood, across the whole install. Only attempts that are allowed count
//! against the window, so a refused flood adds no memory.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

/// How many attempts one peer address may make in a window.
pub(super) const PER_PEER_LIMIT: usize = 10;

/// How many attempts the whole install allows in a window.
pub(super) const PER_INSTANCE_LIMIT: usize = 60;

const WINDOW_SECS: i64 = 60;

/// Key for attempts that arrive with no usable peer address.
const UNKNOWN_PEER: &str = "unknown";

#[derive(Debug, Default)]
pub(super) struct RateLimiter {
    peers: HashMap<String, VecDeque<DateTime<Utc>>>,
    all: VecDeque<DateTime<Utc>>,
}

/// Which limit refused an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LimitHit {
    Peer,
    Instance,
}

impl RateLimiter {
    /// Count an attempt by `peer` at `now`, or say which limit it hit.
    pub(super) fn check(&mut self, peer: Option<&str>, now: DateTime<Utc>) -> Result<(), LimitHit> {
        let cutoff = now - Duration::seconds(WINDOW_SECS);
        prune(&mut self.all, cutoff);
        self.peers.retain(|_, hits| {
            prune(hits, cutoff);
            !hits.is_empty()
        });

        if self.all.len() >= PER_INSTANCE_LIMIT {
            return Err(LimitHit::Instance);
        }
        let hits = self
            .peers
            .entry(peer.unwrap_or(UNKNOWN_PEER).to_string())
            .or_default();
        if hits.len() >= PER_PEER_LIMIT {
            return Err(LimitHit::Peer);
        }
        hits.push_back(now);
        self.all.push_back(now);
        Ok(())
    }
}

fn prune(hits: &mut VecDeque<DateTime<Utc>>, cutoff: DateTime<Utc>) {
    while hits.front().is_some_and(|at| *at <= cutoff) {
        hits.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_is_limited_after_ten_attempts_in_a_minute() {
        let mut limiter = RateLimiter::default();
        let now = Utc::now();
        for _ in 0..PER_PEER_LIMIT {
            assert_eq!(limiter.check(Some("1.2.3.4"), now), Ok(()));
        }
        assert_eq!(
            limiter.check(Some("1.2.3.4"), now),
            Err(LimitHit::Peer),
            "the eleventh attempt in a minute must be refused"
        );
        assert_eq!(
            limiter.check(Some("5.6.7.8"), now),
            Ok(()),
            "another peer is unaffected"
        );
    }

    #[test]
    fn the_window_slides() {
        let mut limiter = RateLimiter::default();
        let now = Utc::now();
        for _ in 0..PER_PEER_LIMIT {
            limiter.check(Some("a"), now).unwrap();
        }
        let later = now + Duration::seconds(WINDOW_SECS + 1);
        assert_eq!(limiter.check(Some("a"), later), Ok(()));
    }

    #[test]
    fn the_whole_install_is_limited_to_sixty_a_minute() {
        let mut limiter = RateLimiter::default();
        let now = Utc::now();
        for i in 0..PER_INSTANCE_LIMIT {
            let peer = format!("peer-{i}");
            assert_eq!(limiter.check(Some(&peer), now), Ok(()), "attempt {i}");
        }
        assert_eq!(
            limiter.check(Some("one-more"), now),
            Err(LimitHit::Instance)
        );
    }

    #[test]
    fn attempts_without_an_address_share_one_bucket() {
        let mut limiter = RateLimiter::default();
        let now = Utc::now();
        for _ in 0..PER_PEER_LIMIT {
            limiter.check(None, now).unwrap();
        }
        assert_eq!(limiter.check(None, now), Err(LimitHit::Peer));
    }
}
