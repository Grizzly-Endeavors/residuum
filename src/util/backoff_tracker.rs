//! Tracks consecutive failures of an automatic, repeatedly-triggered
//! operation (e.g. the observer's extract call, the reflector's compress
//! pass) so a caller can back off between retries instead of re-attempting
//! — and re-spending, for an LLM call — on every trigger, and so the user
//! is told once when a failure streak starts and once when it clears,
//! rather than either never or on every single retry.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Starting backoff after the first failure in a streak.
const BASE_BACKOFF: Duration = Duration::from_secs(60);
/// Backoff never grows past this, so a long-broken subsystem still retries
/// (and can notice its own recovery) at a bounded interval.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// Whether an automatic attempt should proceed right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryGate {
    /// No active backoff (or it has elapsed) — go ahead and attempt.
    Proceed,
    /// Still backing off from a prior failure; skip this attempt. The user
    /// was already notified when the streak started, so this is silent.
    Skip,
}

/// What the caller should tell the user, if anything, after recording an
/// outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeAction {
    /// A repeat failure or success within an already-known state — say
    /// nothing new.
    None,
    /// The first failure of a new streak: notify once, in plain language.
    FailureStarted,
    /// The first success after a failing streak: notify that it recovered.
    Recovered,
}

struct State {
    consecutive_failures: u32,
    backoff_until: Option<Instant>,
}

/// Thread-safe failure/backoff tracker. Cheap enough to check on every
/// automatic trigger attempt; never held across an `await`.
pub struct BackoffTracker {
    state: Mutex<State>,
}

impl Default for BackoffTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BackoffTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                consecutive_failures: 0,
                backoff_until: None,
            }),
        }
    }

    /// Whether an automatic attempt should proceed right now, or be skipped
    /// because a prior failure's backoff hasn't elapsed yet.
    #[must_use]
    pub fn gate(&self) -> RetryGate {
        let state = self.lock();
        match state.backoff_until {
            Some(until) if Instant::now() < until => RetryGate::Skip,
            _ => RetryGate::Proceed,
        }
    }

    /// Record a failed attempt, advancing the backoff. Returns
    /// [`NoticeAction::FailureStarted`] exactly when this is the first
    /// failure of a new streak.
    pub fn record_failure(&self) -> NoticeAction {
        let mut state = self.lock();
        let is_new_streak = state.consecutive_failures == 0;
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        state.backoff_until = Some(Instant::now() + backoff_for(state.consecutive_failures));
        if is_new_streak {
            NoticeAction::FailureStarted
        } else {
            NoticeAction::None
        }
    }

    /// Record a successful attempt, clearing any backoff. Returns
    /// [`NoticeAction::Recovered`] exactly when this follows a failing
    /// streak.
    pub fn record_success(&self) -> NoticeAction {
        let mut state = self.lock();
        let was_failing = state.consecutive_failures > 0;
        state.consecutive_failures = 0;
        state.backoff_until = None;
        if was_failing {
            NoticeAction::Recovered
        } else {
            NoticeAction::None
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Exponential backoff, doubling per consecutive failure and capped at
/// [`MAX_BACKOFF`]: 1m, 2m, 4m, 8m, ... up to 1h.
fn backoff_for(consecutive_failures: u32) -> Duration {
    // Cap the exponent itself (not just the result) so the shift below
    // never overflows u64 on a very long failure streak.
    let exponent = consecutive_failures.saturating_sub(1).min(10);
    let secs = BASE_BACKOFF.as_secs().saturating_mul(1_u64 << exponent);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_open_with_no_backoff() {
        let tracker = BackoffTracker::new();
        assert_eq!(tracker.gate(), RetryGate::Proceed);
    }

    #[test]
    fn first_failure_notifies_and_starts_backoff() {
        let tracker = BackoffTracker::new();
        assert_eq!(tracker.record_failure(), NoticeAction::FailureStarted);
        assert_eq!(
            tracker.gate(),
            RetryGate::Skip,
            "an attempt right after a failure must be backed off"
        );
    }

    #[test]
    fn repeat_failures_in_the_same_streak_do_not_renotify() {
        let tracker = BackoffTracker::new();
        assert_eq!(tracker.record_failure(), NoticeAction::FailureStarted);
        assert_eq!(tracker.record_failure(), NoticeAction::None);
        assert_eq!(tracker.record_failure(), NoticeAction::None);
    }

    #[test]
    fn success_after_failures_notifies_recovery_and_clears_backoff() {
        let tracker = BackoffTracker::new();
        tracker.record_failure();
        assert_eq!(tracker.record_success(), NoticeAction::Recovered);
        assert_eq!(
            tracker.gate(),
            RetryGate::Proceed,
            "backoff must clear on recovery"
        );
    }

    #[test]
    fn success_with_no_prior_failures_does_not_notify() {
        let tracker = BackoffTracker::new();
        assert_eq!(tracker.record_success(), NoticeAction::None);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_for(1), Duration::from_secs(60));
        assert_eq!(backoff_for(2), Duration::from_secs(120));
        assert_eq!(backoff_for(3), Duration::from_secs(240));
        assert_eq!(
            backoff_for(20),
            MAX_BACKOFF,
            "must cap rather than overflow"
        );
    }

    #[test]
    fn a_later_failure_extends_backoff_past_the_first() {
        let tracker = BackoffTracker::new();
        tracker.record_failure();
        let first_deadline = tracker.lock().backoff_until;
        std::thread::sleep(Duration::from_millis(5));
        tracker.record_failure();
        let second_deadline = tracker.lock().backoff_until;
        assert!(
            second_deadline > first_deadline,
            "second consecutive failure should push the deadline further out"
        );
    }
}
