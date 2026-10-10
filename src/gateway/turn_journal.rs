//! The main conversation's turn in progress, kept so a connection that opens
//! partway through a turn can be shown everything the turn has done so far.
//!
//! History records a turn only once it ends, and the conversation's frames
//! go out live only, so without this a page loaded mid-turn shows nothing of
//! the turn until it ends. The journal follows the `MainConversation` topic,
//! keeps the events of the turn in flight, and relays every event on to the
//! connections that follow it, numbered. A connection asks for a
//! [`TurnSnapshot`] and drops the relayed events it already covers, so it sees
//! each event once and in order.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::bus::{BusError, BusHandle, MainConversationEvent, topics};

/// An event of the main conversation, numbered in the order the journal saw it.
#[derive(Debug)]
pub struct JournaledEvent {
    /// Position in the journal's sequence, counting from one.
    pub seq: u64,
    pub event: MainConversationEvent,
}

/// One recorded event of the turn in flight and when it happened.
#[derive(Debug, Clone)]
pub struct TurnEntry {
    /// When the event happened, or for merged stream pieces, when the first did.
    pub at: DateTime<Utc>,
    pub event: MainConversationEvent,
}

/// The turn in flight as recorded so far.
#[derive(Debug, Clone)]
pub struct TurnRecord {
    pub turn_id: String,
    /// When the journal saw the turn's first event.
    pub started_at: DateTime<Utc>,
    /// The turn's events in order, consecutive stream pieces of one model call merged.
    pub entries: Vec<TurnEntry>,
}

/// What a connection needs to show the turn in flight.
#[derive(Debug)]
pub struct TurnSnapshot {
    /// The last event the snapshot covers: relayed events up to here are in it.
    pub through: u64,
    /// The turn in flight, or `None` when no turn is running.
    pub turn: Option<TurnRecord>,
}

#[derive(Default)]
struct JournalState {
    seq: u64,
    turn: Option<TurnRecord>,
    followers: Vec<mpsc::UnboundedSender<JournaledEvent>>,
}

impl JournalState {
    fn record(&mut self, event: &MainConversationEvent, at: DateTime<Utc>) {
        let turn_id = turn_of(event);
        if let MainConversationEvent::TurnEnded { .. } = event {
            if self.turn.as_ref().is_some_and(|t| t.turn_id == turn_id) {
                self.turn = None;
            }
            return;
        }
        // A turn's opening message comes before its `TurnStarted`, so either
        // can begin the record.
        let turn = match &mut self.turn {
            Some(turn) if turn.turn_id == turn_id => turn,
            slot => slot.insert(TurnRecord {
                turn_id: turn_id.to_owned(),
                started_at: at,
                entries: Vec::new(),
            }),
        };
        if let MainConversationEvent::TurnUsage(_) = event {
            // Each usage frame carries the running totals: only the newest counts.
            turn.entries
                .retain(|entry| !matches!(entry.event, MainConversationEvent::TurnUsage(_)));
        }
        if let Some(last) = turn.entries.last_mut()
            && merge_piece(&mut last.event, event)
        {
            return;
        }
        turn.entries.push(TurnEntry {
            at,
            event: event.clone(),
        });
    }
}

/// Add `next` to `last` when both are pieces of the same model call's stream.
fn merge_piece(last: &mut MainConversationEvent, next: &MainConversationEvent) -> bool {
    match (last, next) {
        (
            MainConversationEvent::TextDelta { call, text, .. },
            MainConversationEvent::TextDelta {
                call: next_call,
                text: piece,
                ..
            },
        )
        | (
            MainConversationEvent::ThinkingDelta { call, text, .. },
            MainConversationEvent::ThinkingDelta {
                call: next_call,
                text: piece,
                ..
            },
        ) if call == next_call => {
            text.push_str(piece);
            true
        }
        _ => false,
    }
}

