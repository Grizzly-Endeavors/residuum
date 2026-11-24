use std::sync::Mutex;
use std::time::{Duration, Instant};

const BASE_BACKOFF: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_hours(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryGate {
    Proceed,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeAction {
    None,
    FailureStarted,
    Recovered,
}

struct State {
    consecutive_failures: u32,
    backoff_until: Option<Instant>,
}

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

    #[must_use]
    pub fn gate(&self) -> RetryGate {
        let state = self.lock();
        match state.backoff_until {
            Some(until) if Instant::now() < until => RetryGate::Skip,
            _ => RetryGate::Proceed,
        }
    }

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

    pub fn clear_backoff_wait(&self) {
        self.lock().backoff_until = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn backoff_for(consecutive_failures: u32) -> Duration {
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
        tracker.record_failure();
        let second_deadline = tracker.lock().backoff_until;
        assert!(
            second_deadline > first_deadline,
            "second consecutive failure should push the deadline further out"
        );
    }

    #[test]
    fn clear_backoff_wait_allows_immediate_retry_and_preserves_recovery_notice() {
        let tracker = BackoffTracker::new();
        assert_eq!(tracker.record_failure(), NoticeAction::FailureStarted);
        assert_eq!(tracker.gate(), RetryGate::Skip);

        tracker.clear_backoff_wait();
        assert_eq!(
            tracker.gate(),
            RetryGate::Proceed,
            "clearing backoff wait should allow the next attempt right away"
        );
        assert_eq!(
            tracker.record_success(),
            NoticeAction::Recovered,
            "preserving the failure streak should still emit Recovered on success"
        );
    }
}
