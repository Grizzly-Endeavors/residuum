//! People's messages waiting for the main agent to read them.
//!
//! A message that reaches the main agent while a turn runs waits on the
//! turn's interrupt channel until the turn's next checkpoint, or, when it
//! arrives after the last one, in `deferred_inbound` until it starts a turn
//! of its own. This list tracks those messages by id so the web UI can show
//! them as waiting and act on one before the agent reads it: withdraw it
//! (to send it somewhere else instead), or have it sent now (the running
//! turn stops, and the message starts the next one).
//!
//! The list only says what may still happen to a message; the message itself
//! travels the channel or the deferred queue as before. Whoever takes a
//! message out of either asks the list first (see [`QueuedMessages::take_at_checkpoint`],
//! [`QueuedMessages::keep_after_turn`] and [`QueuedMessages::claim_turn_start`]),
//! so a withdrawn message is dropped there and a message to send now is held
//! back from the turn it would otherwise be folded into.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// What may still happen to a waiting message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    /// It goes to the agent at the next chance.
    Queued,
    /// It starts the next turn: the running one was asked to stop, and the
    /// message is not folded into it.
    SendNow,
    /// The person took it back; whoever finds it drops it.
    Withdrawn,
}

/// What a turn checkpoint does with a message it drained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointTake {
    /// Fold it into the turn.
    Deliver,
    /// Keep it out of this turn; it starts the next one.
    Hold,
    /// It was withdrawn: drop it.
    Drop,
}

/// The main agent's waiting messages, by id, in the order they arrived.
#[derive(Debug, Default)]
pub(crate) struct QueuedMessages {
    entries: Mutex<Vec<(String, Waiting)>>,
}

impl QueuedMessages {
    fn lock(&self) -> MutexGuard<'_, Vec<(String, Waiting)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Track a message that is about to wait for the agent.
    pub(crate) fn enqueue(&self, id: &str) {
        let mut entries = self.lock();
        if !entries.iter().any(|(entry, _)| entry == id) {
            entries.push((id.to_owned(), Waiting::Queued));
        }
    }

    /// A turn checkpoint drained message `id`. An untracked id (an agent's
    /// message, or one no person can act on) is delivered.
    pub(crate) fn take_at_checkpoint(&self, id: &str) -> CheckpointTake {
        let mut entries = self.lock();
        let Some(index) = entries.iter().position(|(entry, _)| entry == id) else {
            return CheckpointTake::Deliver;
        };
        match entries.remove(index) {
            (_, Waiting::Queued) => CheckpointTake::Deliver,
            // Still waiting: it starts the next turn, and is claimed there.
            entry @ (_, Waiting::SendNow) => {
                entries.insert(index, entry);
                CheckpointTake::Hold
            }
            (_, Waiting::Withdrawn) => CheckpointTake::Drop,
        }
    }

    /// The turn ended with message `id` still waiting: whether it goes on to
    /// start a turn of its own. A withdrawn one is dropped here.
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

    /// Message `id` is about to start a turn: it stops waiting. Returns
    /// `false` when it was withdrawn, and no turn should run for it.
    pub(crate) fn claim_turn_start(&self, id: &str) -> bool {
        let mut entries = self.lock();
        let Some(index) = entries.iter().position(|(entry, _)| entry == id) else {
            return true;
        };
        let (_, waiting) = entries.remove(index);
        waiting != Waiting::Withdrawn
    }

    /// Whether message `id` was asked to be sent now, so it runs ahead of
    /// anything else waiting for a turn.
    pub(crate) fn is_send_now(&self, id: &str) -> bool {
        self.lock()
            .iter()
            .any(|(entry, waiting)| entry == id && *waiting == Waiting::SendNow)
    }

    /// Ask for message `id` to be sent now. Returns `false` when it is no
    /// longer waiting (the agent already read it, or it was withdrawn).
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

    /// Take message `id` back before the agent reads it. Returns `false` when
    /// it is no longer waiting (the agent already read it, or it was
    /// withdrawn already).
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
