use std::sync::{Mutex, MutexGuard, PoisonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    Queued,
    SendNow,
    Withdrawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointTake {
    Deliver,
    Hold,
    Drop,
}

#[derive(Debug, Default)]
pub(crate) struct QueuedMessages {
    entries: Mutex<Vec<(String, Waiting)>>,
}

impl QueuedMessages {
    fn lock(&self) -> MutexGuard<'_, Vec<(String, Waiting)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn enqueue(&self, id: &str) {
        let mut entries = self.lock();
        if !entries.iter().any(|(entry, _)| entry == id) {
            entries.push((id.to_owned(), Waiting::Queued));
        }
    }

    pub(crate) fn take_at_checkpoint(&self, id: &str) -> CheckpointTake {
        let mut entries = self.lock();
        let Some(index) = entries.iter().position(|(entry, _)| entry == id) else {
            return CheckpointTake::Deliver;
        };
        match entries.remove(index) {
            (_, Waiting::Queued) => CheckpointTake::Deliver,
            entry @ (_, Waiting::SendNow) => {
                entries.insert(index, entry);
                CheckpointTake::Hold
            }
            (_, Waiting::Withdrawn) => CheckpointTake::Drop,
        }
    }

    pub(crate) fn keep_after_turn(&self, id: &str) -> bool {
        let mut entries = self.lock();
        let withdrawn = entries
            .iter()
            .position(|(entry, waiting)| entry == id && *waiting == Waiting::Withdrawn);
        match withdrawn {
            Some(index) => {
                entries.remove(index);
                false
            }
            None => true,
        }
    }

    pub(crate) fn claim_turn_start(&self, id: &str) -> bool {
        let mut entries = self.lock();
        let Some(index) = entries.iter().position(|(entry, _)| entry == id) else {
            return true;
        };
        let (_, waiting) = entries.remove(index);
        waiting != Waiting::Withdrawn
    }

    pub(crate) fn is_send_now(&self, id: &str) -> bool {
        self.lock()
            .iter()
            .any(|(entry, waiting)| entry == id && *waiting == Waiting::SendNow)
    }

    pub(crate) fn send_now(&self, id: &str) -> bool {
        let mut entries = self.lock();
        match entries.iter_mut().find(|(entry, _)| entry == id) {
            Some((_, waiting)) if *waiting != Waiting::Withdrawn => {
                *waiting = Waiting::SendNow;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn withdraw(&self, id: &str) -> bool {
        let mut entries = self.lock();
        match entries.iter_mut().find(|(entry, _)| entry == id) {
            Some((_, waiting)) if *waiting != Waiting::Withdrawn => {
                *waiting = Waiting::Withdrawn;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_message_is_delivered_at_the_checkpoint_and_then_forgotten() {
        let queued = QueuedMessages::default();
        queued.enqueue("m1");
        assert_eq!(queued.take_at_checkpoint("m1"), CheckpointTake::Deliver);
        assert!(!queued.withdraw("m1"), "read messages can't be taken back");
        assert!(!queued.send_now("m1"));
    }

    #[test]
    fn an_untracked_message_is_delivered_and_kept() {
        let queued = QueuedMessages::default();
        assert_eq!(queued.take_at_checkpoint("agent"), CheckpointTake::Deliver);
        assert!(queued.keep_after_turn("agent"));
        assert!(queued.claim_turn_start("agent"));
    }

    #[test]
    fn a_withdrawn_message_is_dropped_wherever_it_is_found() {
        let queued = QueuedMessages::default();
        queued.enqueue("a");
        queued.enqueue("b");
        queued.enqueue("c");
        assert!(queued.withdraw("a"));
        assert!(queued.withdraw("b"));
        assert!(queued.withdraw("c"));
        assert!(
            !queued.withdraw("a"),
            "a second withdraw finds nothing waiting"
        );
        assert_eq!(queued.take_at_checkpoint("a"), CheckpointTake::Drop);
        assert!(!queued.keep_after_turn("b"));
        assert!(!queued.claim_turn_start("c"));
    }

    #[test]
    fn a_message_to_send_now_is_held_from_the_turn_and_starts_the_next() {
        let queued = QueuedMessages::default();
        queued.enqueue("m1");
        assert!(queued.send_now("m1"));
        assert!(queued.is_send_now("m1"));
        assert_eq!(queued.take_at_checkpoint("m1"), CheckpointTake::Hold);
        assert!(queued.keep_after_turn("m1"));
        assert!(queued.claim_turn_start("m1"));
        assert!(!queued.is_send_now("m1"));
        assert!(
            !queued.withdraw("m1"),
            "a message that started its turn was read"
        );
    }

    #[test]
    fn a_message_to_send_now_can_still_be_withdrawn() {
        let queued = QueuedMessages::default();
        queued.enqueue("m1");
        assert!(queued.send_now("m1"));
        assert!(queued.withdraw("m1"));
        assert!(!queued.send_now("m1"));
        assert_eq!(queued.take_at_checkpoint("m1"), CheckpointTake::Drop);
    }

    #[test]
    fn a_message_left_waiting_after_the_turn_is_claimed_when_its_turn_starts() {
        let queued = QueuedMessages::default();
        queued.enqueue("late");
        assert!(queued.keep_after_turn("late"));
        assert!(queued.withdraw("late"), "still waiting for its own turn");
        assert!(!queued.claim_turn_start("late"));
    }
}
