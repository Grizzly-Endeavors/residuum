use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use residuum_bus::{BusError, BusHandle, MainConversationEvent, topics};
use residuum_protocol::QueuedMessage;

#[derive(Debug)]
pub struct JournaledEvent {
    pub seq: u64,
    pub event: MainConversationEvent,
}

#[derive(Debug, Clone)]
pub struct TurnEntry {
    pub at: DateTime<Utc>,
    pub event: MainConversationEvent,
}

#[derive(Debug, Clone)]
pub struct TurnRecord {
    pub turn_id: String,
    pub started_at: DateTime<Utc>,
    pub entries: Vec<TurnEntry>,
}

#[derive(Debug)]
pub struct TurnSnapshot {
    /// The last event the snapshot covers: relayed events up to here are in it.
    pub through: u64,
    pub turn: Option<TurnRecord>,
    pub queued: Vec<QueuedMessage>,
}

#[derive(Default)]
struct JournalState {
    seq: u64,
    turn: Option<TurnRecord>,
    queued: Vec<QueuedMessage>,
    followers: Vec<mpsc::UnboundedSender<JournaledEvent>>,
    closed: bool,
}

impl JournalState {
    fn record(&mut self, event: &MainConversationEvent, at: DateTime<Utc>) {
        match event {
            MainConversationEvent::MessageQueued {
                id,
                content,
                images,
                attachments,
                sender,
                endpoint,
            } => {
                if !self.queued.iter().any(|queued| &queued.id == id) {
                    self.queued.push(QueuedMessage {
                        id: id.clone(),
                        content: content.clone(),
                        images: images.clone(),
                        attachments: attachments.clone(),
                        sender: sender.clone(),
                        endpoint: endpoint.clone(),
                    });
                }
                return;
            }
            MainConversationEvent::MessageWithdrawn { id } => {
                self.queued.retain(|queued| &queued.id != id);
                return;
            }
            MainConversationEvent::UserMessage { id, .. } => {
                self.queued.retain(|queued| &queued.id != id);
            }
            MainConversationEvent::TurnStarted { .. }
            | MainConversationEvent::TurnEnded { .. }
            | MainConversationEvent::ToolCall { .. }
            | MainConversationEvent::ToolResult(_)
            | MainConversationEvent::TextDelta { .. }
            | MainConversationEvent::ThinkingDelta { .. }
            | MainConversationEvent::StreamRestart { .. }
            | MainConversationEvent::Thinking { .. }
            | MainConversationEvent::Intermediate { .. }
            | MainConversationEvent::Response { .. }
            | MainConversationEvent::TurnUsage(_) => {}
        }
        let Some(turn_id) = turn_of(event) else {
            return;
        };
        if let MainConversationEvent::TurnEnded { .. } = event {
            if self.turn.as_ref().is_some_and(|t| t.turn_id == turn_id) {
                self.turn = None;
            }
            return;
        }
        let turn = match &mut self.turn {
            Some(turn) if turn.turn_id == turn_id => turn,
            slot => slot.insert(TurnRecord {
                turn_id: turn_id.to_owned(),
                started_at: at,
                entries: Vec::new(),
            }),
        };
        if let MainConversationEvent::TurnUsage(_) = event {
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

fn turn_of(event: &MainConversationEvent) -> Option<&str> {
    let turn_id = match event {
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
        MainConversationEvent::MessageQueued { .. }
        | MainConversationEvent::MessageWithdrawn { .. } => return None,
    };
    Some(turn_id)
}

#[derive(Clone, Default)]
pub struct TurnJournal {
    state: Arc<Mutex<JournalState>>,
}

impl TurnJournal {
    pub async fn spawn(bus: &BusHandle) -> Result<Self, BusError> {
        let mut subscriber = bus.subscribe(topics::MainConversation).await?;
        let journal = Self::default();
        let feeder = journal.clone();
        residuum_core::util::spawn_in_span(async move {
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
            feeder.close();
        });
        Ok(journal)
    }

    #[must_use]
    pub fn closed() -> Self {
        let journal = Self::default();
        journal.close();
        journal
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.followers.clear();
    }

    pub fn observe(&self, event: &MainConversationEvent) {
        let mut state = self.lock();
        state.seq += 1;
        let seq = state.seq;
        state.record(event, Utc::now());
        state.followers.retain(|follower| {
            follower
                .send(JournaledEvent {
                    seq,
                    event: event.clone(),
                })
                .is_ok()
        });
    }

    #[must_use]
    pub fn follow(&self) -> mpsc::UnboundedReceiver<JournaledEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.lock();
        if !state.closed {
            state.followers.push(tx);
        }
        rx
    }

    #[must_use]
    pub fn snapshot(&self) -> TurnSnapshot {
        let state = self.lock();
        TurnSnapshot {
            through: state.seq,
            turn: state.turn.clone(),
            queued: state.queued.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JournalState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::TurnOrigin;
    use residuum_memory::types::Visibility;

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
            attachments: vec![],
            sender: None,
            agent_sender: None,
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
            MainConversationEvent::TurnUsage(residuum_bus::TurnUsageEvent {
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
    async fn following_a_journal_that_reads_nothing_ends_at_once() {
        let mut follower = TurnJournal::closed().follow();
        assert!(follower.recv().await.is_none());
    }

    #[tokio::test]
    async fn the_journal_follows_the_main_conversation_topic() {
        let bus = residuum_bus::spawn_broker();
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

    fn queued(id: &str) -> MainConversationEvent {
        MainConversationEvent::MessageQueued {
            id: id.into(),
            content: format!("say {id}"),
            images: vec![],
            attachments: vec![],
            sender: None,
            endpoint: "ws".into(),
        }
    }

    fn read(id: &str, turn: &str) -> MainConversationEvent {
        MainConversationEvent::UserMessage {
            id: id.into(),
            turn_id: turn.into(),
            content: format!("say {id}"),
            images: vec![],
            attachments: vec![],
            sender: None,
            agent_sender: None,
            endpoint: "ws".into(),
        }
    }

    fn waiting(journal: &TurnJournal) -> Vec<String> {
        journal
            .snapshot()
            .queued
            .into_iter()
            .map(|message| message.id)
            .collect()
    }

    #[test]
    fn a_snapshot_lists_the_messages_still_waiting_to_be_read() {
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        journal.observe(&queued("a"));
        journal.observe(&queued("b"));
        journal.observe(&queued("c"));
        assert_eq!(waiting(&journal), ["a", "b", "c"]);

        journal.observe(&read("a", "t1"));
        journal.observe(&MainConversationEvent::MessageWithdrawn { id: "b".into() });
        journal.observe(&MainConversationEvent::TurnEnded {
            turn_id: "t1".into(),
        });
        assert_eq!(
            waiting(&journal),
            ["c"],
            "waiting outlasts the turn it waited on"
        );
        journal.observe(&read("c", "c"));
        let actual = &waiting(&journal);
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
    }

    #[test]
    fn a_waiting_message_keeps_its_attachments_in_the_snapshot() {
        let attachment = residuum_inference::MessageAttachment {
            item_id: "item-1".to_string(),
            filename: "notes.md".to_string(),
            size: 12,
            content_type: "text/markdown".to_string(),
            path: "inbox/agent/attachments/item-1/notes.md".to_string(),
        };
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        journal.observe(&MainConversationEvent::MessageQueued {
            id: "a".into(),
            content: "see attached".into(),
            images: vec![],
            attachments: vec![attachment.clone()],
            sender: None,
            endpoint: "ws".into(),
        });
        let snapshot = journal.snapshot();
        let [waiting] = snapshot.queued.as_slice() else {
            panic!("expected one waiting message, got {:?}", snapshot.queued);
        };
        assert_eq!(waiting.attachments, vec![attachment]);
    }

    fn assert_opening_message_is_kept_with_its_sender(agent: &residuum_inference::AgentSender) {
        let journal = TurnJournal::default();
        journal.observe(&MainConversationEvent::UserMessage {
            id: "bg-1".into(),
            turn_id: "bg-1".into(),
            content: "status?".into(),
            images: vec![],
            attachments: vec![],
            sender: None,
            agent_sender: Some(agent.clone()),
            endpoint: "background".into(),
        });
        journal.observe(&started("bg-1"));

        let snapshot = journal.snapshot();
        assert_eq!(texts(&snapshot), ["user:status?", "started:bg-1"]);
        let opening = snapshot
            .turn
            .as_ref()
            .and_then(|t| t.entries.first())
            .map(|entry| &entry.event);
        assert!(
            matches!(
                opening,
                Some(MainConversationEvent::UserMessage { agent_sender: Some(sender), .. })
                    if sender == agent
            ),
            "{opening:?}"
        );
        assert!(snapshot.queued.is_empty(), "an agent's message never waits");
    }

    #[test]
    fn a_teammates_opening_message_is_kept_with_its_sender() {
        assert_opening_message_is_kept_with_its_sender(&residuum_inference::AgentSender {
            address: "agent:atlas".into(),
            category: "teammate".into(),
            run_id: None,
        });
    }

    #[test]
    fn a_sessions_opening_message_is_kept_with_its_sender_and_run() {
        assert_opening_message_is_kept_with_its_sender(&residuum_inference::AgentSender {
            address: "spawned-research-1a2b".into(),
            category: "spawned".into(),
            run_id: Some("run-1".into()),
        });
    }

    #[test]
    fn waiting_messages_are_no_part_of_the_turn() {
        let journal = TurnJournal::default();
        journal.observe(&started("t1"));
        journal.observe(&queued("a"));
        assert_eq!(texts(&journal.snapshot()), ["started:t1"]);
    }
}
