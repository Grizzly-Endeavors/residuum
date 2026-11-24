use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::time::Duration;

use tokio::time::Instant;

pub(crate) const REFILL_PER_MINUTE: f64 = 300.0;
pub(crate) const BURST: f64 = 60.0;
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PeerKey {
    Ip(IpAddr),
    Other(String),
}

impl PeerKey {
    fn from_peer(peer_ip: &str) -> Self {
        match peer_ip.trim().parse::<IpAddr>() {
            Ok(ip) => match ip.to_canonical() {
                IpAddr::V6(v6) => {
                    let bits = u128::from(v6) & !u128::from(u64::MAX);
                    Self::Ip(IpAddr::V6(Ipv6Addr::from(bits)))
                }
                v4 @ IpAddr::V4(_) => Self::Ip(v4),
            },
            Err(_) => Self::Other(peer_ip.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Debug)]
pub(crate) struct PeerRateLimiter {
    buckets: HashMap<PeerKey, Bucket>,
    per_second: f64,
    burst: f64,
    last_sweep: Instant,
}

impl PeerRateLimiter {
    pub(crate) fn new(now: Instant) -> Self {
        Self::with_rate(REFILL_PER_MINUTE, BURST, now)
    }

    pub(crate) fn with_rate(per_minute: f64, burst: f64, now: Instant) -> Self {
        Self {
            buckets: HashMap::new(),
            per_second: per_minute / 60.0,
            burst,
            last_sweep: now,
        }
    }

    pub(crate) fn check(&mut self, peer_ip: &str, now: Instant) -> Result<(), Duration> {
        self.sweep(now);
        let key = PeerKey::from_peer(peer_ip);
        let burst = self.burst;
        let per_second = self.per_second;
        let bucket = self.buckets.entry(key).or_insert(Bucket {
            tokens: burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * per_second).min(burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - bucket.tokens) / per_second))
        }
    }

    #[cfg(test)]
    pub(crate) fn tracked_peers(&self) -> usize {
        self.buckets.len()
    }

    fn sweep(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last_sweep) < SWEEP_INTERVAL {
            return;
        }
        self.last_sweep = now;
        let burst = self.burst;
        let per_second = self.per_second;
        self.buckets.retain(|_, bucket| {
            let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
            bucket.tokens + elapsed * per_second < burst
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_limited_then_refills() {
        let t0 = Instant::now();
        let mut limiter = PeerRateLimiter::new(t0);
        for _ in 0..60 {
            assert!(limiter.check("203.0.113.9", t0).is_ok());
        }
        let wait = limiter.check("203.0.113.9", t0).unwrap_err();
        assert!(wait <= Duration::from_millis(201) && wait > Duration::ZERO);
        assert!(
            limiter
                .check("203.0.113.9", t0 + Duration::from_millis(250))
                .is_ok()
        );
        assert!(
            limiter
                .check("203.0.113.9", t0 + Duration::from_millis(250))
                .is_err()
        );
    }

    #[test]
    fn peers_are_independent() {
        let t0 = Instant::now();
        let mut limiter = PeerRateLimiter::new(t0);
        for _ in 0..60 {
            limiter.check("203.0.113.9", t0).unwrap();
        }
        assert!(limiter.check("203.0.113.9", t0).is_err());
        assert!(limiter.check("203.0.113.10", t0).is_ok());
    }

    #[test]
    fn ipv6_shares_a_bucket_per_64() {
        let t0 = Instant::now();
        let mut limiter = PeerRateLimiter::new(t0);
        for i in 0..60 {
            limiter.check(&format!("2001:db8:1:2::{i:x}"), t0).unwrap();
        }
        assert!(limiter.check("2001:db8:1:2:ffff::1", t0).is_err());
        assert!(limiter.check("2001:db8:1:3::1", t0).is_ok());
    }

    #[test]
    fn mapped_ipv4_matches_plain_ipv4() {
        let t0 = Instant::now();
        let mut limiter = PeerRateLimiter::new(t0);
        for _ in 0..60 {
            limiter.check("198.51.100.4", t0).unwrap();
        }
        assert!(limiter.check("::ffff:198.51.100.4", t0).is_err());
    }

    #[test]
    fn idle_buckets_are_evicted() {
        let t0 = Instant::now();
        let mut limiter = PeerRateLimiter::new(t0);
        for i in 0..100 {
            limiter.check(&format!("10.0.0.{i}"), t0).unwrap();
        }
        assert_eq!(limiter.tracked_peers(), 100);
        let later = t0 + Duration::from_secs(120);
        limiter.check("192.0.2.1", later).unwrap();
        assert_eq!(limiter.tracked_peers(), 1);
    }

    #[test]
    fn active_limited_bucket_survives_sweep() {
        let t0 = Instant::now();
        let mut limiter = PeerRateLimiter::new(t0);
        for _ in 0..60 {
            limiter.check("203.0.113.9", t0).unwrap();
        }
        let later = t0 + Duration::from_secs(61);
        limiter.check("203.0.113.9", later).unwrap();
        assert_eq!(limiter.tracked_peers(), 1);
    }
}
