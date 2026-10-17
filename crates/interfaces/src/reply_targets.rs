//! Which conversation each in-flight agent turn should answer in.
//!
//! Chat interfaces can receive messages from several conversations; the bus
//! only tells them the correlation ID of the turn that produced output. Each
//! interface records the source conversation when it hands a message to the
//! agent and looks it up again when output for that turn arrives.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Targets for turns that never report an end (e.g. a message folded into
/// another turn mid-flight) are dropped after this long.
const REPLY_TARGET_TTL: Duration = Duration::from_hours(1);

/// Conversation targets keyed by correlation ID.
pub(crate) struct ReplyTargets<C> {
    targets: Mutex<HashMap<String, (C, Instant)>>,
}

impl<C> Default for ReplyTargets<C> {
    fn default() -> Self {
        Self {
            targets: Mutex::new(HashMap::new()),
        }
    }
}

impl<C: Clone> ReplyTargets<C> {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, (C, Instant)>> {
        // Plain map of owned values; a panic mid-insert cannot corrupt it.
        self.targets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Remember where the turn for `correlation_id` should reply.
    pub(crate) fn track(&self, correlation_id: &str, target: C) {
        let mut targets = self.lock();
        targets.retain(|_, (_, at)| at.elapsed() < REPLY_TARGET_TTL);
        targets.insert(correlation_id.to_string(), (target, Instant::now()));
    }

    /// Where the turn for `correlation_id` replies, if it came from a tracked message.
    pub(crate) fn get(&self, correlation_id: &str) -> Option<C> {
        self.lock()
            .get(correlation_id)
            .map(|(target, _)| target.clone())
    }

    pub(crate) fn release(&self, correlation_id: &str) {
        self.lock().remove(correlation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_target_is_returned_until_released() {
        let targets = ReplyTargets::default();
        targets.track("m1", "chat-a".to_string());
        assert_eq!(targets.get("m1").as_deref(), Some("chat-a"));
        assert_eq!(targets.get("m2"), None);
        targets.release("m1");
        assert_eq!(targets.get("m1"), None);
    }
}
