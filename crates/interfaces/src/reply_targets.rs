use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

const REPLY_TARGET_TTL: Duration = Duration::from_hours(1);

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
        self.targets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn track(&self, correlation_id: &str, target: C) {
        let mut targets = self.lock();
        targets.retain(|_, (_, at)| at.elapsed() < REPLY_TARGET_TTL);
        targets.insert(correlation_id.to_string(), (target, Instant::now()));
    }

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