/// The turn an event belongs to.
fn turn_of(event: &MainConversationEvent) -> &str {
    match event {
        MainConversationEvent::TurnStarted { turn_id, .. }
        | MainConversationEvent::TurnEnded { turn_id }
        | MainConversationEvent::UserMessage { turn_id, .. }
        | MainConversationEvent::TextDelta { turn_id, .. }
        | MainConversationEvent::ThinkingDelta { turn_id, .. }
        | MainConversationEvent::StreamRestart { turn_id, .. }
        | MainConversationEvent::Thinking { turn_id, .. }
        | MainConversationEvent::Intermediate { turn_id, .. }
        | MainConversationEvent::Response { turn_id, .. } => turn_id,
        MainConversationEvent::ToolCall { event, .. } => &event.correlation_id,
        MainConversationEvent::ToolResult(result) => &result.correlation_id,
        MainConversationEvent::TurnUsage(usage) => &usage.correlation_id,
    }
}

/// The main conversation's turn in flight, and the relay to the connections following it.
#[derive(Clone, Default)]
pub struct TurnJournal {
    state: Arc<Mutex<JournalState>>,
}

impl TurnJournal {
    /// A journal following the agent's `MainConversation` topic until the bus closes.
    ///
    /// # Errors
    ///
    /// Returns `BusError` if the subscription fails.
    pub async fn spawn(bus: &BusHandle) -> Result<Self, BusError> {
        let mut subscriber = bus.subscribe(topics::MainConversation).await?;
        let journal = Self::default();
        let feeder = journal.clone();
        crate::util::spawn_in_span(async move {
            loop {
                match subscriber.recv().await {
                    Ok(Some(event)) => feeder.observe(&event),
                    Ok(None) => break,
                    Err(e) => {
                        tracing::error!(error = %e, "turn journal stopped reading the main conversation");
                        break;
                    }
                }
            }
        });
        Ok(journal)
    }

    /// Record an event and relay it to every follower.
    pub fn observe(&self, event: &MainConversationEvent) {
        let mut state = self.lock();
        state.seq += 1;
        let seq = state.seq;
        state.record(event, Utc::now());
        // A follower whose connection closed drops its receiver; forget it.
        state.followers.retain(|follower| {
            follower
                .send(JournaledEvent {
                    seq,
                    event: event.clone(),
                })
                .is_ok()
        });
    }

    /// Follow the conversation from the next event on.
    #[must_use]
    pub fn follow(&self) -> mpsc::UnboundedReceiver<JournaledEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().followers.push(tx);
        rx
    }

    /// The turn in flight as of now.
    #[must_use]
    pub fn snapshot(&self) -> TurnSnapshot {
        let state = self.lock();
        TurnSnapshot {
            through: state.seq,
            turn: state.turn.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JournalState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::TurnOrigin;
    use crate::memory::types::Visibility;

    fn started(turn: &str) -> MainConversationEvent {
        MainConversationEvent::TurnStarted {
            turn_id: turn.into(),
            origin: TurnOrigin {
                endpoint: "web".into(),
                sender: None,
                visibility: Visibility::default(),
            },
        }
    }

    fn text(turn: &str, call: u32, piece: &str) -> MainConversationEvent {
        MainConversationEvent::TextDelta {
            turn_id: turn.into(),
            call,
            text: piece.into(),
        }
    }

    fn thinking(turn: &str, call: u32, piece: &str) -> MainConversationEvent {
        MainConversationEvent::ThinkingDelta {
            turn_id: turn.into(),
            call,
            text: piece.into(),
        }
    }

    fn describe(event: &MainConversationEvent) -> String {
        if let MainConversationEvent::TextDelta { call, text, .. } = event {
            format!("text#{call}:{text}")
        } else if let MainConversationEvent::ThinkingDelta { call, text, .. } = event {
            format!("thinking#{call}:{text}")
        } else if let MainConversationEvent::TurnStarted { turn_id, .. } = event {
            format!("started:{turn_id}")
        } else if let MainConversationEvent::UserMessage { content, .. } = event {
            format!("user:{content}")
        } else {
            format!("{event:?}")
        }
    }

    fn texts(snapshot: &TurnSnapshot) -> Vec<String> {
        snapshot
            .turn
            .iter()
            .flat_map(|t| &t.entries)
            .map(|entry| describe(&entry.event))
            .collect()
    }

    #[test]
    fn a_snapshot_holds_the_turn_so_far_with_each_calls_pieces_joined() {
        let journal = TurnJournal::default();
        journal.observe(&MainConversationEvent::UserMessage {
            id: "m1".into(),
            turn_id: "m1".into(),
            content: "hi".into(),
            images: vec![],
            sender: None,
            endpoint: "web".into(),
        });
        journal.observe(&started("m1"));
        journal.observe(&thinking("m1", 0, "hm"));
        journal.observe(&thinking("m1", 0, "m"));
        journal.observe(&text("m1", 0, "Hel"));
        journal.observe(&text("m1", 0, "lo"));
        journal.observe(&text("m1", 1, "Next"));

        let snapshot = journal.snapshot();
        assert_eq!(snapshot.through, 7);
        assert_eq!(
            snapshot.turn.as_ref().map(|t| t.turn_id.as_str()),
            Some("m1")
        );
        assert_eq!(
            texts(&snapshot),
            [
                "user:hi",
                "started:m1",
                "thinking#0:hmm",
                "text#0:Hello",
                "text#1:Next"
            ]
        );
    }

    #[test]
    fn a_turn_that_ended_leaves_nothing_to_show() {
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        journal.observe(&text("t1", 0, "done"));
        journal.observe(&MainConversationEvent::TurnEnded {
            turn_id: "t1".into(),
        });

        let snapshot = journal.snapshot();
        assert_eq!(snapshot.through, 3);
        assert!(snapshot.turn.is_none());
    }

    #[test]
    fn a_new_turn_replaces_the_one_before_it() {
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        journal.observe(&text("t1", 0, "old"));
        journal.observe(&started("t2"));
        journal.observe(&text("t2", 0, "new"));

        assert_eq!(texts(&journal.snapshot()), ["started:t2", "text#0:new"]);
    }

    #[test]
    fn only_the_newest_usage_is_kept() {
        let usage = |tokens: u32| {
            MainConversationEvent::TurnUsage(crate::bus::TurnUsageEvent {
                correlation_id: "t1".into(),
                output_tokens: tokens,
                has_usage: true,
                tool_calls: 0,
                session_totals: None,
            })
        };
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        journal.observe(&usage(5));
        journal.observe(&text("t1", 0, "a"));
        journal.observe(&usage(9));

        let snapshot = journal.snapshot();
        let usages: Vec<u32> = snapshot
            .turn
            .iter()
            .flat_map(|t| &t.entries)
            .filter_map(|entry| {
                if let MainConversationEvent::TurnUsage(u) = &entry.event {
                    Some(u.output_tokens)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(usages, [9]);
    }

    #[tokio::test]
    async fn followers_get_every_event_numbered_in_order() {
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        let mut follower = journal.follow();
        journal.observe(&text("t1", 0, "a"));
        journal.observe(&text("t1", 0, "b"));

        let first = follower.recv().await.unwrap();
        let second = follower.recv().await.unwrap();
        assert_eq!((first.seq, second.seq), (2, 3));
        assert!(
            matches!(second.event, MainConversationEvent::TextDelta { ref text, .. } if text == "b")
        );
    }

    #[tokio::test]
    async fn the_journal_follows_the_main_conversation_topic() {
        let bus = crate::bus::spawn_broker();
        let journal = TurnJournal::spawn(&bus).await.unwrap();
        let mut follower = journal.follow();
        bus.publisher()
            .publish(topics::MainConversation, started("t1"))
            .await
            .unwrap();

        let relayed = follower.recv().await.unwrap();
        assert_eq!(relayed.seq, 1);
        assert_eq!(
            journal.snapshot().turn.map(|t| t.turn_id),
            Some("t1".to_owned())
        );
    }
}
